// Runs the game's own path finder (`PathFinder` with its node evaluators) headless on
// fixed layouts of blocks, so that Clustine's port can be judged node for node
// (docs/groundwork/n7-brief.md, sections 3.0.1 and 3.21; docs/groundwork/n7-plan.md,
// section 11.5, N7c). A search reads only blocks and a dozen things of its mob, so the
// harness gives it a level, a region and a mob whose constructors never run (allocated
// bare) and which answer those from the case: no server is started and nothing is
// drawn.
//
// Bootstrap.bootStrap() is called first, and the tags bound, as Extract.java does, for
// the blocks, their shapes and their tags. Each case is read from the file named first
// and its path written to the file named second:
//
//   case <name>
//   evaluator walk|swim|fly|amphibious
//   mob <x> <y> <z> <width> <height> <step> <max fall> <on ground> <in floatable fluid>
//   flags <pass doors> <open doors> <float> <walk over fences>
//   malus <PathType ordinal> <value>
//   level <min y> <height>
//   fill <x0> <y0> <z0> <x1> <y1> <z1> <state id>
//   search <x> <y> <z> <range> <reach> <max visited>
//   end
//
// Every place not filled is air. The state ids and the path types' ordinals are the
// game's, which Clustine's generated data keeps. The answer for each case is `case <name>`, then `none`,
// or `path <reached> <count>` and a `node <x> <y> <z> <PathType ordinal>` line a node.

import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.EnumMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;

import net.minecraft.SharedConstants;
import net.minecraft.commands.Commands;
import net.minecraft.core.RegistryAccess;
import net.minecraft.server.WorldLoader;
import net.minecraft.server.packs.repository.PackRepository;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.permissions.PermissionSet;
import net.minecraft.tags.BlockTags;
import net.minecraft.util.Util;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.core.BlockPos;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityDimensions;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.entity.animal.cow.Cow;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.PathNavigationRegion;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.pathfinder.AmphibiousNodeEvaluator;
import net.minecraft.world.level.pathfinder.FlyNodeEvaluator;
import net.minecraft.world.level.pathfinder.Node;
import net.minecraft.world.level.pathfinder.NodeEvaluator;
import net.minecraft.world.level.pathfinder.PathFinder;
import net.minecraft.world.level.pathfinder.PathType;
import net.minecraft.world.level.pathfinder.SwimNodeEvaluator;
import net.minecraft.world.level.pathfinder.WalkNodeEvaluator;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.VoxelShape;
import sun.misc.Unsafe;

public class PathHarness {
    /** The blocks of the case being run, by position; air elsewhere. */
    static final Map<BlockPos, BlockState> BLOCKS = new TreeMap<>();
    static int minY;
    static int height;
    static final WorldBorder BORDER = new WorldBorder();

    static BlockState block(BlockPos pos) {
        BlockState state = BLOCKS.get(pos.immutable());
        return state == null ? Blocks.AIR.defaultBlockState() : state;
    }

    /** A level that is never constructed: it answers blocks from the case. */
    static class Layout extends ServerLevel {
        Layout() {
            super(null, null, null, null, null, null, false, 0L, null, false);
        }

        @Override
        public BlockState getBlockState(BlockPos pos) {
            return block(pos);
        }

        @Override
        public FluidState getFluidState(BlockPos pos) {
            return block(pos).getFluidState();
        }

        @Override
        public BlockEntity getBlockEntity(BlockPos pos) {
            return null;
        }

        @Override
        public BlockGetter getChunkForCollisions(int x, int z) {
            return this;
        }

        @Override
        public WorldBorder getWorldBorder() {
            return BORDER;
        }

        @Override
        public List<VoxelShape> getEntityCollisions(Entity entity, AABB box) {
            return List.of();
        }

        @Override
        public int getMinY() {
            return minY;
        }

        @Override
        public int getHeight() {
            return height;
        }
    }

    /** The region a search reads, never constructed either. */
    static class Region extends PathNavigationRegion {
        Region() {
            super(null, null, null);
        }

        @Override
        public BlockState getBlockState(BlockPos pos) {
            return block(pos);
        }

        @Override
        public FluidState getFluidState(BlockPos pos) {
            return block(pos).getFluidState();
        }

        @Override
        public BlockEntity getBlockEntity(BlockPos pos) {
            return null;
        }

        @Override
        public BlockGetter getChunkForCollisions(int x, int z) {
            return this;
        }

