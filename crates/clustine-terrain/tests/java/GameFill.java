// GameFill.java: the blocks of chunks after the game's "fill from noise" step and
// nothing after it, for `tests/game_fill.rs` to hold this crate to.
//
// It asks the classes of the server jar after `Bootstrap.bootStrap()`, as
// `GameBiomes.java` does, and starts no server and makes no world. For each chunk of a
// list, in one thread and in the list's order:
//
// - **The fill itself.** A `NoiseBasedChunkGenerator` of the dimension's settings, a
//   `ProtoChunk` of the dimension's heights, and a `NoiseChunk` made as the generator's
//   `createNoiseChunk` makes it, but with the empty beardifier (no structure is near)
//   and the blender that blends nothing. Then the generator's own `doFill`, which is
//   private and is called by reflection, between the `acquire` and `release` of the
//   sections that `buildTerrain` puts around it. The surface rules, the carvers and
//   everything later are not called. The blocks and the positions marked for
//   post-processing are read back from the chunk.
// - **The density.** The final density of the chunk as a volume, which is what
//   `doFill` reads, from a second noise chunk: a hash of its bits for every chunk, and
//   the bits themselves for the first chunks of the list (`<gate>` of them).
// - **The corners.** For those first chunks, each function the final density reaches
//   through `interpolated`: its input at the corners of the chunk's cells as a volume
//   (the way the game fills), and how many of those values differ in a bit from the
//   input asked at the corner alone.
// - **The aquifer's ceiling** (`skipSamplingAboveY`, a private field read by
//   reflection), where the dimension has aquifers.
// - **Base columns.** `getBaseColumn` of the generator for two columns of each chunk:
//   the column as the game makes it alone, with an aquifer of its own, which is what
//   `getBaseHeight` looks through. The heights themselves are not written: the ocean
//   floor's asks a block tag, and tags are bound to nothing where no server loaded
//   them, so that every such height comes out as the bottom of the world here. The
//   world surface's (not air) is written, as a check of the column.
//
// In 26.3 the ore veins are no part of this step: they are a material rule
// (`OreVeinRule`) and come with the surface. So there is one kind of output, and no
// block of it is an ore.
//
// Needs a JDK 25 and the jar's classes on the class path; `tests/game_fill.rs` says how
// it was run. What it writes is values computed by Mojang's code: Mojang's data, not
// under the licence of this file (see NOTICE.md at the root of the repository).
//
//   java GameFill <seed> <overworld|nether|end> <list of chunks> <gate> <output>
//
// The list has a chunk a line, `x z`. The output, a line each:
//
//   game-fill 1
//   seed <seed>
//   dimension <name>
//   noise <lowest y> <height> <sea level> <default block> <default fluid>
//   constants <way below min y>
//   interpolated <number> <cell size xz> <cell size y>   for each one in the final density
//   chunk <x> <z>
//   density <hash>              FNV-1a, 64 bits, over the bits of the volume, each as
//                               four bytes from the lowest, in the volume's order
//   blocks <runs>               runs of `<state id>*<count>` in the volume's order:
//                               index = y + (x + z * 16) * height, from the lowest y
//   marks <x>,<y>,<z> ...       the marked positions in the chunk, as the chunk kept them
//   ceiling <y>                 or `ceiling none`
//   column <x> <z> <world surface> <runs>   twice: a block position, `getBaseHeight`
//                               for the world surface, and the column from the lowest y
//   bits <hex>                  gate only: the density, eight digits a value
//   corners <number> <differing from the corner alone> <hex>
//                               gate only: 5 by (height / cell + 1) by 5 values in the
//                               order index = y + (x + z * 5) * size y
//   end <chunks>

import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;

