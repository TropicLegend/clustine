// GameSurface.java: the blocks of chunks after the game's "fill from noise" and "build
// surface" steps and nothing after them, for `tests/game_surface.rs` to hold this crate
// to.
//
// It asks the classes of the server jar after `Bootstrap.bootStrap()`, as
// `GameFill.java` does, and starts no server and makes no world. For each chunk of a
// list, in one thread and in the list's order:
//
// - **The biomes**, by the game's biome step as `GameBiomes.java` does it (what the
//   generator's private `doCreateBiomes` does: a climate sampler with caches, the
//   source's resolver for the chunk, `ChunkAccess.fillBiomesFromNoise`), for the chunk
//   and its eight neighbours, each in a `ProtoChunk` of its own, whose status is then
//   set to `BIOMES`, as the game's is by then: a chunk gives no biome before.
// - **The fill**, as `GameFill.java` does it: a `NoiseChunk` made as the generator's
//   `createNoiseChunk` makes it, but with the empty beardifier and the blender that
//   blends nothing, and the generator's own private `doFill`, called by reflection.
// - **The surface**: the generator's own private `buildSurface`, called by reflection
//   with the same noise chunk, the dimension's material rule from its settings, the
//   biomes that the palettes of the nine chunks hold (what
//   `ChunkStatusTasks.collectPossibleBiomes` collects) and a `BiomeManager`. So the
//   `WorldGenerationContext`, the `MaterialRuleContext`, the rule's compilation, the
//   loop over the columns, the pillars and the icebergs are all the game's.
//   The carvers and everything later are not called.
//
// **What is built by hand here**, where the game has a `WorldGenRegion` of a server's
// level: the `BiomeManager` is made of the game's class with the game's
// `obfuscateSeed` of the seed, as the region makes its own, but over a resolver
// written here that does what `LevelReader.getNoiseBiome` does through a region: it
// asks the `ProtoChunk` the cell lies in for `getNoiseBiome`, so the holding of the
// height within the chunk is the game's. The neighbours' chunks are kept in a small
// cache; the nine that a chunk reads are held while it is made. A biome that is an
// exact tie between two entries depends on what the thread searched before, so the
// biomes that were read are written out, and the test gives them to this crate.
//
// No tag is bound where no server loaded them. Nothing of this step asks one: the
// rules name their biomes one by one.
//
// For the first chunks of the list (`<gate>` of them) it also writes what the surface
// is made from, each asked of the game's classes beside the step: the router's
// `chunk_surface_level` as the volume the rules sample, the depth of the surface and
// its second noise at every column (two protected methods, by reflection), the biome
// at blocks through the `BiomeManager`, whether it is cold enough to snow for three
// biomes, the band of terracotta at one height of each column, and, where the rule
// has ore veins,
// hashes of the three density functions as the rule samples them.
//
// Needs a JDK 25 and the jar's classes on the class path; `tests/game_surface.rs` says
// how it was run. What it writes is values computed by Mojang's code: Mojang's data,
// not under the licence of this file (see NOTICE.md at the root of the repository).
//
//   java GameSurface <seed> <overworld|nether|end> <list of chunks> <gate> <output>
//
// The list has a chunk a line, `x z`. The output, a line each:
//
//   game-surface 1
//   seed <seed>
//   dimension <name>
//   noise <lowest y> <height> <sea level> <default block> <default fluid>
//   heights <lowest y of a chunk> <height of a chunk>
//   biome <number> <name>       every biome of the registry, sorted by name
//   bands <192 state ids>       the bands of terracotta, from the lowest
//   chunk <x> <z>
//   biomes <36 columns>         the cells from one before the chunk to one after it, x
//                               counting most, then z; a column is runs of
//                               `<biome>*<count>` joined by commas, from the lowest cell
//   filled <hash>               FNV-1a, 64 bits, over the state ids after the fill, each
//                               as two bytes from the lowest, in the order of `blocks`
//   blocks <runs>               after the surface: runs of `<state id>*<count>` in the
//                               order index = y + (x + z * 16) * height, from the lowest y
//   marks <x>,<y>,<z> ...       the positions marked for post-processing, as the chunk
//                               kept them
//   level <hex>                 gate only, as all that follow: `chunk_surface_level` at
//                               the 256 columns, index = x + z * 16, eight digits each
//   depth <256 numbers>         the surface's depth, same order
//   secondary <hex>             its second noise, sixteen digits each
//   zoom <first y> <step> <count> <biomes>
//                               the biome at blocks: for each column in that order,
//                               `count` heights from `first y`, `step` apart
//   cold <three biomes> <bits>  for each of three biomes, each column and the heights
//                               64, 100, 160 and 250: whether it is cold enough to snow
//   band <256 state ids>        the band of each column at the height 60 + x + 3 z
//   veins <size y> <hashes>     the height of the volume the rule samples in, and the
//                               hash of the bits of copper density, richness and iron
//                               density in that volume and of the gap at its blocks
//   end <chunks>

