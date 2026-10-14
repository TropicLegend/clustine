// GameStructures.java: the structure starts and references of chunks as the game's
// "structure starts" and "structure references" steps make them, and the blocks of
// chunks after "fill from noise", "build surface" and "generate carvers" **with the
// structures' bending of the ground**, for `crates/clustine-structures/tests` to hold
// that crate and `clustine-terrain` to.
//
// It starts no server and makes no world. Unlike the four programs beside it, it has
// the game **load its own data pack** as a server does before it makes a world
// (`WorldLoader.load` over the pack inside the jar): the registries with every tag
// bound, the three dimensions' generators with their biome sources, and a resource
// manager that serves the jar's structure templates. So nothing of the data is read
// or bound by hand here.
//
// For each chunk of a list, in one thread and in the list's order:
//
// - **Starts**: the generator's own `ChunkGenerator.createStructures`, with the
//   game's `ChunkGeneratorStructureState` (`ChunkGenerator.createState`, so the ring
//   positions of strongholds are the game's), a `StructureManager` and the game's
//   `StructureTemplateManager`, on a new `ProtoChunk`. Every start the chunk then has
//   is written with its pieces as the game would save them (`StructurePiece.createTag`).
// - **References**: the generator's own `ChunkGenerator.createReferences`, for which
//   the starts of the 17 by 17 chunks around are made first, the same way.
// - **Terrain** (for the lines that ask for it): the biomes of the nine chunks as
//   `GameCarvers.java` makes them, then the generator's private `createNoiseChunk`
//   (which asks `Beardifier.forStructuresInChunk` of the structure manager), `doFill`,
//   `buildSurface` and `generateCarvers` by reflection, in the order of
//   `buildTerrain`. The beardifier's pieces, junctions and box are written beside the
//   blocks.
//
// **What is built by hand here**, each because there is no server and no world:
//
// 1. **The level the `StructureManager` and `createReferences` ask for chunks**: a
//    `java.lang.reflect.Proxy` of `WorldGenLevel` that answers `getChunk` from this
//    program's map of chunks (each a `ProtoChunk` that the game's `createStructures`
//    filled) and `registryAccess`, and throws at any other call. The structure
//    manager is the game's class over that proxy, with world options that say
//    "generate structures" and no `StructureCheck` (nothing here asks it).
// 2. **A directory for the template manager**: the game's `StructureTemplateManager`
//    wants a `LevelStorageAccess` to look for templates a player saved. It gets one
//    over an empty directory under the system's temporary directory, which holds a
//    lock file and nothing else; every template comes from the jar.
// 3. **The dimension's heights**: -64 and 384 for the overworld, 0 and 256 for the
//    others, as in the programs beside this one (the dimension types are loaded, but
//    a `ProtoChunk` wants a `LevelHeightAccessor`).
// 4. For the terrain: the `BiomeManager` over the nine chunks, the listening biome
//    source's role and the acquire and release of sections, as in `GameCarvers.java`.
//    The generator is the game's own of the dimension, so its biome source is not
//    wrapped; what the carving step asked is not written, and the test makes the
//    chunk with this crate's own biomes for the source chunks.
//
// Needs a JDK 25 and the jar's classes on the class path. What it writes is values
// computed by Mojang's code: Mojang's data, not under the licence of this file (see
// NOTICE.md at the root of the repository).
//
//   java GameStructures <seed> <overworld|nether|end> <list> <output>
//
// The list, a line each:
//
//   area <x0> <z0> <x1> <z1>    starts and references of every chunk of the rectangle
//                               (both ends included), x counting most
//   chunk <x> <z>               starts and references of one chunk
//   terrain <x> <z>             the same, and the terrain with structures on
//
// The output, a line each:
//
//   game-structures 1
//   seed <seed>
//   dimension <name>
//   kernel <hash>               FNV-1a, 64 bits, over the bits of the beardifier's
//                               13,824 numbers, each as four bytes from the lowest
//   structure <number> <name> <step> <terrain adaptation>
//                               every structure in the registry's order
//   set <name> <placement>      every structure set the dimension's state holds, in
//                               its order
//   ring <set> <x>,<z> ...      the chunks of a set placed in concentric rings
//   asked <line of the list>    before the chunks of each line of the list
//   chunk <x> <z>               only chunks with a start or a reference, and every
//                               chunk asked for with `chunk` or `terrain`
//   start <structure> <references> <box>
//   piece <type> <depth> <orientation> <rotation> <mirror> <box> <tag>
//                               in the start's order; the box is six numbers; the
//                               tag is the piece as the game saves it, as text
//   refs <structure> <x>,<z> ...
//                               the starts the chunk refers to, in the order the
//                               chunk holds them
//   beard <box or -> | <piece box>,<adjustment>,<delta> ... | <x>,<y>,<z>,<dy> ...
//                               (terrain only) the beardifier as the fill is given it
//   biomes, blocks, marks       (terrain only) as `GameCarvers.java` writes them
//   end <chunks>

