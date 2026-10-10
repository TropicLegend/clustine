import java.lang.reflect.Field;
import java.security.MessageDigest;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.LevelChunkSection;
import net.minecraft.world.level.chunk.status.ChunkStatus;

/**
 * Spike G2 of the terrain plan: the official server run inside a plain Java program.
 *
 * Starts the server as its own main does, finds it through the thread that reads its
 * console, and has it bring the chunks of an area to the terrain status and then to
 * the features status, one after another in ascending x and then z, as SteelMC's
 * fixture was made. Prints the MD5 of each chunk's blocks at both, as that fixture
 * defines it.
 *
 * Arguments: the first chunk's x and z, and how many chunks along each.
 */
public class G2 {
    public static void main(String[] args) throws Exception {
        int x0 = Integer.parseInt(args[0]);
        int z0 = Integer.parseInt(args[1]);
        int nx = Integer.parseInt(args[2]);
        int nz = Integer.parseInt(args[3]);

        net.minecraft.server.Main.main(new String[] {"nogui"});
        MinecraftServer server = null;
        long end = System.currentTimeMillis() + 300_000;
        while (server == null && System.currentTimeMillis() < end) {
            for (Thread thread : Thread.getAllStackTraces().keySet()) {
                // The thread that reads the console is an inner class of the server.
                if (thread.getClass().getName().equals("net.minecraft.server.dedicated.DedicatedServer$1")) {
                    Field outer = thread.getClass().getDeclaredField("this$0");
                    outer.setAccessible(true);
                    server = (MinecraftServer) outer.get(thread);
                }
            }
            Thread.sleep(200);
        }
        if (server == null) {
            System.out.println("G2: no server found");
            System.exit(2);
        }
        while (!server.isReady()) {
            Thread.sleep(200);
        }
        System.out.println("G2: the server is ready");
        ServerLevel level = server.overworld();

        for (ChunkStatus status : new ChunkStatus[] {ChunkStatus.TERRAIN, ChunkStatus.FEATURES}) {
            for (int x = x0; x < x0 + nx; x++) {
                for (int z = z0; z < z0 + nz; z++) {
                    level.getChunkSource().getChunk(x, z, status, true);
                }
            }
            for (int x = x0; x < x0 + nx; x++) {
                for (int z = z0; z < z0 + nz; z++) {
                    ChunkAccess chunk = level.getChunkSource().getChunk(x, z, status, true);
                    System.out.println("G2: " + status + " " + x + " " + z + " "
                            + chunk.getPersistedStatus() + " " + hash(chunk));
                }
            }
        }
        System.out.println("G2: done");
        server.halt(false);
        Thread.sleep(3000);
        System.exit(0);
    }

    static String hash(ChunkAccess chunk) throws Exception {
        MessageDigest md5 = MessageDigest.getInstance("MD5");
        for (LevelChunkSection section : chunk.getSections()) {
            if (section.hasOnlyAir()) {
                md5.update((byte) 0);
                continue;
            }
            byte[] bytes = new byte[4096 * 4];
            int at = 0;
            for (int y = 0; y < 16; y++) {
                for (int z = 0; z < 16; z++) {
                    for (int x = 0; x < 16; x++) {
                        int id = Block.getId(section.getBlockState(x, y, z));
                        bytes[at++] = (byte) (id >>> 24);
                        bytes[at++] = (byte) (id >>> 16);
                        bytes[at++] = (byte) (id >>> 8);
                        bytes[at++] = (byte) id;
                    }
                }
            }
            md5.update(bytes);
        }
        StringBuilder hex = new StringBuilder();
        for (byte b : md5.digest()) {
            hex.append(String.format("%02x", b));
        }
        return hex.toString();
    }
}