import java.io.BufferedWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

import it.unimi.dsi.fastutil.shorts.ShortList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.IdMap;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.biome.Climate;
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
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.DensityBuffer;
import net.minecraft.world.level.levelgen.densityfunction.DensityBufferPool;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.densityfunction.DensitySampler;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public final class GameSurface {
    /// How many neighbours' chunks of biomes are kept.
    private static final int KEPT = 2048;

    private static long fnv(long hash, int value, int bytes) {
        for (int shift = 0; shift < bytes * 8; shift += 8) {
            hash ^= (value >>> shift) & 0xff;
            hash *= 0x100000001b3L;
        }
        return hash;
    }

    private static void hex(StringBuilder text, long value, int digits) {
        String written = Long.toHexString(value);
        for (int pad = written.length(); pad < digits; pad++) {
            text.append('0');
        }
        text.append(written);
    }

    /// The state ids of a chunk's terrain as runs, and their hash.
    private static long blocks(ProtoChunk chunk, int minY, int height, StringBuilder runs) {
        long hash = 0xcbf29ce484222325L;
        int last = -1;
        int count = 0;
        for (int z = 0; z < 16; z++) {
            for (int x = 0; x < 16; x++) {
                for (int y = minY; y < minY + height; y++) {
                    LevelChunkSection section = chunk.getSection(chunk.getSectionIndex(y));
                    int id = Block.getId(section.getBlockState(x, y & 15, z));
                    hash = fnv(hash, id, 2);
                    if (id != last) {
                        if (count > 0 && runs != null) {
                            runs.append(' ').append(last).append('*').append(count);
                        }
                        last = id;
                        count = 0;
                    }
                    count++;
                }
            }
        }
        if (runs != null) {
            runs.append(' ').append(last).append('*').append(count);
        }
        return hash;
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 5) {
            throw new IllegalArgumentException(
                "usage: java GameSurface <seed> <overworld|nether|end> <chunks> <gate> <output>");
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
        HolderLookup.RegistryLookup<Biome> biomeRegistry = lookup.lookupOrThrow(Registries.BIOME);

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
                source = TheEndBiomeSource.create(biomeRegistry);
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
        MaterialSystem system = state.surfaceSystem();
        MaterialRule rule = settings.materialRule().value();

        Method createFluidPicker = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "createFluidPicker", NoiseGeneratorSettings.class);
        createFluidPicker.setAccessible(true);
        Aquifer.FluidPicker fluidPicker = (Aquifer.FluidPicker) createFluidPicker.invoke(null, settings);
        Method doFill = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "doFill", NoiseChunk.class, ChunkAccess.class);
        doFill.setAccessible(true);
        Method buildSurface = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "buildSurface", ChunkAccess.class, NoiseChunk.class, RandomState.class,
            BiomeManager.class, Set.class, MaterialRule.class);
        buildSurface.setAccessible(true);
        Method getSurfaceDepth = MaterialSystem.class.getDeclaredMethod(
            "getSurfaceDepth", int.class, int.class);
        getSurfaceDepth.setAccessible(true);
        Method getSurfaceSecondary = MaterialSystem.class.getDeclaredMethod(
            "getSurfaceSecondary", int.class, int.class);
        getSurfaceSecondary.setAccessible(true);
        Method getBand = MaterialSystem.class.getDeclaredMethod(
            "getBand", int.class, int.class, int.class);
        getBand.setAccessible(true);
        Field clayBands = MaterialSystem.class.getDeclaredField("clayBands");
        clayBands.setAccessible(true);

        // Every biome of the registry, numbered by name: the chunks' palettes and the
        // output count them so.
        List<Holder<Biome>> biomes = new ArrayList<>();
        biomeRegistry.listElements().forEach(biomes::add);
        biomes.sort(Comparator.comparing(
            biome -> biome.unwrapKey().orElseThrow().identifier().toString()));
        Map<Holder<Biome>, Integer> biomeNumbers = new HashMap<>();
        for (int number = 0; number < biomes.size(); number++) {
            biomeNumbers.put(biomes.get(number), number);
        }
        Holder<Biome> plains = biomeRegistry.getOrThrow(Biomes.PLAINS);
        IdMap<Holder<Biome>> biomeIds = new IdMap<>() {
            @Override
            public int getId(Holder<Biome> biome) {
                Integer number = biomeNumbers.get(biome);
                return number == null ? -1 : number;
            }

            @Override
            public Holder<Biome> byId(int id) {
                return id >= 0 && id < biomes.size() ? biomes.get(id) : null;
            }

            @Override
            public int size() {
                return biomes.size();
            }

            @Override
            public Iterator<Holder<Biome>> iterator() {
                return biomes.iterator();
            }
        };
        Strategy<BlockState> blockStrategy = Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY);
        BlockState air = Blocks.AIR.defaultBlockState();
        PalettedContainerFactory containers = new PalettedContainerFactory(
            blockStrategy, air, PalettedContainer.codecRW(BlockState.CODEC, blockStrategy, air),
            Strategy.createForBiomes(biomeIds), plains, null);

        int minY = settings.noiseSettings().minY();
        int height = settings.noiseSettings().height();
        int seaLevel = settings.seaLevel();
        int lowestCell = dimensionMinY >> 2;
        int cellsHigh = dimensionHeight >> 2;

        // The ore veins' density functions, where the dimension has them.
        HolderLookup.RegistryLookup<DensityFunction> functions =
            lookup.lookupOrThrow(Registries.DENSITY_FUNCTION);
        List<DensityFunction> veins = new ArrayList<>();
        if (dimension.equals("overworld")) {
            for (String name : List.of("copper_density", "richness", "iron_density", "gap")) {
                veins.add(functions.getOrThrow(ResourceKey.create(Registries.DENSITY_FUNCTION,
                    Identifier.withDefaultNamespace("overworld/ore_vein/" + name))).value());
            }
        }
        // The three biomes whose temperature is asked: one that is frozen in patches,
        // one that is cold only high up, and one that is warm.
        List<Holder<Biome>> asked = List.of(
            biomeRegistry.getOrThrow(Biomes.FROZEN_OCEAN),
            biomeRegistry.getOrThrow(Biomes.TAIGA),
            biomeRegistry.getOrThrow(Biomes.DEEP_FROZEN_OCEAN));

        // The biomes of chunks, each made by the game's biome step in a chunk of its
        // own; the least recently read is let go.
        LinkedHashMap<Long, ProtoChunk> kept = new LinkedHashMap<>(KEPT, 0.75f, true) {
            @Override
            protected boolean removeEldestEntry(Map.Entry<Long, ProtoChunk> eldest) {
                return size() > KEPT;
            }
        };

        try (BufferedWriter out = Files.newBufferedWriter(Path.of(args[4]), StandardCharsets.UTF_8)) {
            out.write("game-surface 1\n");
            out.write("seed " + seed + "\n");
            out.write("dimension " + dimension + "\n");
            out.write("noise " + minY + " " + height + " " + seaLevel + " "
                + Block.getId(settings.defaultBlock()) + " " + Block.getId(settings.defaultFluid()) + "\n");
            out.write("heights " + dimensionMinY + " " + dimensionHeight + "\n");
            for (int number = 0; number < biomes.size(); number++) {
                out.write("biome " + number + " "
                    + biomes.get(number).unwrapKey().orElseThrow().identifier() + "\n");
            }
            StringBuilder bands = new StringBuilder("bands");
            for (BlockState band : (BlockState[]) clayBands.get(system)) {
                bands.append(' ').append(Block.getId(band));
            }
            out.write(bands.append('\n').toString());

            int chunks = 0;
            for (String line : Files.readAllLines(Path.of(args[2]), StandardCharsets.UTF_8)) {
                if (line.isBlank()) {
                    continue;
                }
                String[] fields = line.trim().split(" ");
                ChunkPos pos = new ChunkPos(Integer.parseInt(fields[0]), Integer.parseInt(fields[1]));
                out.write("chunk " + pos.x() + " " + pos.z() + "\n");

                // The nine chunks with their biomes; the middle one is the chunk
                // that is made, and is new.
                ProtoChunk[] nine = new ProtoChunk[9];
                for (int dz = -1; dz <= 1; dz++) {
                    for (int dx = -1; dx <= 1; dx++) {
                        ChunkPos at = new ChunkPos(pos.x() + dx, pos.z() + dz);
                        long packed = ((long) at.x() << 32) ^ (at.z() & 0xffffffffL);
                        ProtoChunk made = dx == 0 && dz == 0 ? null : kept.get(packed);
                        if (made == null) {
                            made = new ProtoChunk(at, UpgradeData.EMPTY, heights, containers, null);
                            DensityBufferPool pool = state.acquireDensityBufferPool();
                            try {
                                SamplerContext context =
                                    SamplerContext.builder().enableCaches().useBufferArena(pool).build();
                                Climate.Sampler sampler = state.createClimateSampler(context);
                                BiomeResolver resolver = source.createResolverForChunk(
                                    sampler, at.getMinBlockX() >> 2, dimensionMinY >> 2,
                                    at.getMinBlockZ() >> 2, 4, dimensionHeight >> 2, 4);
                                made.fillBiomesFromNoise(resolver);
                                made.setPersistedStatus(ChunkStatus.BIOMES);
                            } finally {
                                state.releaseDensityBufferPool(pool);
                            }
                            if (dx != 0 || dz != 0) {
                                kept.put(packed, made);
                            }
                        }
                        nine[(dz + 1) * 3 + dx + 1] = made;
                    }
                }
                ProtoChunk chunk = nine[4];
                // What `LevelReader.getNoiseBiome` does through a region.
                BiomeResolver around = (x, y, z) -> {
                    int dx = (x >> 2) - pos.x();
                    int dz = (z >> 2) - pos.z();
                    if (dx < -1 || dx > 1 || dz < -1 || dz > 1) {
                        throw new IllegalStateException("a biome was asked outside the nine chunks");
                    }
                    return nine[(dz + 1) * 3 + dx + 1].getNoiseBiome(x, y, z);
                };
                BiomeManager manager = new BiomeManager(around, BiomeManager.obfuscateSeed(seed));
                Set<Holder<Biome>> possible = new HashSet<>();
                for (ProtoChunk one : nine) {
                    one.collectBiomesInPalette(possible);
                }

                StringBuilder cells = new StringBuilder("biomes");
                for (int x = -1; x <= 4; x++) {
                    for (int z = -1; z <= 4; z++) {
                        cells.append(' ');
                        int last = -1;
                        int count = 0;
                        for (int y = 0; y < cellsHigh; y++) {
                            int number = biomeNumbers.get(around.getNoiseBiome(
                                pos.x() * 4 + x, lowestCell + y, pos.z() * 4 + z));
                            if (number != last) {
                                if (count > 0) {
                                    cells.append(last).append('*').append(count).append(',');
                                }
                                last = number;
                                count = 0;
                            }
                            count++;
                        }
                        cells.append(last).append('*').append(count);
                    }
                }
                out.write(cells.append('\n').toString());

                DensityVolume volume = new DensityVolume(
                    16, height, 16, pos.getMinBlockX(), minY, pos.getMinBlockZ());
                int expectedSizeY;
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
                    out.write("filled " + Long.toHexString(blocks(chunk, minY, height, null)) + "\n");
                    // The volume the rule samples its density functions in, as
                    // `MaterialSystem.buildSurface` makes it before it goes through
                    // the columns.
                    int highest = chunk.getSectionYFromSectionIndex(
                        chunk.getHighestFilledSectionIndex()) * 16 + 15;
                    expectedSizeY = Math.max(highest - minY + 1, 1);

                    buildSurface.invoke(generator, chunk, noiseChunk, state, manager, possible, rule);
                }

                StringBuilder runs = new StringBuilder("blocks");
                blocks(chunk, minY, height, runs);
                out.write(runs.append('\n').toString());

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

                if (chunks < gate) {
                    try (NoiseChunk noiseChunk = new NoiseChunk(
                            state, Beardifier.EMPTY, settings, fluidPicker, Blender.empty(), volume)) {
                        DensityVolume columns = new DensityVolume(
                            16, 1, 16, pos.getMinBlockX(), 0, pos.getMinBlockZ());
                        DensityBuffer levels = DensityBuffer.createUnpooled(columns.size());
                        noiseChunk.cachingSamplers().get(system.preliminarySurfaceFunction())
                            .sampleVolume(levels, columns);
                        StringBuilder level = new StringBuilder("level ");
                        for (int index = 0; index < columns.size(); index++) {
                            hex(level, Float.floatToRawIntBits(levels.get(index)) & 0xffffffffL, 8);
                        }
                        out.write(level.append('\n').toString());

                        if (!veins.isEmpty()) {
                            DensityVolume expected = new DensityVolume(
                                16, expectedSizeY, 16, pos.getMinBlockX(), minY, pos.getMinBlockZ());
                            StringBuilder text = new StringBuilder("veins " + expectedSizeY);
                            for (int which = 0; which < 3; which++) {
                                DensityBuffer values = DensityBuffer.createUnpooled(expected.size());
                                noiseChunk.cachingSamplers().get(veins.get(which))
                                    .sampleVolume(values, expected);
                                long hash = 0xcbf29ce484222325L;
                                for (int index = 0; index < expected.size(); index++) {
                                    hash = fnv(hash, Float.floatToRawIntBits(values.get(index)), 4);
                                }
                                text.append(' ').append(Long.toHexString(hash));
                            }
                            DensitySampler.Bound gap = noiseChunk.cachingSamplers().get(veins.get(3));
                            long hash = 0xcbf29ce484222325L;
                            for (int z = 0; z < 16; z++) {
                                for (int x = 0; x < 16; x++) {
                                    for (int y = 0; y < expectedSizeY; y++) {
                                        hash = fnv(hash, Float.floatToRawIntBits(gap.sampleValue(
                                            pos.getMinBlockX() + x, minY + y, pos.getMinBlockZ() + z)), 4);
                                    }
                                }
                            }
                            text.append(' ').append(Long.toHexString(hash));
                            out.write(text.append('\n').toString());
                        }
                    }

                    StringBuilder depth = new StringBuilder("depth");
                    StringBuilder secondary = new StringBuilder("secondary ");
                    StringBuilder band = new StringBuilder("band");
                    int zoomStep = 13;
                    int zoomCount = dimensionHeight / zoomStep + 1;
                    StringBuilder zoom = new StringBuilder(
                        "zoom " + dimensionMinY + " " + zoomStep + " " + zoomCount);
                    StringBuilder cold = new StringBuilder("cold");
                    for (Holder<Biome> biome : asked) {
                        cold.append(' ').append(biomeNumbers.get(biome));
                    }
                    cold.append(' ');
                    BlockPos.MutableBlockPos at = new BlockPos.MutableBlockPos();
                    for (int z = 0; z < 16; z++) {
                        for (int x = 0; x < 16; x++) {
                            int blockX = pos.getMinBlockX() + x;
                            int blockZ = pos.getMinBlockZ() + z;
                            depth.append(' ').append((int) getSurfaceDepth.invoke(system, blockX, blockZ));
                            hex(secondary, Double.doubleToRawLongBits(
                                (double) getSurfaceSecondary.invoke(system, blockX, blockZ)), 16);
                            band.append(' ').append(Block.getId(
                                (BlockState) getBand.invoke(system, blockX, 60 + x + 3 * z, blockZ)));
                            for (int step = 0; step < zoomCount; step++) {
                                zoom.append(' ').append(biomeNumbers.get(manager.getBiome(
                                    at.set(blockX, dimensionMinY + step * zoomStep, blockZ))));
                            }
                        }
                    }
                    for (Holder<Biome> biome : asked) {
                        for (int z = 0; z < 16; z++) {
                            for (int x = 0; x < 16; x++) {
                                for (int y : new int[] {64, 100, 160, 250}) {
                                    at.set(pos.getMinBlockX() + x, y, pos.getMinBlockZ() + z);
                                    cold.append(biome.value().coldEnoughToSnow(at, seaLevel) ? '1' : '0');
                                }
                            }
                        }
                    }
                    out.write(depth.append('\n').toString());
                    out.write(secondary.append('\n').toString());
                    out.write(zoom.append('\n').toString());
                    out.write(cold.append('\n').toString());
                    out.write(band.append('\n').toString());
                }
                chunks++;
            }
            out.write("end " + chunks + "\n");
        }
    }
}