import java.io.BufferedWriter;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

import it.unimi.dsi.fastutil.longs.LongSet;
import it.unimi.dsi.fastutil.shorts.ShortList;
import net.minecraft.SharedConstants;
import net.minecraft.commands.Commands;
import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.WorldLoader;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.server.packs.repository.PackRepository;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.ResourceManager;
import net.minecraft.server.permissions.PermissionSet;
import net.minecraft.util.Util;
import net.minecraft.util.datafix.DataFixers;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeResolver;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.chunk.ChunkGeneratorStructureState;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.Beardifier;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.WorldDimensions;
import net.minecraft.world.level.levelgen.WorldOptions;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.densityfunction.DensityBufferPool;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.material.rule.MaterialRule;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.Structure;
import net.minecraft.world.level.levelgen.structure.StructurePiece;
import net.minecraft.world.level.levelgen.structure.StructureSet;
import net.minecraft.world.level.levelgen.structure.StructureStart;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.placement.ConcentricRingsStructurePlacement;
import net.minecraft.world.level.levelgen.structure.pools.JigsawJunction;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplateManager;
import net.minecraft.world.level.storage.LevelStorageSource;

public final class GameStructures {
    /// What the game loaded: the registries, the dimensions and the resources.
    private record Loaded(RegistryAccess.Frozen registries, RegistryAccess.Frozen dimensions,
        ResourceManager resources) {}

    private static long pack(int x, int z) {
        return ((long) x << 32) ^ (z & 0xffffffffL);
    }

    private static String box(BoundingBox box) {
        return box.minX() + " " + box.minY() + " " + box.minZ() + " "
            + box.maxX() + " " + box.maxY() + " " + box.maxZ();
    }

    /// The state ids of a chunk's terrain as runs, as `GameCarvers.java` writes them.
    private static void blocks(ProtoChunk chunk, int minY, int height, StringBuilder runs) {
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
        runs.append(' ').append(last).append('*').append(count);
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 4) {
            throw new IllegalArgumentException(
                "usage: java GameStructures <seed> <overworld|nether|end> <list> <output>");
        }
        long seed = Long.parseLong(args[0]);
        String dimension = args[1];

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        // The game's own loading of its data pack, as a server does before it has a
        // world: packs, registries, tags, dimensions.
        PackRepository packs = ServerPacksSource.createVanillaTrustedRepository();
        WorldLoader.PackConfig packConfig =
            new WorldLoader.PackConfig(packs, WorldDataConfiguration.DEFAULT, false, true);
        WorldLoader.InitConfig init = new WorldLoader.InitConfig(
            packConfig, Commands.CommandSelection.DEDICATED, PermissionSet.NO_PERMISSIONS);
        Loaded loaded = WorldLoader.<RegistryAccess.Frozen, Loaded>load(
            init,
            context -> new WorldLoader.DataLoadOutput<>(
                context.datapackDimensions(), context.datapackDimensions()),
            (resources, serverResources, layers, dimensions) ->
                new Loaded(layers.compositeAccess(), dimensions, resources),
            Util.backgroundExecutor(), Runnable::run).get();
        RegistryAccess.Frozen registries = loaded.registries();