        @Override
        public WorldBorder getWorldBorder() {
            return BORDER;
        }

        @Override
        public List<VoxelShape> getEntityCollisions(Entity entity, AABB box) {
            return List.of();
        }

        @Override
        public int getMinY() {
            return minY;
        }

        @Override
        public int getHeight() {
            return height;
        }
    }

    /** A mob that answers what a search asks of it from the case. */
    static class Searcher extends Cow {
        Level layout;
        boolean grounded;
        boolean floatable;
        float step;
        int maxFall;
        Map<PathType, Float> malus;

        Searcher() {
            super(null, null);
        }

        @Override
        public Level level() {
            return layout;
        }

        @Override
        public boolean onGround() {
            return grounded;
        }

        @Override
        public boolean isInFloatableFluid() {
            return floatable;
        }

        @Override
        public float maxUpStep() {
            return step;
        }

        @Override
        public int getMaxFallDistance() {
            return maxFall;
        }

        @Override
        public float getPathfindingMalus(PathType type) {
            Float value = malus.get(type);
            return value == null ? type.getMalus() : value;
        }

        @Override
        public void setPathfindingMalus(PathType type, float value) {
            malus.put(type, value);
        }

        @Override
        public boolean canStandOnFluid(FluidState fluid) {
            return false;
        }

        @Override
        public boolean isDescending() {
            return false;
        }

        @Override
        public boolean isShiftKeyDown() {
            return false;
        }

        @Override
        public ItemStack getMainHandItem() {
            return ItemStack.EMPTY;
        }

        @Override
        public ItemStack getItemBySlot(EquipmentSlot slot) {
            return ItemStack.EMPTY;
        }

        @Override
        public void onPathfindingStart() {}

        @Override
        public void onPathfindingDone() {}
    }

    static Unsafe unsafe() throws Exception {
        Field field = Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        return (Unsafe) field.get(null);
    }

    static void set(Object object, String name, Object value) throws Exception {
        Field field = Entity.class.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        bindTags();
        Unsafe unsafe = unsafe();
        Layout layout = (Layout) unsafe.allocateInstance(Layout.class);
        Region region = (Region) unsafe.allocateInstance(Region.class);

        List<String> lines = Files.readAllLines(Path.of(args[0]), StandardCharsets.UTF_8);
        StringBuilder out = new StringBuilder();
        String name = null;
        String evaluatorName = "walk";
        double[] mob = null;
        boolean[] flags = new boolean[4];
        Map<PathType, Float> malus = new EnumMap<>(PathType.class);
        int[] search = null;
        float range = 0;
        for (String line : lines) {
            String[] w = line.trim().split("\\s+");
            switch (w[0]) {
                case "" -> {}
                case "case" -> {
                    name = w[1];
                    evaluatorName = "walk";
                    mob = null;
                    flags = new boolean[4];
                    malus = new EnumMap<>(PathType.class);
                    search = null;
                    BLOCKS.clear();
                    minY = -64;
                    height = 384;
                }
                case "evaluator" -> evaluatorName = w[1];
                case "mob" -> {
                    mob = new double[9];
                    for (int i = 0; i < 9; i++) {
                        mob[i] = Double.parseDouble(w[i + 1]);
                    }
                }
                case "flags" -> {
                    for (int i = 0; i < 4; i++) {
                        flags[i] = w[i + 1].equals("1");
                    }
                }
                case "malus" -> malus.put(PathType.values()[Integer.parseInt(w[1])], Float.parseFloat(w[2]));
                case "level" -> {
                    minY = Integer.parseInt(w[1]);
                    height = Integer.parseInt(w[2]);
                }
                case "fill" -> {
                    int[] c = new int[6];
                    for (int i = 0; i < 6; i++) {
                        c[i] = Integer.parseInt(w[i + 1]);
                    }
                    BlockState state = Block.stateById(Integer.parseInt(w[7]));
                    for (int x = c[0]; x <= c[3]; x++) {
                        for (int y = c[1]; y <= c[4]; y++) {
                            for (int z = c[2]; z <= c[5]; z++) {
                                BLOCKS.put(new BlockPos(x, y, z), state);
                            }
                        }
                    }
                }
                case "search" -> {
                    search = new int[] {
                        Integer.parseInt(w[1]), Integer.parseInt(w[2]), Integer.parseInt(w[3]),
                        Integer.parseInt(w[5]), Integer.parseInt(w[6])
                    };
                    range = Float.parseFloat(w[4]);
                }
                case "end" -> {
                    out.append("case ").append(name).append('\n');
                    run(unsafe, layout, region, evaluatorName, mob, flags, malus, search, range, out);
                }
                default -> throw new IllegalArgumentException("unknown line: " + line);
            }
        }
        Files.writeString(Path.of(args[1]), out.toString(), StandardCharsets.UTF_8);
        // The game's start leaves threads behind that would keep the JVM alive.
        System.exit(0);
    }

