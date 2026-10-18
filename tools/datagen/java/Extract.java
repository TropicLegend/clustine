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
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.function.Supplier;

import com.mojang.datafixers.util.Pair;

import it.unimi.dsi.fastutil.doubles.DoubleList;

import net.minecraft.SharedConstants;
import net.minecraft.commands.Commands;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.WorldLoader;
import net.minecraft.server.packs.repository.PackRepository;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.permissions.PermissionSet;
import net.minecraft.util.Util;
import net.minecraft.world.entity.EntityAttachment;
import net.minecraft.world.entity.EntityAttachments;
import net.minecraft.world.entity.EntityDimensions;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.item.StandingAndWallBlockItem;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.FeatureSorter;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FlowerPotBlock;
import net.minecraft.world.level.block.SupportType;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.entity.BlockEntityType;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.BlockSetType;
import net.minecraft.world.level.chunk.ChunkGenerator;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseRouter;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.WorldDimensions;
import net.minecraft.world.level.levelgen.densityfunction.DensityFunction;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.DiscreteVoxelShape;
import net.minecraft.world.phys.shapes.Shapes;
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
        write(out, "items.txt", items());
        write(out, "block_physics.txt", blockPhysics());
        Collisions collisions = collisions();
        write(out, "voxel_shapes.txt", collisions.shapes.toString());
        write(out, "state_collisions.txt", collisions.states.toString());
        write(out, "entity_types.txt", entityTypes());
        for (Map.Entry<String, String> list : biomeParameters().entrySet()) {
            write(out, "biome_parameters_" + list.getKey() + ".txt", list.getValue());
        }
        for (Map.Entry<String, String> file : routerValues().entrySet()) {
            write(out, "router_" + file.getKey() + ".txt", file.getValue());
        }
        write(out, "spawn_rings.txt", spawnRings());
        write(out, "feature_order.txt", featureOrder());
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

            // What a click or a bucket's ray hits. The game gives it without the offset
            // that plants have by their position, which it adds where it is used.
            level.reset();
            String outlineText = shape(state.getShape(level, FIRST));
            note(consulted, "outline", level);
            if (!outlineText.equals(shape(state.getShape(level, SECOND)))) {
                consulted.add("outline:position");
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
            // Bits 12 to 14 are worked out from the rest by the Rust side.
            flags |= bit(15, state.isRandomlyTicking());
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
            field(text, outlineText);

            // What redstone asks of a state (docs/adr/0026-redstone.md, section 1).
            level.reset();
            boolean conductor = state.isRedstoneConductor(level, FIRST);
            note(consulted, "conductor", level);
            if (conductor != state.isRedstoneConductor(level, SECOND)) {
                consulted.add("conductor:position");
            }
            level.reset();
            float destroyTime = state.getDestroySpeed(level, FIRST);
            note(consulted, "destroy_time", level);
            int redstone = bit(0, conductor)
                | bit(1, state.isSignalSource())
                | bit(2, state.hasAnalogOutputSignal())
                | bit(3, destroyTime == -1.0f);
            field(text, Integer.toString(redstone));

            // The sounds of the block: volume and pitch as their bits, then the break,
            // step, place, hit and fall sounds by their ids (docs/adr/0026-redstone.md,
            // section 8).
            net.minecraft.world.level.block.SoundType sound = state.getSoundType();
            field(text, String.join(",",
                Integer.toHexString(Float.floatToIntBits(sound.getVolume())),
                Integer.toHexString(Float.floatToIntBits(sound.getPitch())),
                Integer.toString(BuiltInRegistries.SOUND_EVENT.getId(sound.getBreakSound())),
                Integer.toString(BuiltInRegistries.SOUND_EVENT.getId(sound.getStepSound())),
                Integer.toString(BuiltInRegistries.SOUND_EVENT.getId(sound.getPlaceSound())),
                Integer.toString(BuiltInRegistries.SOUND_EVENT.getId(sound.getHitSound())),
                Integer.toString(BuiltInRegistries.SOUND_EVENT.getId(sound.getFallSound()))));
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
    /// its block entity type or -1, whether a click without an item uses it, for the
    /// reader the class that handles the click and whether its set type opens by hand,
    /// the class that says where the block can stay (`-` if none), the classes its
    /// class is made from (`-` if none but the base of all blocks), and for a flower
    /// pot the id of the block it holds (-1 for every other block, and the empty pot
    /// holds air).
    private static String blocks() throws ReflectiveOperationException {
        // A simple name has to mean one class, since the committed table has only it.
        TreeMap<String, Class<?>> classes = new TreeMap<>();
        Field pottedField = FlowerPotBlock.class.getDeclaredField("potted");
        pottedField.setAccessible(true);
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
            Class<?> survival = survivalRule(block.getClass());
            field(text, survival == null ? "-" : survival.getSimpleName());

            // The classes the block's class is made from, nearest first, without the
            // base of all blocks: a rule written for doors holds for copper doors.
            List<String> chain = new ArrayList<>();
            for (Class<?> c = named.getSuperclass();
                c != null && c != Block.class && c != BlockBehaviour.class;
                c = c.getSuperclass()) {
                if (c.getSimpleName().isEmpty()) {
                    continue;
                }
                Class<?> other = classes.put(c.getSimpleName(), c);
                if (other != null && other != c) {
                    throw new IllegalStateException(
                        "two block classes are called " + c.getSimpleName());
                }
                chain.add(c.getSimpleName());
            }
            field(text, chain.isEmpty() ? "-" : String.join(",", chain));

            int potted = -1;
            if (block instanceof FlowerPotBlock pot) {
                Block content = (Block) pottedField.get(pot);
                potted = BuiltInRegistries.BLOCK.getId(content);
            }
            field(text, Integer.toString(potted));
            text.append('\n');
        }
        return text.toString();
    }

    /// One line an item, in id order: id, name, the simple name of its class, the id of
    /// the block it places or -1, and for an item that places a standing or a wall
    /// block, the id of the wall block and the direction it attaches in (`down`, `up`
    /// and the four sides), or -1 and `-`.
    private static String items() throws ReflectiveOperationException {
        TreeMap<String, Class<?>> classes = new TreeMap<>();
        Field wallBlock = StandingAndWallBlockItem.class.getDeclaredField("wallBlock");
        Field attachment = StandingAndWallBlockItem.class.getDeclaredField("attachmentDirection");
        wallBlock.setAccessible(true);
        attachment.setAccessible(true);
        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.ITEM.size(); id++) {
            Item item = BuiltInRegistries.ITEM.byId(id);

            Class<?> named = item.getClass();
            while (named.getSimpleName().isEmpty()) {
                named = named.getSuperclass();
            }
            Class<?> earlier = classes.put(named.getSimpleName(), named);
            if (earlier != null && earlier != named) {
                throw new IllegalStateException(
                    "two item classes are called " + named.getSimpleName());
            }

            int block = -1;
            int wall = -1;
            String direction = "-";
            if (item instanceof BlockItem placing) {
                block = BuiltInRegistries.BLOCK.getId(placing.getBlock());
            }
            if (item instanceof StandingAndWallBlockItem standing) {
                wall = BuiltInRegistries.BLOCK.getId((Block) wallBlock.get(standing));
                direction = ((Direction) attachment.get(standing)).getSerializedName();
            }

            text.append(Integer.toString(id));
            field(text, BuiltInRegistries.ITEM.getKey(item).toString());
            field(text, named.getSimpleName());
            field(text, Integer.toString(block));
            field(text, Integer.toString(wall));
            field(text, direction);
            text.append('\n');
        }
        return text.toString();
    }

    /// One line a block, in id order: id, name, then what an entity meets on it
    /// (docs/groundwork/n4-brief.md, sections 1.8 and 11): friction, speed factor, jump
    /// factor, bounce restitution and fall-distance reduction as the bits of the floats
    /// the block holds; the offset type its properties gave it (`NONE`, `XZ` or `XYZ`)
    /// and the largest horizontal and vertical offsets as float bits; and the name of
    /// its loot table, or `-` for a block without one. A block that drops like another
    /// (a wall torch, a potted plant's pot) names the other's table.
    private static String blockPhysics() throws ReflectiveOperationException {
        Method horizontal = BlockBehaviour.class.getDeclaredMethod("getMaxHorizontalOffset");
        Method vertical = BlockBehaviour.class.getDeclaredMethod("getMaxVerticalOffset");
        horizontal.setAccessible(true);
        vertical.setAccessible(true);
        // The offset type is not kept; what is kept is the function `offsetType` chose,
        // one shared instance a type, which a fresh set of properties shows.
        Field function = BlockBehaviour.Properties.class.getDeclaredField("offsetFunction");
        function.setAccessible(true);
        Object xz = function.get(BlockBehaviour.Properties.of()
            .offsetType(BlockBehaviour.OffsetType.XZ));
        Object xyz = function.get(BlockBehaviour.Properties.of()
            .offsetType(BlockBehaviour.OffsetType.XYZ));
        if (xz == null || xyz == null || xz == xyz
            || xz != function.get(BlockBehaviour.Properties.of()
                .offsetType(BlockBehaviour.OffsetType.XZ))) {
            throw new IllegalStateException("the offset functions are not one instance a type");
        }

        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.BLOCK.size(); id++) {
            Block block = BuiltInRegistries.BLOCK.byId(id);
            Object own = function.get(block.properties());
            String offsetType;
            if (own == null) {
                offsetType = "NONE";
            } else if (own == xz) {
                offsetType = "XZ";
            } else if (own == xyz) {
                offsetType = "XYZ";
            } else {
                throw new IllegalStateException(
                    "the offset function of " + BuiltInRegistries.BLOCK.getKey(block)
                        + " is of no offset type");
            }
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                if (state.hasOffsetFunction() != (own != null)) {
                    throw new IllegalStateException(
                        "a state of " + BuiltInRegistries.BLOCK.getKey(block)
                            + " has another offset function than its block");
                }
                if (offsetType.equals("XZ") && state.getOffset(SECOND).y != 0.0) {
                    throw new IllegalStateException(
                        "an offset of the type XZ moves "
                            + BuiltInRegistries.BLOCK.getKey(block) + " up or down");
                }
            }

            text.append(Integer.toString(id));
            field(text, BuiltInRegistries.BLOCK.getKey(block).toString());
            field(text, floatBits(block.getFriction()));
            field(text, floatBits(block.getSpeedFactor()));
            field(text, floatBits(block.getJumpFactor()));
            field(text, floatBits(block.getBounceRestitution()));
            field(text, floatBits(block.getFallDistanceReduction()));
            field(text, offsetType);
            field(text, floatBits((Float) horizontal.invoke(block)));
            field(text, floatBits((Float) vertical.invoke(block)));
            field(text, block.getLootTable()
                .map(key -> key.identifier().toString()).orElse("-"));
            text.append('\n');
        }
        return text.toString();
    }

    /// The points of one kind an entity type's attachments hold, as `entityTypes` writes
    /// them. The builder fills a kind no type declares from its fallback, so none is
    /// empty.
    private static String attachmentPoints(EntityAttachments attachments, EntityAttachment kind)
            throws ReflectiveOperationException {
        Field field = EntityAttachments.class.getDeclaredField("attachments");
        field.setAccessible(true);
        @SuppressWarnings("unchecked")
        Map<EntityAttachment, List<Vec3>> points = (Map<EntityAttachment, List<Vec3>>) field.get(attachments);
        List<Vec3> list = points.get(kind);
        if (list == null || list.isEmpty()) {
            throw new IllegalStateException("an entity type has no " + kind + " points");
        }
        StringBuilder text = new StringBuilder();
        for (Vec3 point : list) {
            if (text.length() > 0) {
                text.append('|');
            }
            text.append(Long.toHexString(Double.doubleToRawLongBits(point.x)));
            text.append(';');
            text.append(Long.toHexString(Double.doubleToRawLongBits(point.y)));
            text.append(';');
            text.append(Long.toHexString(Double.doubleToRawLongBits(point.z)));
        }
        return text.toString();
    }

    private static String floatBits(float value) {
        return Integer.toHexString(Float.floatToRawIntBits(value));
    }

    /// The classes that override `getEntityInsideCollisionShape`
    /// (docs/groundwork/n4-brief.md, section 11, item 4). Powder snow's answer is the
    /// entity's own collision shape, or the whole block where that is empty, and is
    /// written as `entity`; the others answer from the state and are asked with no
    /// entity.
    private static final String[] INSIDE_SHAPE_CLASSES = {
        "EndPortalBlock", "LavaCauldronBlock", "LayeredCauldronBlock", "PowderSnowBlock",
        "TripWireBlock",
    };

    /// The two files of collision shapes in the game's own voxel form.
    private static final class Collisions {
        /// `voxel_shapes.txt`: one line a distinct shape, in the order the states first
        /// name them, its line number being its index: whether it is the instance
        /// `Shapes.block()` (1 or 0), the coordinates of its cells' boundaries along x,
        /// y and z (each a list of double bits separated by commas), the numbers of
        /// cells along x, y and z, and the filled cells as a bit set in hexadecimal,
        /// two digits a byte, bit `(x * ySize + y) * zSize + z` being cell (x, y, z),
        /// or `-` if none is filled.
        final StringBuilder shapes = new StringBuilder();
        /// `state_collisions.txt`: one line a state, in id order: its id, the index of
        /// its collision shape (with the empty context, and without the sideways
        /// offset of a block whose collision shape is shifted by its position),
        /// `hasLargeCollisionShape` (1 or 0), and the index of its entity-inside
        /// collision shape or `entity`.
        final StringBuilder states = new StringBuilder();
        final Map<String, Integer> numbers = new LinkedHashMap<>();

        int number(VoxelShape shape) throws ReflectiveOperationException {
            String line = voxels(shape);
            Integer known = numbers.get(line);
            if (known != null) {
                return known;
            }
            int number = numbers.size();
            numbers.put(line, number);
            shapes.append(line).append('\n');
            return number;
        }
    }

    private static Collisions collisions() throws ReflectiveOperationException {
        Collisions collisions = new Collisions();
        Recorder level = new Recorder();
        for (int id = 0; id < Block.BLOCK_STATE_REGISTRY.size(); id++) {
            BlockState state = Block.BLOCK_STATE_REGISTRY.byId(id);
            level.reset();
            VoxelShape collision = state.getCollisionShape(level, FIRST);
            String atSecond = voxels(state.getCollisionShape(level, SECOND));
            if (!voxels(collision).equals(atSecond)) {
                // Bamboo and the dripstones: the shape is shifted by the offset of its
                // position. The table keeps it unshifted, which has to be the shape at
                // the second position shifted back by that position's offset.
                Vec3 first = state.getOffset(FIRST);
                Vec3 second = state.getOffset(SECOND);
                collision = collision.move(-first.x, -first.y, -first.z);
                VoxelShape moved = state.getCollisionShape(level, SECOND)
                    .move(-second.x, -second.y, -second.z);
                if (!voxels(collision).equals(voxels(moved))) {
                    throw new IllegalStateException(
                        "the collision shape of state " + id
                            + " changes with its position by more than its offset");
                }
            }

            int collisionNumber = collisions.number(collision);

            // The entity-inside shape has to be the state's alone.
            level.reset();
            String inside;
            Class<?> overrides = insideShapeClass(state.getBlock().getClass());
            if (overrides == null) {
                VoxelShape shape = state.getEntityInsideCollisionShape(level, FIRST, null);
                if (shape != Shapes.block()) {
                    throw new IllegalStateException(
                        "state " + id + " has an entity-inside shape of its own");
                }
                inside = Integer.toString(collisions.number(shape));
            } else if (overrides.getSimpleName().equals("PowderSnowBlock")) {
                inside = "entity";
            } else if (List.of(INSIDE_SHAPE_CLASSES).contains(overrides.getSimpleName())) {
                inside = Integer.toString(collisions.number(
                    state.getEntityInsideCollisionShape(level, FIRST, null)));
            } else {
                throw new IllegalStateException(
                    overrides.getSimpleName() + " has an entity-inside shape of its own");
            }
            if (!level.asked.isEmpty()) {
                throw new IllegalStateException(
                    "the entity-inside shape of state " + id + " looked at the level");
            }

            collisions.states.append(Integer.toString(id)).append('\t')
                .append(Integer.toString(collisionNumber)).append('\t')
                .append(state.hasLargeCollisionShape() ? '1' : '0').append('\t')
                .append(inside).append('\n');
        }
        return collisions;
    }

    /// The class, at or above `start` and below the base of all blocks, that overrides
    /// `getEntityInsideCollisionShape`; `null` if none does.
    private static Class<?> insideShapeClass(Class<?> start) {
        for (Class<?> c = start; c != BlockBehaviour.class && c != null; c = c.getSuperclass()) {
            try {
                c.getDeclaredMethod(
                    "getEntityInsideCollisionShape",
                    BlockState.class, BlockGetter.class, BlockPos.class,
                    net.minecraft.world.entity.Entity.class);
                return c;
            } catch (NoSuchMethodException none) {
                // Look further up.
            }
        }
        return null;
    }

    /// A shape in its voxel form, as a line of `voxel_shapes.txt`.
    private static String voxels(VoxelShape shape) throws ReflectiveOperationException {
        Field field = VoxelShape.class.getDeclaredField("shape");
        field.setAccessible(true);
        DiscreteVoxelShape cells = (DiscreteVoxelShape) field.get(shape);
        StringBuilder line = new StringBuilder();
        line.append(shape == Shapes.block() ? '1' : '0');
        Direction.Axis[] axes = {Direction.Axis.X, Direction.Axis.Y, Direction.Axis.Z};
        for (Direction.Axis axis : axes) {
            DoubleList coords = shape.getCoords(axis);
            if (coords.size() != cells.getSize(axis) + 1) {
                throw new IllegalStateException(
                    "a shape has " + coords.size() + " coordinates along " + axis
                        + " for " + cells.getSize(axis) + " cells");
            }
            List<String> parts = new ArrayList<>();
            for (int i = 0; i < coords.size(); i++) {
                parts.add(bits(coords.getDouble(i)));
            }
            line.append('\t').append(String.join(",", parts));
        }
        int xs = cells.getXSize();
        int ys = cells.getYSize();
        int zs = cells.getZSize();
        line.append('\t').append(Integer.toString(xs)).append(',')
            .append(Integer.toString(ys)).append(',').append(Integer.toString(zs));
        byte[] bitSet = new byte[(xs * ys * zs + 7) / 8];
        boolean any = false;
        for (int x = 0; x < xs; x++) {
            for (int y = 0; y < ys; y++) {
                for (int z = 0; z < zs; z++) {
                    if (cells.isFull(x, y, z)) {
                        int index = (x * ys + y) * zs + z;
                        bitSet[index / 8] |= (byte) (1 << (index % 8));
                        any = true;
                    }
                }
            }
        }
        line.append('\t');
        if (!any) {
            line.append('-');
        } else {
            for (byte b : bitSet) {
                String hex = Integer.toHexString(b & 0xff);
                line.append(hex.length() == 1 ? "0" + hex : hex);
            }
        }
        return line.toString();
    }

    /// One line an entity type, in id order: id, name, width, height and eye height as
    /// float bits, whether its size is fixed, its client tracking range in chunks, its
    /// update interval in ticks, whether its motion is sent (`trackDeltas`), whether it
    /// is immune to fire, its mob category by the game's name for it, and the points its
    /// passengers sit at and it sits at on a vehicle (`EntityAttachments`' `PASSENGER` and
    /// `VEHICLE` lists, unrotated: `docs/groundwork/n5-brief.md`, section 8.1), each a list
    /// of `x;y;z` as double bits joined by `|`.
    private static String entityTypes() throws ReflectiveOperationException {
        StringBuilder text = new StringBuilder();
        for (int id = 0; id < BuiltInRegistries.ENTITY_TYPE.size(); id++) {
            EntityType<?> type = BuiltInRegistries.ENTITY_TYPE.byId(id);
            EntityDimensions dimensions = type.getDimensions();
            if (dimensions.width() != type.getWidth() || dimensions.height() != type.getHeight()) {
                throw new IllegalStateException("an entity type's size is two sizes");
            }
            text.append(Integer.toString(id));
            field(text, BuiltInRegistries.ENTITY_TYPE.getKey(type).toString());
            field(text, floatBits(dimensions.width()));
            field(text, floatBits(dimensions.height()));
            field(text, floatBits(dimensions.eyeHeight()));
            field(text, dimensions.fixed() ? "1" : "0");
            field(text, Integer.toString(type.clientTrackingRange()));
            field(text, Integer.toString(type.updateInterval()));
            field(text, type.trackDeltas() ? "1" : "0");
            field(text, type.fireImmune() ? "1" : "0");
            field(text, type.getCategory().getSerializedName());
            field(text, attachmentPoints(dimensions.attachments(), EntityAttachment.PASSENGER));
            field(text, attachmentPoints(dimensions.attachments(), EntityAttachment.VEHICLE));
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

    /// The class, at or above `start` and below the base of all blocks, that says
    /// where a block of it can stay (`canSurvive`); `null` if none does, and the block
    /// stays anywhere.
    private static Class<?> survivalRule(Class<?> start) {
        for (Class<?> c = start; c != BlockBehaviour.class && c != null; c = c.getSuperclass()) {
            try {
                c.getDeclaredMethod(
                    "canSurvive", BlockState.class, LevelReader.class, BlockPos.class);
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

    /// The offsets from the centre of a ring at which `NoiseSpawnFinder.radialSearch`
    /// samples, one line each: the pass's increment, then x and z. They are the same
    /// for every seed, as the centre is fixed for a pass and only added. The loop is
    /// a copy of the game's, run here because `Math.sin` and `Math.cos` are what
    /// HotSpot makes of them, which no Rust library is bound to give in the last bit;
    /// the terrain's comparison with the game's own search over many seeds judges the
    /// copy.
    private static String spawnRings() {
        StringBuilder text = new StringBuilder();
        // The two passes of `NoiseSpawnFinder`'s constructor: the largest radius, then
        // the increment.
        float[][] passes = {{2048.0f, 512.0f}, {512.0f, 32.0f}};
        for (float[] pass : passes) {
            float max = pass[0];
            float increment = pass[1];
            float angle = 0.0f;
            float radius = increment;
            while (radius <= max) {
                int x = (int) (Math.sin((double) angle) * (double) radius);
                int z = (int) (Math.cos((double) angle) * (double) radius);
                text.append((int) increment).append('\t').append(x).append('\t').append(z)
                        .append('\n');
                angle += increment / radius;
                if ((double) angle > 6.283185307179586) {
                    angle = 0.0f;
                    radius += increment;
                }
            }
        }
        return text.toString();
    }

    private record Loaded(RegistryAccess.Frozen registries) {}

    /// The order in which the game places the features of each dimension of a new
    /// world, step by step: one line a feature, its dimension, its step and its name,
    /// in the order of the step. It is read from each generator's own memo,
    /// `featuresPerStep`, which the game's `FeatureSorter` made from the biome source's
    /// possible biomes; a feature's index in its step, which is part of its seed, is its
    /// place in that list, and that is checked against the game's own mapping. The data
    /// is the game's own data pack, loaded as a server loads it before it has a world.
    private static String featureOrder() throws Exception {
        PackRepository packs = ServerPacksSource.createVanillaTrustedRepository();
        WorldLoader.PackConfig packConfig =
            new WorldLoader.PackConfig(packs, WorldDataConfiguration.DEFAULT, false, true);
        WorldLoader.InitConfig init = new WorldLoader.InitConfig(
            packConfig, Commands.CommandSelection.DEDICATED, PermissionSet.NO_PERMISSIONS);
        RegistryAccess.Frozen registries = WorldLoader.<Object, Loaded>load(
            init,
            context -> new WorldLoader.DataLoadOutput<>(null, context.datapackDimensions()),
            (resources, serverResources, layers, unused) -> new Loaded(layers.compositeAccess()),
            Util.backgroundExecutor(), Runnable::run).get().registries();

        WorldDimensions dimensions = registries.lookupOrThrow(Registries.WORLD_PRESET)
            .getOrThrow(WorldPresets.NORMAL).value().createWorldDimensions();
        Registry<PlacedFeature> placed = registries.lookupOrThrow(Registries.PLACED_FEATURE);
        Field memo = ChunkGenerator.class.getDeclaredField("featuresPerStep");
        memo.setAccessible(true);

        TreeMap<String, ResourceKey<LevelStem>> stems = new TreeMap<>();
        stems.put("overworld", LevelStem.OVERWORLD);
        stems.put("nether", LevelStem.NETHER);
        stems.put("end", LevelStem.END);
        StringBuilder text = new StringBuilder();
        for (Map.Entry<String, ResourceKey<LevelStem>> stem : stems.entrySet()) {
            ChunkGenerator generator =
                dimensions.get(stem.getValue()).orElseThrow().generator();
            @SuppressWarnings("unchecked")
            List<FeatureSorter.StepFeatureData> steps =
                ((Supplier<List<FeatureSorter.StepFeatureData>>) memo.get(generator)).get();
            for (int step = 0; step < steps.size(); step++) {
                List<PlacedFeature> features = steps.get(step).features();
                for (int index = 0; index < features.size(); index++) {
                    PlacedFeature feature = features.get(index);
                    if (steps.get(step).indexMapping().applyAsInt(feature) != index) {
                        throw new IllegalStateException(
                            "a feature's index in its step is not its place in the list");
                    }
                    Object name = placed.getKey(feature);
                    if (name == null) {
                        throw new IllegalStateException("a placed feature has no name");
                    }
                    text.append(stem.getKey()).append('\t').append(step).append('\t')
                        .append(name).append('\n');
                }
            }
        }
        return text.toString();
    }

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
