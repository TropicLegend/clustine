import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;

import it.unimi.dsi.fastutil.shorts.ShortList;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ChunkLevel;
import net.minecraft.server.level.ServerChunkCache;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.Ticket;
import net.minecraft.server.level.TicketType;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.ticks.SavedTick;

/// The judge of Clustine's features (docs/groundwork/f-plan.md, section 3; the rules of
/// docs/adr/0021-generation-is-a-function.md's section on testing): the official server,
/// started inside this program as its own main starts it, made to place the features of
/// an area in Clustine's order of runs, rank by rank, and read back in this process.
///
/// It runs in a server directory the caller prepared (eula.txt, server.properties with
/// the seed and structures off, a data pack that turns the features Clustine has not
/// built into no_op), with its standard input kept open, since the thread that reads the
/// console and through which the server is found ends when it closes. Arguments: the
/// file of the area (one line, `x0 z0 x1 z1`, the chunks whose features run, both corners
/// included), `ascending` or `descending` (the order within a rank), the output file.
///
/// What it checks as it goes, and fails on:
/// - every chunk within two of the area is held by a ticket of this program's own,
///   without a timeout, at the level that keeps a chunk at TERRAIN, so that none is let
///   go, saved and read back between two requests;
/// - before the first run, no held chunk is beyond TERRAIN: the server's search for a
///   spawn, which decorates the chunks it looks at, did not reach the area;
/// - after each request, exactly the chunks requested so far are at FEATURES, so a
///   request decorated no neighbour;
/// - at the end, every chunk of the area is the object first loaded, at FEATURES and
///   not beyond.
///
/// It writes, for every chunk of the area: its block states, its block entities as the
/// game saves them (without id, x, y, z) in the network form of NBT, in hexadecimal, its
/// block and fluid ticks, its marks for post-processing and its four final heightmaps.
public class GameFeatures {
    /// Clustine's ranks of runs by (x mod 3, z mod 3) (ADR-0021; `lattice.rs`).
    private static final int[][] RANKS = {{0, 1, 2}, {7, 6, 3}, {8, 5, 4}};

    private static final Heightmap.Types[] FINAL = {
        Heightmap.Types.WORLD_SURFACE,
        Heightmap.Types.OCEAN_FLOOR,
        Heightmap.Types.MOTION_BLOCKING,
        Heightmap.Types.MOTION_BLOCKING_NO_LEAVES,
    };

    public static void main(String[] args) throws Exception {
        if (args.length != 3) {
            throw new IllegalArgumentException(
                "usage: java GameFeatures <area> <ascending|descending> <output>");
        }
        String[] area = Files.readString(Path.of(args[0])).trim().split("\\s+");
        int x0 = Integer.parseInt(area[0]);
        int z0 = Integer.parseInt(area[1]);
        int x1 = Integer.parseInt(area[2]);
        int z1 = Integer.parseInt(area[3]);
        boolean ascending = switch (args[1]) {
            case "ascending" -> true;
            case "descending" -> false;
            default -> throw new IllegalArgumentException("an order is ascending or descending");
        };
        Path output = Path.of(args[2]);

        net.minecraft.server.Main.main(new String[] {"nogui"});
        MinecraftServer server = find();
        while (!server.isReady()) {
            Thread.sleep(100);
        }
        ServerLevel level = server.overworld();
        ServerChunkCache cache = level.getChunkSource();

        // Held: the area and two chunks around it.
        TicketType hold = new TicketType(TicketType.NO_TIMEOUT, TicketType.FLAG_LOADING);
        int terrain = ChunkLevel.byStatus(ChunkStatus.TERRAIN);
        List<ChunkPos> held = new ArrayList<>();
        for (int x = x0 - 2; x <= x1 + 2; x++) {
            for (int z = z0 - 2; z <= z1 + 2; z++) {
                held.add(new ChunkPos(x, z));
            }
        }
        on(server, () -> {
            for (ChunkPos pos : held) {
                cache.addTicket(new Ticket(hold, terrain), pos);
            }
            return null;
        });
        Map<Long, ChunkAccess> loaded = new HashMap<>();
        for (ChunkPos pos : held) {
            ChunkAccess chunk = cache.getChunk(pos.x(), pos.z(), ChunkStatus.TERRAIN, true);
            loaded.put(pos.pack(), chunk);
        }
        for (ChunkPos pos : held) {
            ChunkStatus status = loaded.get(pos.pack()).getPersistedStatus();
            if (!status.isOrBefore(ChunkStatus.TERRAIN)) {
                fail("the chunk " + pos + " was at " + status + " before the first run: "
                    + "the server decorated it before it was asked to");
            }
        }

        // The runs, rank by rank.
        List<ChunkPos> runs = new ArrayList<>();
        for (int rank = 0; rank < 9; rank++) {
            List<ChunkPos> ofRank = new ArrayList<>();
            for (int x = x0; x <= x1; x++) {
                for (int z = z0; z <= z1; z++) {
                    if (RANKS[Math.floorMod(x, 3)][Math.floorMod(z, 3)] == rank) {
                        ofRank.add(new ChunkPos(x, z));
                    }
                }
            }
            Comparator<ChunkPos> byPlace =
                Comparator.<ChunkPos>comparingInt(ChunkPos::x).thenComparingInt(ChunkPos::z);
            ofRank.sort(ascending ? byPlace : byPlace.reversed());
            for (ChunkPos pos : ofRank) {
                ChunkAccess chunk = cache.getChunk(pos.x(), pos.z(), ChunkStatus.FEATURES, true);
                if (chunk != loaded.get(pos.pack())) {
                    fail("the chunk " + pos + " is not the object first loaded");
                }
                runs.add(pos);
                long decorated = held.stream()
                    .filter(p -> loaded.get(p.pack()).getPersistedStatus()
                        .isOrAfter(ChunkStatus.FEATURES))
                    .count();
                if (decorated != runs.size()) {
                    fail("after the run of " + pos + ", " + decorated + " chunks are decorated "
                        + "where " + runs.size() + " runs were asked for");
                }
            }
        }

        StringBuilder text = new StringBuilder("game-features 1\n");
        on(server, () -> {
            for (int x = x0; x <= x1; x++) {
                for (int z = z0; z <= z1; z++) {
                    ChunkAccess chunk = loaded.get(ChunkPos.pack(x, z));
                    if (chunk.getPersistedStatus() != ChunkStatus.FEATURES) {
                        fail("the chunk " + x + ", " + z + " ended at "
                            + chunk.getPersistedStatus());
                    }
                    write(text, level, chunk);
                }
            }
            // The biome cells of every chunk held, so that Clustine is given the game's
            // answers to exact ties of biomes wherever a run reads them.
            for (ChunkPos pos : held) {
                text.append("cells ").append(pos.x()).append(' ').append(pos.z());
                biomes(text, loaded.get(pos.pack()));
            }
            return null;
        });
        Files.writeString(output, text.toString());
        // The server's threads would keep the process alive, and its own stop did not
        // end it in the trial G2.
        Runtime.getRuntime().halt(0);
    }

