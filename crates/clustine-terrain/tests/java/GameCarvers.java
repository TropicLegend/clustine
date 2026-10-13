// GameCarvers.java: the blocks of chunks after the game's "fill from noise", "build
// surface" and "generate carvers" steps and nothing after them, for
// `tests/game_carvers.rs` to hold this crate to.
//
// It asks the classes of the server jar after `Bootstrap.bootStrap()`, as
// `GameSurface.java` does, and starts no server and makes no world. For each chunk of
// a list, in one thread and in the list's order:
//
// - **The biomes, the fill and the surface** exactly as `GameSurface.java` makes them
//   (its head says how): the game's biome step for the chunk and its eight
//   neighbours, the generator's own private `doFill` and `buildSurface` by reflection.
// - **The carvers**: the generator's own private `generateCarvers`, called by
//   reflection with the same chunk, the blender that blends nothing, the same noise
//   chunk (so the aquifer is the one that filled the chunk), the same `BiomeManager`
//   and the dimension's material rule, **and no `WorldGenRegion`**. The method takes
//   none where it is given `null`: it then asks the generator's biome source for the
//   biome of each source chunk, which is what it asks through a region as well (a
//   region's chunk only remembers the answer, `ChunkAccess.carverBiome`). So the
//   seventeen by seventeen source chunks, the seeds, the carvers, the mask, the
//   aquifer's answers, the top material and the marks are all the game's. Features
//   and everything later are not called.
// - **The top material by itself**, between the surface and the carvers: the game's
//   `MaterialSystem.topMaterial`, which the carvers ask for what dirt under carved
//   grass becomes, is asked at three blocks of every column of the chunk (the
//   highest block of ground, the block twelve below it, and the highest block of
//   ground not above six over the sea level), each with a fluid above and without,
//   with what the carving step gives it: the rule, the chunk, the noise chunk's
//   samplers, and the biome at a block through the `BiomeManager` over the
//   generator's biome source. The carvers ask it at few blocks and nearly always
//   under grass on level ground, where the slope does not change its answer; here
//   it is asked on slopes, under the sea and below the surface in every biome.
//
// **What is built by hand here**, each because no server loaded it or because the
// game keeps it to itself:
//
// 1. **The tag `#minecraft:uncarvable`.** A block state asked for a tag throws where
//    no tags are bound, and tags are bound when a server loads its data. The entries
//    of `data/minecraft/tags/block/uncarvable.json` are read from the jar and bound
//    to their blocks (`Holder.Reference.bindTags`, by reflection); every other block
//    is bound to no tag at all. The carving asks no other tag. That the binding is
//    at work shows in the test: it counts the bedrock that the masks name and the
//    game left.
// 2. **A biome source that writes down what it is asked**: the generator is given a
//    `BiomeSource` that passes every call on to the dimension's own and keeps the
//    cells the carving step asked with the answers. A biome that is an exact tie
//    between two entries depends on what the thread searched before, so the test
//    gives this crate the answers the game got: the 289 source chunks' biomes
//    (`sources`), the cells the step's top material asked (`asked`), and for the
//    top material by itself each answer of the source that was not the biome the
//    chunks stored (`topasked`).
// 3. **The carving mask**, which the game keeps in a local of `generateCarvers` and
//    throws away. After the game's own step, the loop of that method is done once
//    more here over the game's own carvers (`WorldCarver.isStartChunk`, `carve`) and
//    a `CarvingMask` of the game's class, with the biomes written down in the step,
//    and the bits of that mask are written out. The blocks are the game's own
//    step's; only the mask comes from the loop written here, and
//    `tests/game_carvers.rs` holds both to this crate, which makes its blocks from
//    its mask.
// 4. The `BiomeManager` over the nine chunks, as in `GameSurface.java`.
//
// Needs a JDK 25 and the jar's classes on the class path; `tests/game_carvers.rs` says
// how it was run. What it writes is values computed by Mojang's code: Mojang's data,
// not under the licence of this file (see NOTICE.md at the root of the repository).
//
//   java GameCarvers <seed> <overworld|nether|end> <list of chunks> <output>
//
// The list has a chunk a line, `x z`. The output, a line each:
//
//   game-carvers 1
//   seed <seed>
//   dimension <name>
//   noise <lowest y> <height> <sea level> <default block> <default fluid>
//   heights <lowest y of a chunk> <height of a chunk>
//   context <lowest y the carvers read> <how many blocks high>
//   uncarvable <state ids>      every state of the blocks of the tag
//   biome <number> <name>       every biome of the registry, sorted by name
//   chunk <x> <z>
//   biomes <36 columns>         as `GameSurface.java` writes them
//   surfaced <hash>             FNV-1a, 64 bits, over the state ids after the surface,
//                               each as two bytes from the lowest, in the order of
//                               `blocks`
//   top <1536 answers>          the top material by itself: for each column, index =
//                               x + z * 16, at the highest block that is neither air
//                               nor a fluid, at the block twelve below it, and at the
//                               highest such block not above six over the sea level,
//                               each without a fluid above and with one: the state id,
//                               `-` where the rule gives none, `x` where there is no
//                               such block in the terrain
//   topasked <answer>:<x>,<y>,<z>,<biome> ...
//                               for an answer of `top`, by its place in that line: a
//                               cell it asked the biome source for whose answer was
//                               not the biome the chunks stored there
//   sources <289 biomes>        the biome each source chunk's carvers were taken from,
//                               from eight chunks before to eight after, x counting most
//   asked <x>,<y>,<z>,<biome> ...
//                               every other cell the step asked the biome source for
//   blocks <runs>               after the carvers: runs of `<state id>*<count>` in the
//                               order index = y + (x + z * 16) * height, from the lowest y
//   marks <x>,<y>,<z> ...       the positions marked for post-processing, as the chunk
//                               kept them (a position can stand twice)
//   mask <lowest y> <highest y> <first>+<count> ...
//                               the mask's heights and its set bits as runs of the
//                               game's index, (y - lowest) + (z + x * 16) * height
//   end <chunks>

