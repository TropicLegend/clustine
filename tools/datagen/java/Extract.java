// Extract.java: values of Minecraft's blocks and biomes that are in the game's code and
// in none of its data files, and values the game computes that the emitted Rust is
// tested against, written out for `cargo datagen` (docs/adr/0019, section 5).
//
// Clustine's own program, under the AGPL like the rest of the repository. It calls the
// public classes of the server jar after `Bootstrap.bootStrap()` and starts no server.
// It was written from the names in the jar's class files; Mojang's source was not read.
// The idea of asking the running game for these values, and which calls answer them, is
// SteelExtractor's (https://github.com/Steel-Foundation/SteelExtractor, CC0, at commit
// e1e269595b4e059abaa16d1b440b4369c55dddff), which is credited here although its
// licence asks for nothing.
//
// What makes the output the same bytes on every machine: integers are written with
// `Integer.toString` and `Long.toString`, doubles as their bits with `Long.toHexString`,
// never through `format`; rows are taken in id order from the game's registries; files
// are written in UTF-8, named explicitly, and nothing goes to standard output, which the
// game's start wraps.
//
// Usage: java Extract <output directory>

import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;

import com.mojang.datafixers.util.Pair;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.registries.Registries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SupportType;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockSetType;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.shapes.VoxelShape;

public final class Extract {
    /// The two positions every question that takes a position is asked at. An answer
    /// that differs between them is not a value of the state alone.
    private static final BlockPos FIRST = new BlockPos(0, 0, 0);
    private static final BlockPos SECOND = new BlockPos(1013, 71, -3187);

    private static final SupportType[] SUPPORT_TYPES = {
        SupportType.FULL, SupportType.CENTER, SupportType.RIGID,
    };

    /// A level that holds nothing but air and remembers that it was asked.
    ///
    /// Several of the state's methods want a level and a position. Almost every block
    /// answers from the state alone; one that looks at the level gives here the answer
    /// it gives in empty air, which is not a value of the state, so it has to be named.
    private static final class Recorder implements BlockGetter {
        private final TreeSet<String> asked = new TreeSet<>();

        void reset() {
            asked.clear();
        }

        @Override
        public BlockEntity getBlockEntity(BlockPos pos) {
            asked.add("block_entity");
            return null;
        }

        @Override
        public BlockState getBlockState(BlockPos pos) {
            asked.add("block_state");
            return Blocks.AIR.defaultBlockState();
        }

        @Override
        public FluidState getFluidState(BlockPos pos) {
            asked.add("fluid_state");
            return Fluids.EMPTY.defaultFluidState();
        }

        @Override
        public int getHeight() {
            asked.add("height");
            return 384;
        }

        @Override
        public int getMinY() {
            asked.add("height");
            return -64;
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            throw new IllegalArgumentException("usage: java Extract <output directory>");
        }
        Path out = Path.of(args[0]);
        Files.createDirectories(out);

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();

        write(out, "fluids.txt", fluids());
        write(out, "block_entity_types.txt", blockEntityTypes());
        write(out, "block_states.txt", blockStates());
        write(out, "blocks.txt", blocks());
        for (Map.Entry<String, String> list : biomeParameters().entrySet()) {
            write(out, "biome_parameters_" + list.getKey() + ".txt", list.getValue());
        }
        for (Map.Entry<String, String> file : routerValues().entrySet()) {
            write(out, "router_" + file.getKey() + ".txt", file.getValue());
        }
        // The game's start leaves threads behind that would keep the JVM alive.
        System.exit(0);
    }

    private static void write(Path out, String name, String content) throws IOException {
        Files.write(out.resolve(name), content.getBytes(StandardCharsets.UTF_8));
    }

