// GameBiomes.java: the biomes of chunks as the game's own classes make them, for
// `tests/game.rs` to hold this crate to.
//
// It asks the public classes of the server jar after `Bootstrap.bootStrap()`, as
// `tools/datagen/java/Extract.java` does, and starts no server and makes no world. For
// each chunk it does what the game's generator does in its biomes step
// (`ChunkGenerator.doCreateBiomes`, `ChunkAccess.fillBiomesFromNoise`,
// `LevelChunkSection.fillBiomesFromNoise`): a climate sampler with caches, the biome
// source's resolver for the chunk, and every cell of every section asked in the game's
// order, in one thread and in the order of the list. So the search of the parameter
// list remembers here exactly what it found for the cell before, across chunks too,
// and a replay in the same order can be held to every cell.
//
// The game samples the climate of a chunk's cells as volumes. Beside that the program
// asks the climate of every cell at its single position and writes down each cell at
// which the two differ.
//
// Needs a JDK 25 and the jar's classes on the class path; `tests/game.rs` says how it
// was run. What it writes is values computed by Mojang's code: Mojang's data, not
// under the licence of this file (see NOTICE.md at the root of the repository).
//
//   java GameBiomes <seed> <overworld|nether|end> <list of chunks> <output>
//
// The list has a chunk a line, `x z`. The output:
//
//   game-biomes 1
//   seed <seed>
//   dimension <name>
//   sections <lowest section> <count>
//   biome <number> <name>            for every biome that was found
//   chunk <x> <z> <section>...       a section is one number, or 64 joined by commas
//                                    in the order of y << 4 | z << 2 | x
//   differs <x> <y> <z> <six of the volume> <six of the single position>
//                                    a cell, in cells; none if the two always agree
//   cells <count> differing <count>

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.TreeMap;