import it.unimi.dsi.fastutil.shorts.ShortList;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.IdMap;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.NoiseColumn;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.Strategy;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.DfRewriteRule;
import net.minecraft.world.level.levelgen.densityfunction.ScopedDensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.op.InterpolatedFunction;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public final class GameFill {
    /// The functions marked `interpolated` that a function reaches, each once, in the
    /// order they are met.
    private static void interpolated(DensityFunction function, List<InterpolatedFunction> found) {
        DensityFunction inlined = DfRewriteRule.INLINE_REFERENCE.rewrite(function);
        if (inlined != function) {
            interpolated(inlined, found);
            return;
        }
        if (function instanceof InterpolatedFunction marked && !found.contains(marked)) {
            found.add(marked);
        }
        function.rewriteChildren(child -> {
            interpolated(child, found);
            return child;
        });
    }

    private static void hex(StringBuilder text, float value) {
        String digits = Integer.toHexString(Float.floatToRawIntBits(value));
        for (int pad = digits.length(); pad < 8; pad++) {
            text.append('0');
        }
        text.append(digits);
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 5) {
            throw new IllegalArgumentException(
                "usage: java GameFill <seed> <overworld|nether|end> <chunks> <gate> <output>");
        }
        long seed = Long.parseLong(args[0]);
        String dimension = args[1];
        int gate = Integer.parseInt(args[3]);

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        HolderLookup.Provider lookup = VanillaRegistries.createWorldLookup();
        HolderGetter<NormalNoise> noises = lookup.lookupOrThrow(Registries.NOISE);
        HolderLookup.RegistryLookup<NoiseGeneratorSettings> allSettings =
            lookup.lookupOrThrow(Registries.NOISE_SETTINGS);

        // The settings, a biome source (the fill never asks it) and the heights of
        // the dimension's type, as the game's three built-in dimensions have them.
        ResourceKey<NoiseGeneratorSettings> key;
        BiomeSource source;
        int dimensionMinY;
        int dimensionHeight;
        switch (dimension) {
            case "overworld" -> {
                key = NoiseGeneratorSettings.OVERWORLD;
                source = MultiNoiseBiomeSource.createFromPreset(
                    lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                        .getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
                dimensionMinY = -64;
                dimensionHeight = 384;
            }
            case "nether" -> {
                key = NoiseGeneratorSettings.NETHER;
                source = MultiNoiseBiomeSource.createFromPreset(
                    lookup.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                        .getOrThrow(MultiNoiseBiomeSourceParameterLists.NETHER));
                dimensionMinY = 0;
                dimensionHeight = 256;
            }
            case "end" -> {
                key = NoiseGeneratorSettings.END;
                source = TheEndBiomeSource.create(lookup.lookupOrThrow(Registries.BIOME));
                dimensionMinY = 0;
                dimensionHeight = 256;
            }
            default -> throw new IllegalArgumentException("no dimension " + dimension);
        }
        Holder<NoiseGeneratorSettings> held = allSettings.getOrThrow(key);
        NoiseGeneratorSettings settings = held.value();
        RandomState state = RandomState.create(noises, seed, settings);
        NoiseBasedChunkGenerator generator = new NoiseBasedChunkGenerator(source, held);
        LevelHeightAccessor heights = LevelHeightAccessor.create(dimensionMinY, dimensionHeight);

        Method createFluidPicker = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "createFluidPicker", NoiseGeneratorSettings.class);
        createFluidPicker.setAccessible(true);
        Aquifer.FluidPicker fluidPicker = (Aquifer.FluidPicker) createFluidPicker.invoke(null, settings);
        Method doFill = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "doFill", NoiseChunk.class, ChunkAccess.class);
        doFill.setAccessible(true);

        // A chunk's sections need a list of biomes to be made at all; the fill writes
        // no biome, so the list holds one.
        Holder<Biome> plains = lookup.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
        IdMap<Holder<Biome>> biomes = new IdMap<>() {
            @Override
            public int getId(Holder<Biome> biome) {
                return biome == plains ? 0 : -1;
            }

            @Override
            public Holder<Biome> byId(int id) {
                return id == 0 ? plains : null;
            }

            @Override
            public int size() {
                return 1;
            }

            @Override
            public Iterator<Holder<Biome>> iterator() {
                return List.of(plains).iterator();
            }
        };
        Strategy<BlockState> blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        BlockState air = Blocks.AIR.defaultBlockState();
        PalettedContainerFactory containers = new PalettedContainerFactory(
            blockStrategy, air, PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, air),
            Strategy.createForBiomes(biomes), plains, null);

        DensityFunction finalDensity = settings.noiseRouter().finalDensity();
        List<InterpolatedFunction> marked = new ArrayList<>();
        interpolated(finalDensity, marked);

        int minY = settings.noiseSettings().minY();
        int height = settings.noiseSettings().height();

        try (BufferedWriter out = Files.newBufferedWriter(Path.of(args[4]), StandardCharsets.UTF_8)) {
            out.write("game-fill 1\n");
            out.write("seed " + seed + "\n");
            out.write("dimension " + dimension + "\n");
            out.write("noise " + minY + " " + height + " " + settings.seaLevel() + " "
                + Block.getId(settings.defaultBlock()) + " " + Block.getId(settings.defaultFluid()) + "\n");
            out.write("constants " + DimensionType.WAY_BELOW_MIN_Y + "\n");
            for (int number = 0; number < marked.size(); number++) {
                out.write("interpolated " + number + " " + marked.get(number).cellSizeXz() + " "
                    + marked.get(number).cellSizeY() + "\n");
            }

            int chunks = 0;
            for (String line : Files.readAllLines(Path.of(args[2]), StandardCharsets.UTF_8)) {
                if (line.isBlank()) {
                    continue;
                }
                String[] fields = line.trim().split(" ");
                ChunkPos pos = new ChunkPos(Integer.parseInt(fields[0]), Integer.parseInt(fields[1]));
                DensityVolume volume = new DensityVolume(
                    16, height, 16, pos.getMinBlockX(), minY, pos.getMinBlockZ());
                out.write("chunk " + pos.x() + " " + pos.z() + "\n");

                // The fill, by the generator's own method.
                ProtoChunk chunk = new ProtoChunk(pos, UpgradeData.EMPTY, heights, containers, null);
                Integer ceiling = null;
                try (NoiseChunk noiseChunk = new NoiseChunk(
                        state, Beardifier.EMPTY, settings, fluidPicker, Blender.empty(), volume)) {
                    int top = chunk.getSectionIndex(volume.maxBlockY());
                    int bottom = chunk.getSectionIndex(volume.minBlockY());
                    for (int section = top; section >= bottom; section--) {
                        chunk.getSection(section).acquire();
                    }
                    try {
                        doFill.invoke(generator, noiseChunk, chunk);
                    } finally {
                        for (int section = top; section >= bottom; section--) {
                            chunk.getSection(section).release();
                        }
                    }
                    Aquifer aquifer = noiseChunk.aquifer();
                    if (aquifer.getClass().getSimpleName().equals("NoiseBasedAquifer")) {
                        Field field = aquifer.getClass().getDeclaredField("skipSamplingAboveY");
                        field.setAccessible(true);
                        ceiling = field.getInt(aquifer);
                    }
                }

                // The density the fill read, from a noise chunk of its own.
                StringBuilder bits = new StringBuilder();
                List<String> corners = new ArrayList<>();
                long hash = 0xcbf29ce484222325L;
                try (NoiseChunk noiseChunk = new NoiseChunk(
                        state, Beardifier.EMPTY, settings, fluidPicker, Blender.empty(), volume)) {
                    DensitySampler.Bound sampler = noiseChunk.cachingSamplers().get(finalDensity);
                    try (ScopedDensityBuffer buffer = sampler.sampleVolume(volume)) {
                        for (int index = 0; index < volume.size(); index++) {
                            float value = buffer.get(index);
                            int raw = Float.floatToRawIntBits(value);
                            for (int shift = 0; shift < 32; shift += 8) {
                                hash ^= (raw >>> shift) & 0xff;
                                hash *= 0x100000001b3L;
                            }
                            if (chunks < gate) {
                                hex(bits, value);
                            }
                        }
                    }
                    for (int number = 0; chunks < gate && number < marked.size(); number++) {
                        InterpolatedFunction function = marked.get(number);
                        int cellXz = function.cellSizeXz();
                        int cellY = function.cellSizeY();
                        DensityVolume atCorners = new DensityVolume(
                            16 / cellXz + 1, height / cellY + 1, 16 / cellXz + 1,
                            pos.getMinBlockX(), minY, pos.getMinBlockZ(), cellXz, cellY, cellXz);
                        DensitySampler.Bound input = noiseChunk.cachingSamplers().get(function.input());
                        StringBuilder text = new StringBuilder();
                        int differing = 0;
                        try (ScopedDensityBuffer buffer = input.sampleVolume(atCorners)) {
                            for (int z = 0; z < atCorners.sizeZ(); z++) {
                                for (int x = 0; x < atCorners.sizeX(); x++) {
                                    for (int y = 0; y < atCorners.sizeY(); y++) {
                                        float value = buffer.get(atCorners.indexUnchecked(x, y, z));
                                        float alone = state.sampleBlockValueUncached(
                                            function.input(), atCorners.blockX(x), atCorners.blockY(y),
                                            atCorners.blockZ(z));
                                        if (Float.floatToRawIntBits(value) != Float.floatToRawIntBits(alone)) {
                                            differing++;
                                        }
                                        hex(text, value);
                                    }
                                }
                            }
                        }
                        corners.add("corners " + number + " " + differing + " " + text + "\n");
                    }
                }
                out.write("density " + Long.toHexString(hash) + "\n");

                // The blocks, in runs.
                StringBuilder runs = new StringBuilder("blocks");
                int last = -1;
                int count = 0;
                for (int z = 0; z < 16; z++) {
                    for (int x = 0; x < 16; x++) {
                        for (int y = minY; y < minY + height; y++) {
                            LevelChunkSection section = chunk.getSection(chunk.getSectionIndex(y));
                            int id = Block.getId(section.getBlockState(x, y & 15, z));
                            if (id != last) {
                                if (count > 0) {
                                    runs.append(' ').append(last).append('*').append(count);
                                }
                                last = id;
                                count = 0;
                            }
                            count++;
                        }
                    }
                }
                runs.append(' ').append(last).append('*').append(count).append('\n');
                out.write(runs.toString());

                StringBuilder marks = new StringBuilder("marks");
                ShortList[] lists = chunk.getPostProcessing();
                for (int section = 0; section < lists.length; section++) {
                    if (lists[section] == null) {
                        continue;
                    }
                    for (short packed : lists[section]) {
                        int y = (chunk.getMinSectionY() + section) * 16 + (packed >>> 4 & 15);
                        marks.append(' ').append(packed & 15).append(',').append(y).append(',')
                            .append(packed >>> 8 & 15);
                    }
                }
                out.write(marks.append('\n').toString());
                out.write("ceiling " + (ceiling == null ? "none" : ceiling.toString()) + "\n");

                // Two columns that move about the chunk with its position.
                for (int column = 0; column < 2; column++) {
                    int x = pos.getMinBlockX() + Math.floorMod(pos.x() * 7 + pos.z() * 3 + column * 5, 16);
                    int z = pos.getMinBlockZ() + Math.floorMod(pos.x() * 5 + pos.z() * 11 + column * 9, 16);
                    StringBuilder text = new StringBuilder("column ");
                    text.append(x).append(' ').append(z).append(' ').append(generator.getBaseHeight(
                        x, z, Heightmap.Types.WORLD_SURFACE_WG, heights, state));
                    NoiseColumn alone = generator.getBaseColumn(x, z, heights, state);
                    int before = -1;
                    int run = 0;
                    for (int y = minY; y < minY + height; y++) {
                        int id = Block.getId(alone.getBlock(y));
                        if (id != before) {
                            if (run > 0) {
                                text.append(' ').append(before).append('*').append(run);
                            }
                            before = id;
                            run = 0;
                        }
                        run++;
                    }
                    text.append(' ').append(before).append('*').append(run).append('\n');
                    out.write(text.toString());
                }

                if (chunks < gate) {
                    out.write("bits " + bits + "\n");
                    for (String text : corners) {
                        out.write(text);
                    }
                }
                chunks++;
            }
            out.write("end " + chunks + "\n");
        }
    }
}