    /// One line a fluid: its id and its name.
    private static String fluids() {
        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.FLUID.size(); id++) {
            Fluid fluid = BuiltInRegistries.FLUID.byId(id);
            text.append(Integer.toString(id)).append('\t');
            text.append(BuiltInRegistries.FLUID.getKey(fluid).toString()).append('\n');
        }
        return text.toString();
    }

    /// One line a block entity type: its id and its name.
    private static String blockEntityTypes() {
        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.BLOCK_ENTITY_TYPE.size(); id++) {
            BlockEntityType<?> type = BuiltInRegistries.BLOCK_ENTITY_TYPE.byId(id);
            text.append(Integer.toString(id)).append('\t');
            text.append(BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(type).toString()).append('\n');
        }
        return text.toString();
    }

    /// One line a block state, in id order; the fields are separated by tabs and named
    /// in `tools/datagen/src/extract.rs`, which reads them.
    private static String blockStates() {
        Recorder level = new Recorder();
        StringBuilder text = new StringBuilder();
        int count = Block.BLOCK_STATE_REGISTRY.size();
        for (int id = 0; id < count; id++) {
            BlockState state = Block.BLOCK_STATE_REGISTRY.byId(id);
            if (state == null || Block.getId(state) != id) {
                throw new IllegalStateException("block state ids are not dense at " + id);
            }
            // Which questions looked at the level or answered by position.
            TreeSet<String> consulted = new TreeSet<>();

            text.append(Integer.toString(id));
            field(text, BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
            field(text, Integer.toString(state.getLightEmission()));
            field(text, Integer.toString(state.getLightDampening()));

            level.reset();
            VoxelShape collision = state.getCollisionShape(level, FIRST);
            String collisionText = shape(collision);
            boolean collisionFull = state.isCollisionShapeFullBlock(level, FIRST);
            note(consulted, "collision", level);
            if (!collisionText.equals(shape(state.getCollisionShape(level, SECOND)))
                || collisionFull != state.isCollisionShapeFullBlock(level, SECOND)) {
                consulted.add("collision:position");
            }

            int flags = 0;
            flags |= bit(0, state.isAir());
            flags |= bit(1, state.isSolid());
            flags |= bit(2, state.isSolidRender());
            flags |= bit(3, state.liquid());
            flags |= bit(4, state.canBeReplaced());
            flags |= bit(5, state.canOcclude());
            flags |= bit(6, state.useShapeForLightOcclusion());
            flags |= bit(7, state.propagatesSkylightDown());
            flags |= bit(8, state.isLightPermeable());
            flags |= bit(9, state.hasBlockEntity());
            flags |= bit(10, collision.isEmpty());
            flags |= bit(11, collisionFull);
            field(text, Integer.toString(flags));

            for (SupportType type : SUPPORT_TYPES) {
                level.reset();
                int faces = sturdyFaces(state, level, FIRST, type);
                note(consulted, "sturdy", level);
                if (faces != sturdyFaces(state, level, SECOND, type)) {
                    consulted.add("sturdy:position");
                }
                field(text, Integer.toString(faces));
            }

            field(text, state.getPistonPushReaction().name());

            FluidState fluid = state.getFluidState();
            if (fluid.isEmpty()) {
                field(text, "-1");
                field(text, "0");
                field(text, "0");
                field(text, "0");
            } else {
                field(text, Integer.toString(BuiltInRegistries.FLUID.getId(fluid.getType())));
                field(text, Integer.toString(fluid.getAmount()));
                field(text, fluid.isSource() ? "1" : "0");
                boolean falling = fluid.hasProperty(FlowingFluid.FALLING)
                    && fluid.getValue(FlowingFluid.FALLING);
                field(text, falling ? "1" : "0");
            }

            level.reset();
            String post = offset(state.getPostProcessPos(level, FIRST), FIRST);
            note(consulted, "post_process", level);
            if (!post.equals(offset(state.getPostProcessPos(level, SECOND), SECOND))) {
                throw new IllegalStateException(
                    "post-processing of state " + id + " is not one offset at two positions");
            }
            field(text, post);

            for (Direction face : Direction.values()) {
                field(text, shape(state.getFaceOcclusionShape(face)));
            }
            field(text, collisionText);
            field(text, consulted.isEmpty() ? "-" : String.join(",", consulted));
            text.append('\n');
        }
        return text.toString();
    }

    private static void field(StringBuilder text, String value) {
        text.append('\t').append(value);
    }

    private static int bit(int index, boolean set) {
        return set ? 1 << index : 0;
    }

    private static void note(TreeSet<String> consulted, String question, Recorder level) {
        for (String what : level.asked) {
            consulted.add(question + ":" + what);
        }
    }

    /// The faces that are sturdy for `type`, one bit a direction in the order of
    /// `Direction`: down, up, north, south, west, east.
    private static int sturdyFaces(
        BlockState state, BlockGetter level, BlockPos pos, SupportType type) {
        int faces = 0;
        for (Direction face : Direction.values()) {
            faces |= bit(face.ordinal(), state.isFaceSturdy(level, pos, face, type));
        }
        return faces;
    }

    /// A position as its offset from where the question was asked, or `none`.
    private static String offset(BlockPos answer, BlockPos asked) {
        if (answer == null) {
            return "none";
        }
        return Integer.toString(answer.getX() - asked.getX())
            + "," + Integer.toString(answer.getY() - asked.getY())
            + "," + Integer.toString(answer.getZ() - asked.getZ());
    }

    /// A shape as its boxes in the game's own order, each six doubles written as bits;
    /// `-` for the empty shape.
    private static String shape(VoxelShape shape) {
        if (shape.isEmpty()) {
            return "-";
        }
        List<String> boxes = new ArrayList<>();
        shape.forAllBoxes((minX, minY, minZ, maxX, maxY, maxZ) -> boxes.add(
            bits(minX) + "," + bits(minY) + "," + bits(minZ)
                + "," + bits(maxX) + "," + bits(maxY) + "," + bits(maxZ)));
        return String.join(";", boxes);
    }

    private static String bits(double value) {
        // Negative zero would be other bits for the same place.
        return Long.toHexString(Double.doubleToLongBits(value == 0.0 ? 0.0 : value));
    }

    /// One line a block, in id order: id, name, the simple name of its class, the id of
    /// its block entity type or -1, whether a click without an item uses it, and for the
    /// reader the class that handles the click and whether its set type opens by hand.
    private static String blocks() throws ReflectiveOperationException {
        // A simple name has to mean one class, since the committed table has only it.
        TreeMap<String, Class<?>> classes = new TreeMap<>();
        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.BLOCK.size(); id++) {
            Block block = BuiltInRegistries.BLOCK.byId(id);

            Class<?> named = block.getClass();
            while (named.getSimpleName().isEmpty()) {
                named = named.getSuperclass();
            }
            Class<?> earlier = classes.put(named.getSimpleName(), named);
            if (earlier != null && earlier != named) {
                throw new IllegalStateException(
                    "two block classes are called " + named.getSimpleName());
            }

            int entityType = -1;
            BlockState state = block.defaultBlockState();
            for (int type = 0; type < BuiltInRegistries.BLOCK_ENTITY_TYPE.size(); type++) {
                if (BuiltInRegistries.BLOCK_ENTITY_TYPE.byId(type).isValid(state)) {
                    if (entityType != -1) {
                        throw new IllegalStateException(
                            "two block entity types take " + BuiltInRegistries.BLOCK.getKey(block));
                    }
                    entityType = type;
                }
            }
            if (state.hasBlockEntity() != (entityType != -1)) {
                throw new IllegalStateException(
                    "no one block entity type for " + BuiltInRegistries.BLOCK.getKey(block));
            }

            Class<?> handler = clickHandler(block.getClass());
            Boolean opensByHand = opensByHand(block);
            boolean used = handler != null && (opensByHand == null || opensByHand);

            text.append(Integer.toString(id));
            field(text, BuiltInRegistries.BLOCK.getKey(block).toString());
            field(text, named.getSimpleName());
            field(text, Integer.toString(entityType));
            field(text, used ? "1" : "0");
            field(text, handler == null ? "-" : handler.getSimpleName());
            field(text, opensByHand == null ? "-" : opensByHand ? "1" : "0");
            text.append('\n');
        }
        return text.toString();
    }

    /// The class, at or above `start` and below the base of all blocks, that has a
    /// handler of its own for a click without an item; `null` if none has.
    private static Class<?> clickHandler(Class<?> start) {
        for (Class<?> c = start; c != BlockBehaviour.class && c != null; c = c.getSuperclass()) {
            try {
                c.getDeclaredMethod(
                    "useWithoutItem",
                    BlockState.class, Level.class, BlockPos.class, Player.class,
                    BlockHitResult.class);
                return c;
            } catch (NoSuchMethodException none) {
                // Look further up.
            }
        }
        return null;
    }

    /// Whether the block's set type opens by hand; `null` for a block without one.
    private static Boolean opensByHand(Block block) throws ReflectiveOperationException {
        Boolean answer = null;
        for (Class<?> c = block.getClass(); c != null; c = c.getSuperclass()) {
            for (Field field : c.getDeclaredFields()) {
                if (field.getType() != BlockSetType.class
                    || Modifier.isStatic(field.getModifiers())) {
                    continue;
                }
                field.setAccessible(true);
                BlockSetType type = (BlockSetType) field.get(block);
                if (answer != null && answer != type.canOpenByHand()) {
                    throw new IllegalStateException("two set types in " + block);
                }
                answer = type.canOpenByHand();
            }
        }
        return answer;
    }

    /// The parameter lists by the name of their preset (`overworld`, `nether`), one line
    /// an entry in the list's own order: the lower and upper bound of temperature,
    /// humidity, continentalness, erosion, depth and weirdness, the offset, and the
    /// biome, all as the integers the game holds.
    private static TreeMap<String, String> biomeParameters() {
        TreeMap<String, String> lists = new TreeMap<>();
        for (Map.Entry<
                MultiNoiseBiomeSourceParameterList.Preset,
                Climate.ParameterList<ResourceKey<Biome>>> preset
            : MultiNoiseBiomeSourceParameterList.knownPresets().entrySet()) {
            StringBuilder text = new StringBuilder();
            for (Pair<Climate.ParameterPoint, ResourceKey<Biome>> entry
                : preset.getValue().values()) {
                Climate.ParameterPoint point = entry.getFirst();
                Climate.Parameter[] parameters = {
                    point.temperature(), point.humidity(), point.continentalness(),
                    point.erosion(), point.depth(), point.weirdness(),
                };
                for (Climate.Parameter parameter : parameters) {
                    text.append(Long.toString(parameter.min())).append('\t');
                    text.append(Long.toString(parameter.max())).append('\t');
                }
                text.append(Long.toString(point.offset())).append('\t');
                text.append(entry.getSecond().identifier().toString()).append('\n');
            }
            String name = preset.getKey().id().getPath();
            if (lists.put(name, text.toString()) != null) {
                throw new IllegalStateException("two parameter lists are called " + name);
            }
        }
        return lists;
    }

    /// The seeds the routers' values are computed for.
    private static final long[] ROUTER_SEEDS = {13579L, 0L};

    /// How many positions each router is asked at, and how many of them are chosen
    /// by hand.
    private static final int ROUTER_POSITIONS = 256;
    private static final int BY_HAND = 24;

    /// The values of every entry of the noise routers of the overworld, the Nether and
    /// the End at fixed positions, for the seeds above: one file for each dimension and
    /// seed, by a name such as `overworld_13579`.
    ///
    /// They are what `RandomState.sampleBlockValueUncached` gives: the game's own
    /// density functions, made as a world of that seed makes them, asked at one block
    /// at a time with nothing cached and no structure near. The settings are those the
    /// game has built in, which its data generator writes out as the files in the jar;
    /// `cargo datagen` checks that the two are the same bytes.
    ///
    /// The positions are the same for both seeds but for the heights that are taken
    /// near the surface, which is elsewhere in another world.
    ///
    /// A file names the settings and the seed, then each entry: `entry <name>` for one
    /// that has a value in every row, `constant <name> <bits>` for one whose value is
    /// the same at every position. A row is a position (x, y, z) and the values of the
    /// entries in their order, each the bits of a float in hexadecimal.
    private static TreeMap<String, String> routerValues() {
        HolderLookup.Provider lookup = VanillaRegistries.createWorldLookup();
        HolderGetter<NormalNoise> noises = lookup.lookupOrThrow(Registries.NOISE);
        HolderLookup.RegistryLookup<NoiseGeneratorSettings> registry =
            lookup.lookupOrThrow(Registries.NOISE_SETTINGS);
        TreeMap<String, ResourceKey<NoiseGeneratorSettings>> dimensions = new TreeMap<>();
        dimensions.put("overworld", NoiseGeneratorSettings.OVERWORLD);
        dimensions.put("nether", NoiseGeneratorSettings.NETHER);
        dimensions.put("end", NoiseGeneratorSettings.END);

        TreeMap<String, String> files = new TreeMap<>();
        for (Map.Entry<String, ResourceKey<NoiseGeneratorSettings>> dimension
            : dimensions.entrySet()) {
            NoiseGeneratorSettings settings = registry.getOrThrow(dimension.getValue()).value();
            List<String> names = new ArrayList<>();
            List<DensityFunction> functions = new ArrayList<>();
            NoiseRouter router = settings.noiseRouter();
            names.add("temperature");
            functions.add(router.temperature());
            names.add("vegetation");
            functions.add(router.vegetation());
            names.add("continents");
            functions.add(router.continents());
            names.add("erosion");
            functions.add(router.erosion());
            names.add("depth");
            functions.add(router.depth());
            names.add("ridges");
            functions.add(router.ridges());
            names.add("chunk_surface_level");
            functions.add(router.chunkSurfaceLevel());
            names.add("final_density");
            functions.add(router.finalDensity());
            if (settings.aquifers().isPresent()) {
                Aquifer.Config aquifers = settings.aquifers().get();
                names.add("aquifers.barrier");
                functions.add(aquifers.barrierNoise());
                names.add("aquifers.fluid_level_floodedness");
                functions.add(aquifers.fluidLevelFloodednessNoise());
                names.add("aquifers.fluid_level_spread");
                functions.add(aquifers.fluidLevelSpreadNoise());
                names.add("aquifers.lava");
                functions.add(aquifers.lavaNoise());
                names.add("aquifers.exclusion");
                functions.add(aquifers.exclusion());
                names.add("aquifers.surface_level");
                functions.add(aquifers.surfaceLevel());
            }
            for (long seed : ROUTER_SEEDS) {
                RandomState state = RandomState.create(noises, seed, settings);
                int[][] positions = routerPositions(
                    settings.noiseSettings().minY(), settings.noiseSettings().height());
                if (settings.aquifers().isPresent()) {
                    // Most of a dimension is far above or below the ground, where the
                    // final density is at one of its two ends. Every other position is
                    // moved to within 48 blocks below and 16 above where the game
                    // itself expects the surface, so that caves and slopes are asked.
                    DensityFunction surface = settings.aquifers().get().surfaceLevel();
                    for (int row = BY_HAND; row < positions.length; row += 2) {
                        int[] at = positions[row];
                        float level = state.sampleBlockValueUncached(surface, at[0], at[1], at[2]);
                        at[1] = (int) level - 48 + Math.floorMod(at[1], 64);
                    }
                }
                int[][] values = new int[functions.size()][positions.length];
                boolean[] constant = new boolean[functions.size()];
                for (int entry = 0; entry < functions.size(); entry++) {
                    constant[entry] = true;
                    for (int row = 0; row < positions.length; row++) {
                        int[] at = positions[row];
                        values[entry][row] = Float.floatToRawIntBits(
                            state.sampleBlockValueUncached(
                                functions.get(entry), at[0], at[1], at[2]));
                        constant[entry] &= values[entry][row] == values[entry][0];
                    }
                }

                StringBuilder text = new StringBuilder();
                text.append("settings ").append(dimension.getValue().identifier().toString());
                text.append('\n');
                text.append("seed ").append(Long.toString(seed)).append('\n');
                for (int entry = 0; entry < functions.size(); entry++) {
                    if (constant[entry]) {
                        text.append("constant ").append(names.get(entry)).append(' ');
                        text.append(Integer.toHexString(values[entry][0])).append('\n');
                    } else {
                        text.append("entry ").append(names.get(entry)).append('\n');
                    }
                }
                for (int row = 0; row < positions.length; row++) {
                    int[] at = positions[row];
                    text.append(Integer.toString(at[0])).append(' ');
                    text.append(Integer.toString(at[1])).append(' ');
                    text.append(Integer.toString(at[2]));
                    for (int entry = 0; entry < functions.size(); entry++) {
                        if (!constant[entry]) {
                            text.append(' ').append(Integer.toHexString(values[entry][row]));
                        }
                    }
                    text.append('\n');
                }
                files.put(dimension.getKey() + "_" + Long.toString(seed), text.toString());
            }
        }
        return files;
    }

    /// The positions a router is asked at, for a dimension whose terrain starts at
    /// `minY` and is `height` blocks high: some chosen by hand, at and beside the
    /// corners of the cells that functions are interpolated in, at the origin, below
    /// zero and at the ends of the height; the rest spread by a fixed sequence of
    /// numbers, most near the origin, some far out and some near the world's edge.
    private static int[][] routerPositions(int minY, int height) {
        int top = minY + height;
        int middle = minY + height / 2;
        int[][] byHand = {
            {0, 0, 0}, {0, middle, 0}, {-1, middle - 1, -1}, {1, middle + 1, 1},
            {3, middle + 7, 3}, {4, middle + 8, 4}, {15, 63, 15}, {16, 64, 16},
            {-16, 64, -16}, {-17, 65, -17}, {8, minY, 8}, {8, minY - 1, 8},
            {8, minY + 1, 8}, {-8, top, -8}, {-8, top - 1, -8}, {-8, top + 1, -8},
            {1023, middle, 0}, {1024, middle, 0}, {0, middle, -1025}, {-737, 60, 737},
            {5000, 70, -5000}, {-5001, 30, 4999}, {123456, 40, -654321},
            {29999984, middle, -29999984},
        };
        if (byHand.length != BY_HAND) {
            throw new IllegalStateException("the positions chosen by hand are not " + BY_HAND);
        }
        int[][] positions = new int[ROUTER_POSITIONS][];
        System.arraycopy(byHand, 0, positions, 0, byHand.length);
        // Knuth's 64-bit linear congruential generator, of which the high bits are
        // taken; it is written out so that the positions rest on nothing else.
        long state = 0x636c757374696e65L;
        for (int row = byHand.length; row < positions.length; row++) {
            int[] at = new int[3];
            int reach = row % 8 == 7 ? 29_999_000 : row % 4 == 3 ? 100_000 : 600;
            int[] spans = {2 * reach + 1, height + 16, 2 * reach + 1};
            int[] lowest = {-reach, minY - 8, -reach};
            for (int axis = 0; axis < 3; axis++) {
                state = state * 6364136223846793005L + 1442695040888963407L;
                at[axis] = lowest[axis] + (int) Long.remainderUnsigned(state >>> 16, spans[axis]);
            }
            positions[row] = at;
        }
        return positions;
    }
}