    private record Loaded(RegistryAccess.Frozen registries) {}

    /// Binds the tags of the vanilla data pack, as Extract.java does: the evaluators ask
    /// whether a fluid is water and a block a fence, a door or a fire, by tag.
    static void bindTags() throws Exception {
        PackRepository packs = ServerPacksSource.createVanillaTrustedRepository();
        WorldLoader.PackConfig packConfig =
            new WorldLoader.PackConfig(packs, WorldDataConfiguration.DEFAULT, false, true);
        WorldLoader.InitConfig init = new WorldLoader.InitConfig(
            packConfig, Commands.CommandSelection.DEDICATED, PermissionSet.NO_PERMISSIONS);
        WorldLoader.<Object, Loaded>load(
            init,
            context -> new WorldLoader.DataLoadOutput<>(null, context.datapackDimensions()),
            (resources, serverResources, layers, unused) -> {
                serverResources.updateComponentsAndStaticRegistryTags();
                return new Loaded(layers.compositeAccess());
            },
            Util.backgroundExecutor(), Runnable::run).get();
        if (!Blocks.OAK_FENCE.defaultBlockState().is(BlockTags.FENCES)) {
            throw new IllegalStateException("the block tags are not bound");
        }
    }

    static void run(
        Unsafe unsafe,
        Layout layout,
        Region region,
        String evaluatorName,
        double[] mob,
        boolean[] flags,
        Map<PathType, Float> malus,
        int[] search,
        float range,
        StringBuilder out
    ) throws Exception {
        Searcher searcher = (Searcher) unsafe.allocateInstance(Searcher.class);
        searcher.layout = layout;
        searcher.step = (float) mob[5];
        searcher.maxFall = (int) mob[6];
        searcher.grounded = mob[7] != 0;
        searcher.floatable = mob[8] != 0;
        searcher.malus = malus;
        Vec3 position = new Vec3(mob[0], mob[1], mob[2]);
        EntityDimensions dimensions = EntityDimensions.scalable((float) mob[3], (float) mob[4]);
        set(searcher, "level", layout);
        set(searcher, "type", EntityTypes.COW);
        set(searcher, "position", position);
        set(searcher, "blockPosition", BlockPos.containing(position));
        set(searcher, "dimensions", dimensions);
        set(searcher, "bb", dimensions.makeBoundingBox(position));

        NodeEvaluator evaluator = switch (evaluatorName) {
            case "walk" -> new WalkNodeEvaluator();
            case "swim" -> new SwimNodeEvaluator(false);
            case "fly" -> new FlyNodeEvaluator();
            case "amphibious" -> new AmphibiousNodeEvaluator(false);
            default -> throw new IllegalArgumentException("unknown evaluator: " + evaluatorName);
        };
        evaluator.setCanPassDoors(flags[0]);
        evaluator.setCanOpenDoors(flags[1]);
        evaluator.setCanFloat(flags[2]);
        evaluator.setCanWalkOverFences(flags[3]);
        PathFinder finder = new PathFinder(evaluator, search[4]);
        net.minecraft.world.level.pathfinder.Path path = finder.findPath(
            region, searcher, Set.of(new BlockPos(search[0], search[1], search[2])), range, search[3], 1.0f);
        if (path == null) {
            out.append("none\n");
            return;
        }
        Field reached = net.minecraft.world.level.pathfinder.Path.class.getDeclaredField("reached");
        reached.setAccessible(true);
        out.append("path ").append(reached.getBoolean(path) ? 1 : 0).append(' ')
            .append(path.getNodeCount()).append('\n');
        for (int i = 0; i < path.getNodeCount(); i++) {
            Node node = path.getNode(i);
            out.append("node ").append(node.x).append(' ').append(node.y).append(' ').append(node.z)
                .append(' ').append(node.type.ordinal()).append('\n');
        }
    }
}