import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityBufferPool;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public final class GameBiomes {
    public static void main(String[] args) throws IOException {
        if (args.length != 4) {
            throw new IllegalArgumentException(
                "usage: java GameBiomes <seed> <overworld|nether|end> <chunks> <output>");
        }
        long seed = Long.parseLong(args[0]);
        String dimension = args[1];

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        HolderLookup.Provider lookup = VanillaRegistries.createWorldLookup();
        HolderGetter<NormalNoise> noises = lookup.lookupOrThrow(Registries.NOISE);
        HolderLookup.RegistryLookup<NoiseGeneratorSettings> settings =
            lookup.lookupOrThrow(Registries.NOISE_SETTINGS);

        // The settings, the source and the heights of the dimension's type, as the
        // game's three built-in dimensions have them.
        ResourceKey<NoiseGeneratorSettings> key;
        BiomeSource source;
        int minY;
        int height;
        switch (dimension) {
            case "overworld" -> {
                key = NoiseGeneratorSettings.OVERWORLD;
                source = MultiNoiseBiomeSource.createFromPreset(
                    lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                        .getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
                minY = -64;
                height = 384;
            }
            case "nether" -> {
                key = NoiseGeneratorSettings.NETHER;
                source = MultiNoiseBiomeSource.createFromPreset(
                    lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                        .getOrThrow(MultiNoiseBiomeSourceParameterLists.NETHER));
                minY = 0;
                height = 256;
            }
            case "end" -> {
                key = NoiseGeneratorSettings.END;
                source = TheEndBiomeSource.create(lookup.lookupOrThrow(Registries.BIOME));
                minY = 0;
                height = 256;
            }
            default -> throw new IllegalArgumentException("no dimension " + dimension);
        }
        RandomState state = RandomState.create(noises, seed, settings.getOrThrow(key).value());
        Climate.Sampler single = state.createClimateSampler(SamplerContext.EMPTY_UNCACHED);

        TreeMap<String, Integer> numbers = new TreeMap<>();
        List<String> names = new ArrayList<>();
        StringBuilder chunks = new StringBuilder();
        StringBuilder differing = new StringBuilder();
        long cells = 0;
        long differ = 0;
        int lowestSection = minY >> 4;
        int sections = height >> 4;

        for (String line : Files.readAllLines(Path.of(args[2]), StandardCharsets.UTF_8)) {
            if (line.isBlank()) {
                continue;
            }
            String[] fields = line.trim().split(" ");
            int chunkX = Integer.parseInt(fields[0]);
            int chunkZ = Integer.parseInt(fields[1]);
            int cellX = chunkX << 2;
            int cellY = minY >> 2;
            int cellZ = chunkZ << 2;

            // The biomes step of the game's generator.
            DensityBufferPool pool = state.acquireDensityBufferPool();
            int[][] found = new int[sections][64];
            try {
                SamplerContext context =
                    SamplerContext.builder().enableCaches().useBufferArena(pool).build();
                Climate.Sampler sampler = state.createClimateSampler(context);
                BiomeResolver resolver = source.createResolverForChunk(
                    sampler, cellX, cellY, cellZ, 4, height >> 2, 4);
                for (int section = 0; section < sections; section++) {
                    int sectionY = (lowestSection + section) << 2;
                    for (int x = 0; x < 4; x++) {
                        for (int y = 0; y < 4; y++) {
                            for (int z = 0; z < 4; z++) {
                                Holder<Biome> biome =
                                    resolver.getNoiseBiome(cellX + x, sectionY + y, cellZ + z);
                                String name = biome.unwrapKey().orElseThrow().identifier().toString();
                                Integer number = numbers.get(name);
                                if (number == null) {
                                    number = names.size();
                                    numbers.put(name, number);
                                    names.add(name);
                                }
                                found[section][y << 4 | z << 2 | x] = number;
                            }
                        }
                    }
                }
            } finally {
                state.releaseDensityBufferPool(pool);
            }

            chunks.append("chunk ").append(chunkX).append(' ').append(chunkZ);
            for (int[] section : found) {
                boolean uniform = true;
                for (int cell : section) {
                    uniform &= cell == section[0];
                }
                chunks.append(' ').append(section[0]);
                for (int cell = 1; !uniform && cell < 64; cell++) {
                    chunks.append(',').append(section[cell]);
                }
            }
            chunks.append('\n');

            // The climate as volumes, as the multi-noise source's resolver for a chunk
            // samples it, against the climate at single positions.
            DensityVolume volume = new DensityVolume(
                4, height >> 2, 4, cellX << 2, cellY << 2, cellZ << 2, 4, 4, 4);
            DensitySampler.Bound[] six = {
                single.temperature(), single.humidity(), single.continentalness(),
                single.erosion(), single.depth(), single.weirdness(),
            };
            DensityBuffer[] buffers = new DensityBuffer[6];
            for (int parameter = 0; parameter < 6; parameter++) {
                buffers[parameter] = DensityBuffer.createUnpooled(volume.size());
                six[parameter].sampleVolume(buffers[parameter], volume);
            }
            for (int x = 0; x < 4; x++) {
                for (int y = 0; y < height >> 2; y++) {
                    for (int z = 0; z < 4; z++) {
                        int index = volume.indexUnchecked(x, y, z);
                        Climate.TargetPoint inVolume = Climate.target(
                            buffers[0].get(index), buffers[1].get(index), buffers[2].get(index),
                            buffers[3].get(index), buffers[4].get(index), buffers[5].get(index));
                        Climate.TargetPoint alone = single.sample(cellX + x, cellY + y, cellZ + z);
                        cells++;
                        if (!inVolume.equals(alone)) {
                            differ++;
                            differing.append("differs ").append(cellX + x).append(' ');
                            differing.append(cellY + y).append(' ').append(cellZ + z);
                            for (Climate.TargetPoint point : List.of(inVolume, alone)) {
                                differing.append(' ').append(point.temperature());
                                differing.append(' ').append(point.humidity());
                                differing.append(' ').append(point.continentalness());
                                differing.append(' ').append(point.erosion());
                                differing.append(' ').append(point.depth());
                                differing.append(' ').append(point.weirdness());
                            }
                            differing.append('\n');
                        }
                    }
                }
            }
        }

        StringBuilder text = new StringBuilder();
        text.append("game-biomes 1\n");
        text.append("seed ").append(seed).append('\n');
        text.append("dimension ").append(dimension).append('\n');
        text.append("sections ").append(lowestSection).append(' ').append(sections).append('\n');
        for (int number = 0; number < names.size(); number++) {
            text.append("biome ").append(number).append(' ').append(names.get(number)).append('\n');
        }
        text.append(chunks);
        text.append(differing);
        text.append("cells ").append(cells).append(" differing ").append(differ).append('\n');
        Files.writeString(Path.of(args[3]), text.toString(), StandardCharsets.UTF_8);
    }
}