        ResourceKey<LevelStem> stemKey;
        ResourceKey<Level> levelKey;
        int dimensionMinY;
        int dimensionHeight;
        switch (dimension) {
            case "overworld" -> {
                stemKey = LevelStem.OVERWORLD;
                levelKey = Level.OVERWORLD;
                dimensionMinY = -64;
                dimensionHeight = 384;
            }
            case "nether" -> {
                stemKey = LevelStem.NETHER;
                levelKey = Level.NETHER;
                dimensionMinY = 0;
                dimensionHeight = 256;
            }
            case "end" -> {
                stemKey = LevelStem.END;
                levelKey = Level.END;
                dimensionMinY = 0;
                dimensionHeight = 256;
            }
            default -> throw new IllegalArgumentException("no dimension " + dimension);
        }
        // The dimensions of the world preset "normal", which is what a new world has.
        WorldDimensions dimensions = registries.lookupOrThrow(Registries.WORLD_PRESET)
            .getOrThrow(WorldPresets.NORMAL).value().createWorldDimensions();
        NoiseBasedChunkGenerator generator =
            (NoiseBasedChunkGenerator) dimensions.get(stemKey).orElseThrow().generator();
        BiomeSource source = generator.getBiomeSource();
        NoiseGeneratorSettings settings = generator.generatorSettings().value();
        RandomState state =
            RandomState.create(registries.lookupOrThrow(Registries.NOISE), seed, settings);
        ChunkGeneratorStructureState structureState = generator.createState(
            registries.lookupOrThrow(Registries.STRUCTURE_SET), state, seed);
        structureState.ensureStructuresGenerated();
        LevelHeightAccessor heights = LevelHeightAccessor.create(dimensionMinY, dimensionHeight);
        PalettedContainerFactory containers = PalettedContainerFactory.create(registries);

        Path directory = Files.createTempDirectory("game-structures");
        LevelStorageSource.LevelStorageAccess storage =
            LevelStorageSource.createDefault(directory).createAccess("none");
        StructureTemplateManager templates = new StructureTemplateManager(
            loaded.resources(), storage, DataFixers.getDataFixer(),
            registries.lookupOrThrow(Registries.BLOCK));
        StructurePieceSerializationContext saving =
            new StructurePieceSerializationContext(loaded.resources(), registries, templates);

        Registry<Structure> structures = registries.lookupOrThrow(Registries.STRUCTURE);
        Registry<Biome> biomeRegistry = registries.lookupOrThrow(Registries.BIOME);

        // Every chunk whose starts were made, by the game's own step.
        Map<Long, ProtoChunk> made = new HashMap<>();
        StructureManager[] managerHolder = new StructureManager[1];
        InvocationHandler handler = (proxy, method, arguments) -> {
            switch (method.getName()) {
                case "getChunk" -> {
                    int x = (Integer) arguments[0];
                    int z = (Integer) arguments[1];
                    ProtoChunk chunk = made.get(pack(x, z));
                    if (chunk == null) {
                        throw new IllegalStateException("chunk " + x + " " + z + " was not made");
                    }
                    return chunk;
                }
                case "registryAccess" -> {
                    return registries;
                }
                case "hashCode" -> {
                    return 0;
                }
                case "toString" -> {
                    return "the level of GameStructures";
                }
                default -> throw new UnsupportedOperationException(
                    "the level was asked " + method.getName());
            }
        };
        WorldGenLevel level = (WorldGenLevel) Proxy.newProxyInstance(
            GameStructures.class.getClassLoader(), new Class<?>[] {WorldGenLevel.class}, handler);
        StructureManager manager =
            new StructureManager(level, new WorldOptions(seed, true, false), null);
        managerHolder[0] = manager;