    private static void write(StringBuilder text, ServerLevel level, ChunkAccess chunk)
        throws Exception {
        ChunkPos pos = chunk.getPos();
        text.append("chunk ").append(pos.x()).append(' ').append(pos.z()).append('\n');

        // Block states in the order y, z, x from the bottom, in runs.
        text.append("blocks");
        int last = -1;
        int count = 0;
        for (LevelChunkSection section : chunk.getSections()) {
            for (int y = 0; y < 16; y++) {
                for (int z = 0; z < 16; z++) {
                    for (int x = 0; x < 16; x++) {
                        int id = Block.getId(section.getBlockState(x, y, z));
                        if (id == last) {
                            count++;
                            continue;
                        }
                        if (count > 0) {
                            text.append(' ').append(last).append('*').append(count);
                        }
                        last = id;
                        count = 1;
                    }
                }
            }
        }
        text.append(' ').append(last).append('*').append(count).append('\n');

        // Biome cells of each section in the order y, z, x, in runs of names: where the
        // game answered an exact tie of biomes otherwise than Clustine, a feature may be
        // placed in one world and not in the other.
        text.append("biomes");
        biomes(text, chunk);

        // Block entities, by position: those made and those still kept as data.
        ProtoChunk proto = (ProtoChunk) chunk;
        TreeMap<Long, String> entities = new TreeMap<>();
        for (Map.Entry<BlockPos, BlockEntity> entry : proto.getBlockEntities().entrySet()) {
            BlockEntity entity = entry.getValue();
            CompoundTag tag = entity.saveWithoutMetadata(level.registryAccess());
            String type = BuiltInRegistries.BLOCK_ENTITY_TYPE.getKey(entity.getType()).toString();
            entities.put(entry.getKey().asLong(), line(entry.getKey(), type, tag));
        }
        for (Map.Entry<BlockPos, CompoundTag> entry : proto.getBlockEntityNbts().entrySet()) {
            CompoundTag tag = entry.getValue().copy();
            String type = tag.getStringOr("id", "?");
            for (String key : new String[] {"id", "x", "y", "z"}) {
                tag.remove(key);
            }
            entities.put(entry.getKey().asLong(), line(entry.getKey(), type, tag));
        }
        for (String entity : entities.values()) {
            text.append(entity);
        }

        // Ticks as the chunk would save them.
        ChunkAccess.PackedTicks ticks = chunk.getTicksForSerialization(0);
        for (SavedTick<Block> tick : ticks.blocks()) {
            text.append("tick block ").append(at(tick.pos())).append(' ')
                .append(BuiltInRegistries.BLOCK.getKey(tick.type())).append(' ')
                .append(tick.delay()).append(' ').append(tick.priority().getValue()).append('\n');
        }
        for (SavedTick<Fluid> tick : ticks.fluids()) {
            text.append("tick fluid ").append(at(tick.pos())).append(' ')
                .append(BuiltInRegistries.FLUID.getKey(tick.type())).append(' ')
                .append(tick.delay()).append(' ').append(tick.priority().getValue()).append('\n');
        }

        // Marks for post-processing, by section.
        text.append("marks");
        ShortList[] marks = chunk.getPostProcessing();
        for (int index = 0; index < marks.length; index++) {
            if (marks[index] == null) {
                continue;
            }
            int sectionY = chunk.getSectionYFromSectionIndex(index);
            for (short packed : marks[index]) {
                BlockPos mark = ProtoChunkPos.unpack(packed, sectionY, pos);
                text.append(' ').append(at(mark));
            }
        }
        text.append('\n');

        for (Heightmap.Types type : FINAL) {
            Heightmap map = chunk.getOrCreateHeightmapUnprimed(type);
            text.append("height ").append(type.getSerializationKey());
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    text.append(' ').append(map.getFirstAvailable(x, z));
                }
            }
            text.append('\n');
        }
    }

    /// The biome cells of `chunk`, section by section in the order y, z, x, as runs of
    /// names, and the end of the line.
    private static void biomes(StringBuilder text, ChunkAccess chunk) {
        ChunkPos pos = chunk.getPos();
        String lastBiome = null;
        int biomes = 0;
        int sections = chunk.getSections().length;
        for (int index = 0; index < sections; index++) {
            int quartY = chunk.getSectionYFromSectionIndex(index) << 2;
            for (int y = 0; y < 4; y++) {
                for (int z = 0; z < 4; z++) {
                    for (int x = 0; x < 4; x++) {
                        String biome = chunk.getNoiseBiome(
                            (pos.x() << 2) + x, quartY + y, (pos.z() << 2) + z)
                            .unwrapKey().orElseThrow().identifier().toString();
                        if (biome.equals(lastBiome)) {
                            biomes++;
                            continue;
                        }
                        if (biomes > 0) {
                            text.append(' ').append(lastBiome).append('*').append(biomes);
                        }
                        lastBiome = biome;
                        biomes = 1;
                    }
                }
            }
        }
        text.append(' ').append(lastBiome).append('*').append(biomes).append('\n');
    }

    private static String line(BlockPos pos, String type, CompoundTag tag) throws Exception {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        NbtIo.writeAnyTag(tag, new DataOutputStream(bytes));
        StringBuilder hex = new StringBuilder();
        for (byte b : bytes.toByteArray()) {
            hex.append(String.format("%02x", b));
        }
        return "entity " + at(pos) + " " + type + " " + hex + "\n";
    }

    private static String at(BlockPos pos) {
        return pos.getX() + "," + pos.getY() + "," + pos.getZ();
    }

    /// The position a mark for post-processing stands for: the game packs the block's
    /// place in its section into a short (`ProtoChunk.packOffsetCoordinates`).
    private static final class ProtoChunkPos {
        static BlockPos unpack(short packed, int sectionY, ChunkPos chunk) {
            int x = packed & 15;
            int y = (packed >>> 4) & 15;
            int z = (packed >>> 8) & 15;
            return new BlockPos(chunk.getMinBlockX() + x, (sectionY << 4) + y,
                chunk.getMinBlockZ() + z);
        }
    }

    private static MinecraftServer find() throws Exception {
        long end = System.currentTimeMillis() + 300_000;
        while (System.currentTimeMillis() < end) {
            for (Thread thread : Thread.getAllStackTraces().keySet()) {
                // The thread that reads the console is an inner class of the server.
                if (thread.getClass().getName()
                    .equals("net.minecraft.server.dedicated.DedicatedServer$1")) {
                    Field outer = thread.getClass().getDeclaredField("this$0");
                    outer.setAccessible(true);
                    return (MinecraftServer) outer.get(thread);
                }
            }
            Thread.sleep(100);
        }
        fail("no server was found");
        return null;
    }

    private static <T> T on(MinecraftServer server, Callable<T> task) throws Exception {
        CompletableFuture<T> done = server.submit(() -> {
            try {
                return task.call();
            } catch (Exception error) {
                throw new RuntimeException(error);
            }
        });
        return done.get();
    }

    private static void fail(String why) {
        System.err.println("GameFeatures: " + why);
        System.out.flush();
        Runtime.getRuntime().halt(3);
    }
}