import java.io.BufferedWriter;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.BitSet;
import java.util.Collection;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.stream.Stream;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mojang.serialization.MapCodec;
import it.unimi.dsi.fastutil.shorts.ShortList;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.IdMap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagKey;
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
import net.minecraft.world.level.chunk.CarvingMask;
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
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.WorldGenerationContext;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.carver.WorldCarver;
import net.minecraft.world.level.levelgen.densityfunction.DensityBufferPool;
import net.minecraft.world.level.levelgen.densityfunction.DensityVolume;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public final class GameCarvers {
    /// How many neighbours' chunks of biomes are kept.
    private static final int KEPT = 2048;

    /// A cell the carving step asked the biome source for, and the answer.
    private record Asked(int x, int y, int z, Holder<Biome> biome) {}

    /// A biome source that passes every call on to another and, while `listening`,
    /// keeps what it was asked.
    private static final class Listening extends BiomeSource {
        private final BiomeSource inner;
        final List<Asked> asked = new ArrayList<>();
        boolean listening;

        Listening(BiomeSource inner) {
            this.inner = inner;
        }

        @Override
        protected MapCodec<? extends BiomeSource> codec() {
            throw new UnsupportedOperationException("nothing writes this source");
        }

        @Override
        protected Stream<Holder<Biome>> collectPossibleBiomes() {
            return this.inner.possibleBiomes().stream();
        }

        @Override
        public BiomeResolver createResolver(Climate.Sampler sampler) {
            BiomeResolver resolver = this.inner.createResolver(sampler);
            return (x, y, z) -> {
                Holder<Biome> biome = resolver.getNoiseBiome(x, y, z);
                if (this.listening) {
                    this.asked.add(new Asked(x, y, z, biome));
                }
                return biome;
            };
        }
    }

    private static long fnv(long hash, int value, int bytes) {
        for (int shift = 0; shift < bytes * 8; shift += 8) {
            hash ^= (value >>> shift) & 0xff;
            hash *= 0x100000001b3L;
        }
        return hash;
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

    /// Binds the blocks named in the jar's `uncarvable.json` to that tag and every
    /// other block to no tag. Gives the tag's blocks.
    private static List<Block> bindUncarvable() throws Exception {
        List<Block> tagged = new ArrayList<>();
        try (InputStream in = GameCarvers.class.getResourceAsStream(
                "/data/minecraft/tags/block/uncarvable.json")) {
            if (in == null) {
                throw new IllegalStateException("the jar has no tag of what carvers leave");
            }
            JsonObject tag = JsonParser.parseReader(
                new InputStreamReader(in, StandardCharsets.UTF_8)).getAsJsonObject();
            for (JsonElement value : tag.getAsJsonArray("values")) {
                String name = value.getAsString();
                if (name.startsWith("#")) {
                    throw new IllegalStateException("the tag names another tag, " + name);
                }
                tagged.add(BuiltInRegistries.BLOCK.getOptional(Identifier.parse(name))
                    .orElseThrow(() -> new IllegalStateException(name + " is no block")));
            }
        }
        Method bindTags = Holder.Reference.class.getDeclaredMethod("bindTags", Collection.class);
        bindTags.setAccessible(true);
        for (Block block : BuiltInRegistries.BLOCK) {
            Set<TagKey<Block>> tags = new HashSet<>();
            if (tagged.contains(block)) {
                tags.add(BlockTags.UNCARVABLE);
            }
            bindTags.invoke(block.builtInRegistryHolder(), tags);
        }
        return tagged;
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 4) {
            throw new IllegalArgumentException(
                "usage: java GameCarvers <seed> <overworld|nether|end> <chunks> <output>");
        }
        long seed = Long.parseLong(args[0]);
        String dimension = args[1];

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
        Listening listening = new Listening(source);
        NoiseBasedChunkGenerator generator = new NoiseBasedChunkGenerator(listening, held);
        LevelHeightAccessor heights = LevelHeightAccessor.create(dimensionMinY, dimensionHeight);
        MaterialRule rule = settings.materialRule().value();
        MaterialSystem system = state.surfaceSystem();

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
        Method generateCarvers = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "generateCarvers", ChunkAccess.class, Blender.class, NoiseChunk.class,
            RandomState.class, BiomeManager.class, WorldGenRegion.class, MaterialRule.class);
        generateCarvers.setAccessible(true);
        Field maskBits = CarvingMask.class.getDeclaredField("mask");
        maskBits.setAccessible(true);

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

        // The biomes of chunks, each made by the game's biome step in a chunk of its
        // own; the least recently read is let go.
        LinkedHashMap<Long, ProtoChunk> kept = new LinkedHashMap<>(KEPT, 0.75f, true) {
            @Override
            protected boolean removeEldestEntry(Map.Entry<Long, ProtoChunk> eldest) {
                return size() > KEPT;
            }
        };

        List<String> lines = new ArrayList<>();
        for (String line : Files.readAllLines(Path.of(args[2]), StandardCharsets.UTF_8)) {
            if (!line.isBlank()) {
                lines.add(line.trim());
            }
        }

        try (BufferedWriter out = Files.newBufferedWriter(Path.of(args[3]), StandardCharsets.UTF_8)) {
            out.write("game-carvers 1\n");
            out.write("seed " + seed + "\n");
            out.write("dimension " + dimension + "\n");
            out.write("noise " + minY + " " + height + " " + seaLevel + " "
                + Block.getId(settings.defaultBlock()) + " " + Block.getId(settings.defaultFluid()) + "\n");
            out.write("heights " + dimensionMinY + " " + dimensionHeight + "\n");
            WorldGenerationContext context = new WorldGenerationContext(generator, heights);
            out.write("context " + context.getMinGenY() + " " + context.getGenDepth() + "\n");
            StringBuilder uncarvable = new StringBuilder("uncarvable");
            for (Block block : bindUncarvable()) {
                for (BlockState one : block.getStateDefinition().getPossibleStates()) {
                    uncarvable.append(' ').append(Block.getId(one));
                }
            }
            out.write(uncarvable.append('\n').toString());
            for (int number = 0; number < biomes.size(); number++) {
                out.write("biome " + number + " "
                    + biomes.get(number).unwrapKey().orElseThrow().identifier() + "\n");
            }

            int chunks = 0;
            for (String line : lines) {
                String[] fields = line.split(" ");
                ChunkPos pos = new ChunkPos(Integer.parseInt(fields[0]), Integer.parseInt(fields[1]));
                StringBuilder text = new StringBuilder();
                text.append("chunk ").append(pos.x()).append(' ').append(pos.z()).append('\n');

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
                                SamplerContext sampling =
                                    SamplerContext.builder().enableCaches().useBufferArena(pool).build();
                                Climate.Sampler sampler = state.createClimateSampler(sampling);
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

                text.append("biomes");
                for (int x = -1; x <= 4; x++) {
                    for (int z = -1; z <= 4; z++) {
                        text.append(' ');
                        int last = -1;
                        int count = 0;
                        for (int y = 0; y < cellsHigh; y++) {
                            int number = biomeNumbers.get(around.getNoiseBiome(
                                pos.x() * 4 + x, lowestCell + y, pos.z() * 4 + z));
                            if (number != last) {
                                if (count > 0) {
                                    text.append(last).append('*').append(count).append(',');
                                }
                                last = number;
                                count = 0;
                            }
                            count++;
                        }
                        text.append(last).append('*').append(count);
                    }
                }
                text.append('\n');

                DensityVolume volume = new DensityVolume(
                    16, height, 16, pos.getMinBlockX(), minY, pos.getMinBlockZ());
                listening.asked.clear();
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
                    buildSurface.invoke(generator, chunk, noiseChunk, state, manager, possible, rule);
                    text.append("surfaced ")
                        .append(Long.toHexString(blocks(chunk, minY, height, null))).append('\n');

                    // The top material by itself, as the carving step would ask it.
                    WorldGenerationContext asking =
                        new WorldGenerationContext(generator, chunk.getHeightAccessorForGeneration());
                    StringBuilder otherwise = new StringBuilder("topasked");
                    int answers = -1;
                    listening.listening = true;
                    try {
                        BiomeManager overTheSource =
                            manager.withDifferentSource(listening.createUncachedResolver(state));
                        text.append("top");
                        for (int z = 0; z < 16; z++) {
                            for (int x = 0; x < 16; x++) {
                                // The highest ground of the column, and the highest
                                // not above six over the sea level; below the
                                // terrain where there is none.
                                int[] ground = {minY + height - 1, Math.min(seaLevel + 6, minY + height - 1)};
                                for (int which = 0; which < 2; which++) {
                                    while (ground[which] >= minY) {
                                        BlockState at = chunk.getSection(chunk.getSectionIndex(ground[which]))
                                            .getBlockState(x, ground[which] & 15, z);
                                        if (!at.isAir() && at.getFluidState().isEmpty()) {
                                            break;
                                        }
                                        ground[which]--;
                                    }
                                }
                                int[] places = {
                                    ground[0], ground[0] < minY ? ground[0] : ground[0] - 12, ground[1]};
                                for (int y : places) {
                                    for (boolean underFluid : new boolean[] {false, true}) {
                                        answers++;
                                        if (y < minY) {
                                            text.append(" x");
                                            continue;
                                        }
                                        int before = listening.asked.size();
                                        Optional<BlockState> given = system.topMaterial(
                                            rule, state, asking, overTheSource::getBiome, chunk,
                                            noiseChunk.cachingSamplers(),
                                            new BlockPos(pos.getMinBlockX() + x, y, pos.getMinBlockZ() + z),
                                            underFluid);
                                        // An answer of the source that is not what the
                                        // chunks stored is a tie the thread decided
                                        // otherwise this time.
                                        for (Asked asked : listening.asked.subList(
                                                before, listening.asked.size())) {
                                            if (asked.biome()
                                                    != around.getNoiseBiome(asked.x(), asked.y(), asked.z())) {
                                                otherwise.append(' ').append(answers).append(':')
                                                    .append(asked.x()).append(',').append(asked.y())
                                                    .append(',').append(asked.z()).append(',')
                                                    .append(biomeNumbers.get(asked.biome()));
                                            }
                                        }
                                        text.append(' ');
                                        if (given.isPresent()) {
                                            text.append(Block.getId(given.get()));
                                        } else {
                                            text.append('-');
                                        }
                                    }
                                }
                            }
                        }
                        text.append('\n');
                    } finally {
                        listening.listening = false;
                    }
                    text.append(otherwise).append('\n');
                    listening.asked.clear();

                    listening.listening = true;
                    try {
                        generateCarvers.invoke(
                            generator, chunk, Blender.empty(), noiseChunk, state, manager, null, rule);
                    } finally {
                        listening.listening = false;
                    }
                }

                // What the step asked the biome source: first the source chunks, in
                // the step's order, then whatever the top material asked.
                if (listening.asked.size() < 289) {
                    throw new IllegalStateException("the step asked for fewer biomes than source chunks");
                }
                Map<Long, Holder<Biome>> ofSource = new HashMap<>();
                text.append("sources");
                for (int index = 0; index < 289; index++) {
                    Asked asked = listening.asked.get(index);
                    int dx = index / 17 - 8;
                    int dz = index % 17 - 8;
                    if (asked.x() != (pos.x() + dx) << 2 || asked.y() != 0
                            || asked.z() != (pos.z() + dz) << 2) {
                        throw new IllegalStateException(
                            "the step did not ask for the source chunks' biomes first and in order");
                    }
                    ofSource.put(((long) (pos.x() + dx) << 32) ^ ((pos.z() + dz) & 0xffffffffL),
                        asked.biome());
                    text.append(' ').append(biomeNumbers.get(asked.biome()));
                }
                text.append('\n');
                text.append("asked");
                for (Asked asked : listening.asked.subList(289, listening.asked.size())) {
                    text.append(' ').append(asked.x()).append(',').append(asked.y()).append(',')
                        .append(asked.z()).append(',').append(biomeNumbers.get(asked.biome()));
                }
                text.append('\n');

                StringBuilder runs = new StringBuilder("blocks");
                blocks(chunk, minY, height, runs);
                text.append(runs).append('\n');

                text.append("marks");
                ShortList[] lists = chunk.getPostProcessing();
                for (int section = 0; section < lists.length; section++) {
                    if (lists[section] == null) {
                        continue;
                    }
                    for (short packed : lists[section]) {
                        int y = (chunk.getMinSectionY() + section) * 16 + (packed >>> 4 & 15);
                        text.append(' ').append(packed & 15).append(',').append(y).append(',')
                            .append(packed >>> 8 & 15);
                    }
                }
                text.append('\n');

                // The mask, by the loop of `generateCarvers` done once more over the
                // game's carvers, with the biomes the step was given.
                WorldGenerationContext carving =
                    new WorldGenerationContext(generator, chunk.getHeightAccessorForGeneration());
                int maskMinY = carving.getMinGenY() + 1;
                int maskMaxY = carving.getMinGenY() + carving.getGenDepth() - 1 - 7;
                CarvingMask mask = new CarvingMask(maskMinY, maskMaxY);
                WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
                for (int dx = -8; dx <= 8; dx++) {
                    for (int dz = -8; dz <= 8; dz++) {
                        ChunkPos from = new ChunkPos(pos.x() + dx, pos.z() + dz);
                        Holder<Biome> biome =
                            ofSource.get(((long) from.x() << 32) ^ (from.z() & 0xffffffffL));
                        int index = 0;
                        for (Holder<WorldCarver> carver
                                : generator.getBiomeGenerationSettings(biome).getCarvers()) {
                            random.setLargeFeatureSeed(state.seed() + index, from.x(), from.z());
                            if (carver.value().isStartChunk(random)) {
                                carver.value().carve(carving, random, pos, from, mask);
                            }
                            index++;
                        }
                    }
                }
                BitSet bits = (BitSet) maskBits.get(mask);
                text.append("mask ").append(maskMinY).append(' ').append(maskMaxY);
                for (int first = bits.nextSetBit(0); first >= 0;) {
                    int after = bits.nextClearBit(first);
                    text.append(' ').append(first).append('+').append(after - first);
                    first = bits.nextSetBit(after);
                }
                text.append('\n');

                out.write(text.toString());
                chunks++;
            }
            out.write("end " + chunks + "\n");
        }
    }
}