        Method createNoiseChunk = NoiseBasedChunkGenerator.class.getDeclaredMethod(
            "createNoiseChunk", ChunkAccess.class, StructureManager.class, Blender.class,
            RandomState.class, NoiseSettings.class);
        createNoiseChunk.setAccessible(true);
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
        Field beardPieces = Beardifier.class.getDeclaredField("pieces");
        beardPieces.setAccessible(true);
        Field beardJunctions = Beardifier.class.getDeclaredField("junctions");
        beardJunctions.setAccessible(true);
        Field beardBox = Beardifier.class.getDeclaredField("affectedBox");
        beardBox.setAccessible(true);

        // The biome numbers of the terrain's `biomes` line: by name, as the programs
        // beside this one count them.
        List<Holder.Reference<Biome>> biomes = new ArrayList<>();
        biomeRegistry.listElements().forEach(biomes::add);
        biomes.sort((a, b) -> a.key().identifier().toString().compareTo(b.key().identifier().toString()));
        Map<Holder<Biome>, Integer> biomeNumbers = new HashMap<>();
        for (int number = 0; number < biomes.size(); number++) {
            biomeNumbers.put(biomes.get(number), number);
        }

        NoiseSettings noise = settings.noiseSettings();
        int minY = noise.minY();
        int height = noise.height();
        MaterialRule rule = settings.materialRule().value();

        try (BufferedWriter out = Files.newBufferedWriter(Path.of(args[3]), StandardCharsets.UTF_8)) {
            out.write("game-structures 1\n");
            out.write("seed " + seed + "\n");
            out.write("dimension " + dimension + "\n");
            // The beardifier's kernel, which the game makes once with `Math.pow`.
            Field kernelField = Beardifier.class.getDeclaredField("BEARD_KERNEL");
            kernelField.setAccessible(true);
            long kernelHash = 0xcbf29ce484222325L;
            for (float value : (float[]) kernelField.get(null)) {
                int bits = Float.floatToRawIntBits(value);
                for (int shift = 0; shift < 32; shift += 8) {
                    kernelHash ^= (bits >>> shift) & 0xff;
                    kernelHash *= 0x100000001b3L;
                }
            }
            out.write("kernel " + Long.toHexString(kernelHash) + "\n");
            for (Structure structure : structures) {
                out.write("structure " + structures.getId(structure) + " "
                    + structures.getKey(structure) + " " + structure.step().getSerializedName() + " "
                    + structure.terrainAdaptation().getSerializedName() + "\n");
            }
            for (Holder<StructureSet> set : structureState.possibleStructureSets()) {
                String name = set.unwrapKey().orElseThrow().identifier().toString();
                out.write("set " + name + " "
                    + set.value().placement().getClass().getSimpleName() + "\n");
                if (set.value().placement() instanceof ConcentricRingsStructurePlacement rings) {
                    StringBuilder text = new StringBuilder("ring ").append(name);
                    for (ChunkPos pos : structureState.getRingPositionsFor(rings)) {
                        text.append(' ').append(pos.x()).append(',').append(pos.z());
                    }
                    out.write(text.append('\n').toString());
                }
            }

            int chunks = 0;
            for (String line : Files.readAllLines(Path.of(args[2]), StandardCharsets.UTF_8)) {
                if (line.isBlank()) {
                    continue;
                }
                String[] fields = line.trim().split(" ");
                out.write("asked " + line.trim() + "\n");
                // Chunks of earlier lines are let go: a start is made the same
                // again when another line reaches it.
                if (made.size() > 60_000) {
                    made.clear();
                }
                boolean terrain = fields[0].equals("terrain");
                boolean always = !fields[0].equals("area");
                int x0 = Integer.parseInt(fields[1]);
                int z0 = Integer.parseInt(fields[2]);
                int x1 = always ? x0 : Integer.parseInt(fields[3]);
                int z1 = always ? z0 : Integer.parseInt(fields[4]);
                for (int cx = x0; cx <= x1; cx++) {
                    // Chunks behind the rectangle's reach are let go, so that a
                    // large area does not keep every chunk.
                    if (!always && made.size() > 200_000) {
                        final int behind = cx - 9;
                        made.keySet().removeIf(key -> (int) (key >> 32) < behind);
                    }
                    for (int cz = z0; cz <= z1; cz++) {
                        ChunkPos pos = new ChunkPos(cx, cz);
                        for (int dx = -8; dx <= 8; dx++) {
                            for (int dz = -8; dz <= 8; dz++) {
                                ChunkPos at = new ChunkPos(cx + dx, cz + dz);
                                long key = pack(at.x(), at.z());
                                if (!made.containsKey(key)) {
                                    ProtoChunk chunk = new ProtoChunk(
                                        at, UpgradeData.EMPTY, heights, containers, null);
                                    generator.createStructures(
                                        registries, structureState, manager, chunk, templates, levelKey);
                                    made.put(key, chunk);
                                }
                            }
                        }
                        ProtoChunk chunk = made.get(pack(cx, cz));
                        if (chunk.getAllReferences().isEmpty()) {
                            generator.createReferences(level, manager, chunk);
                        }
                        Map<Structure, StructureStart> starts = chunk.getAllStarts();
                        Map<Structure, LongSet> references = chunk.getAllReferences();
                        if (!always && starts.isEmpty() && references.isEmpty()) {
                            continue;
                        }
                        StringBuilder text = new StringBuilder();
                        text.append("chunk ").append(cx).append(' ').append(cz).append('\n');
                        for (Map.Entry<Structure, StructureStart> entry : starts.entrySet()) {
                            StructureStart start = entry.getValue();
                            if (!start.isValid()) {
                                continue;
                            }
                            text.append("start ").append(structures.getKey(entry.getKey())).append(' ')
                                .append(start.getReferences()).append(' ')
                                .append(box(start.getBoundingBox())).append('\n');
                            for (StructurePiece piece : start.getPieces()) {
                                text.append("piece ")
                                    .append(BuiltInRegistries.STRUCTURE_PIECE.getKey(piece.getType()))
                                    .append(' ').append(piece.getGenDepth()).append(' ')
                                    .append(piece.getOrientation() == null
                                        ? "-" : piece.getOrientation().getSerializedName())
                                    .append(' ').append(piece.getRotation() == null
                                        ? "-" : piece.getRotation().getSerializedName())
                                    .append(' ').append(piece.getMirror() == null
                                        ? "-" : piece.getMirror().getSerializedName())
                                    .append(' ').append(box(piece.getBoundingBox()))
                                    .append(' ').append(piece.createTag(saving)).append('\n');
                            }
                        }
                        for (Map.Entry<Structure, LongSet> entry : references.entrySet()) {
                            if (entry.getValue().isEmpty()) {
                                continue;
                            }
                            text.append("refs ").append(structures.getKey(entry.getKey()));
                            for (long packed : entry.getValue()) {
                                ChunkPos from = ChunkPos.unpack(packed);
                                text.append(' ').append(from.x()).append(',').append(from.z());
                            }
                            text.append('\n');
                        }
                        if (terrain) {
                            terrain(text, pos, heights, containers, state, source, generator, manager,
                                seed, dimensionMinY, dimensionHeight, biomeNumbers, createNoiseChunk,
                                doFill, buildSurface, generateCarvers, beardPieces,
                                beardJunctions, beardBox, noise, rule, minY, height);
                        }
                        out.write(text.toString());
                        chunks++;
                    }
                }
            }
            out.write("end " + chunks + "\n");
        }
        storage.close();
        // The game's executors are not daemons.
        System.exit(0);
    }

    /// The terrain of a chunk with the structures' bending, by the generator's own
    /// steps in the order of `buildTerrain`.
    private static void terrain(
        StringBuilder text, ChunkPos pos, LevelHeightAccessor heights,
        PalettedContainerFactory containers, RandomState state, BiomeSource source,
        NoiseBasedChunkGenerator generator, StructureManager manager, long seed, int dimensionMinY,
        int dimensionHeight, Map<Holder<Biome>, Integer> biomeNumbers, Method createNoiseChunk,
        Method doFill, Method buildSurface, Method generateCarvers,
        Field beardPieces, Field beardJunctions, Field beardBox, NoiseSettings noise,
        MaterialRule rule, int minY, int height) throws Exception {
        int lowestCell = dimensionMinY >> 2;
        int cellsHigh = dimensionHeight >> 2;
        ProtoChunk[] nine = new ProtoChunk[9];
        for (int dz = -1; dz <= 1; dz++) {
            for (int dx = -1; dx <= 1; dx++) {
                ChunkPos at = new ChunkPos(pos.x() + dx, pos.z() + dz);
                ProtoChunk one = new ProtoChunk(at, UpgradeData.EMPTY, heights, containers, null);
                DensityBufferPool pool = state.acquireDensityBufferPool();
                try {
                    SamplerContext sampling =
                        SamplerContext.builder().enableCaches().useBufferArena(pool).build();
                    Climate.Sampler sampler = state.createClimateSampler(sampling);
                    BiomeResolver resolver = source.createResolverForChunk(
                        sampler, at.getMinBlockX() >> 2, dimensionMinY >> 2,
                        at.getMinBlockZ() >> 2, 4, dimensionHeight >> 2, 4);
                    one.fillBiomesFromNoise(resolver);
                    one.setPersistedStatus(ChunkStatus.BIOMES);
                } finally {
                    state.releaseDensityBufferPool(pool);
                }
                nine[(dz + 1) * 3 + dx + 1] = one;
            }
        }
        ProtoChunk chunk = nine[4];
        BiomeResolver around = (x, y, z) -> {
            int dx = (x >> 2) - pos.x();
            int dz = (z >> 2) - pos.z();
            if (dx < -1 || dx > 1 || dz < -1 || dz > 1) {
                throw new IllegalStateException("a biome was asked outside the nine chunks");
            }
            return nine[(dz + 1) * 3 + dx + 1].getNoiseBiome(x, y, z);
        };
        BiomeManager biomeManager = new BiomeManager(around, BiomeManager.obfuscateSeed(seed));
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

        // The terrain chunk is not the chunk the starts were made in: the beardifier
        // asks the structure manager by position, and the manager asks the level.
        try (NoiseChunk noiseChunk = (NoiseChunk) createNoiseChunk.invoke(
                generator, chunk, manager, Blender.empty(), state, noise)) {
            // The noise chunk keeps its beardifier to itself; the same call that the
            // generator made for it gives the same one again.
            Beardifier beard = Beardifier.forStructuresInChunk(manager, pos);
            BoundingBox affected = (BoundingBox) beardBox.get(beard);
            text.append("beard ").append(affected == null ? "-" : box(affected).replace(' ', ','))
                .append(" |");
            for (Object rigid : (List<?>) beardPieces.get(beard)) {
                Method boxOf = rigid.getClass().getDeclaredMethod("box");
                Method adjustment = rigid.getClass().getDeclaredMethod("terrainAdjustment");
                Method delta = rigid.getClass().getDeclaredMethod("groundLevelDelta");
                boxOf.setAccessible(true);
                adjustment.setAccessible(true);
                delta.setAccessible(true);
                text.append(' ').append(box((BoundingBox) boxOf.invoke(rigid)).replace(' ', ','))
                    .append(',').append(adjustment.invoke(rigid)).append(',')
                    .append(delta.invoke(rigid));
            }
            text.append(" |");
            for (Object each : (List<?>) beardJunctions.get(beard)) {
                JigsawJunction junction = (JigsawJunction) each;
                text.append(' ').append(junction.getSourceX()).append(',')
                    .append(junction.getSourceGroundY()).append(',').append(junction.getSourceZ())
                    .append(',').append(junction.getDeltaY());
            }
            text.append('\n');

            int top = chunk.getSectionIndex(noiseChunk.volume().maxBlockY());
            int bottom = chunk.getSectionIndex(noiseChunk.volume().minBlockY());
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
            buildSurface.invoke(generator, chunk, noiseChunk, state, biomeManager, possible, rule);
            generateCarvers.invoke(
                generator, chunk, Blender.empty(), noiseChunk, state, biomeManager, null, rule);
        }
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
    }
}
