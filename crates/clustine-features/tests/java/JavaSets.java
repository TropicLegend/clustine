import java.util.HashSet;
import java.util.Random;
import java.util.ArrayList;
import java.util.List;
import net.minecraft.core.BlockPos;

/// Iteration orders of java.util.HashSet<BlockPos> after sequences of adds and removes,
/// for the tests of Clustine's port of that order (`src/java_set.rs`): trees and patches
/// place in it. It made `tests/data/java_sets.txt`, with the game's own BlockPos:
///
///     javac -proc:none -cp <classes> -d <out> crates/clustine-features/tests/java/JavaSets.java
///     java -cp <out>:<classes> JavaSets > crates/clustine-features/tests/data/java_sets.txt
///
/// where <classes> is every jar under target/datagen/26.3/extract/classpath, on JDK 25.
public class JavaSets {
    public static void main(String[] args) {
        Random random = new Random(26_3_2026L);
        StringBuilder out = new StringBuilder();
        out.append("# java.util.HashSet<net.minecraft.core.BlockPos> on JDK ")
            .append(Runtime.version().feature()).append(": a case is its operations in order\n")
            .append("# (a: add, r: remove) and then the set's iteration order.\n");
        int cases = 0;
        for (int i = 0; i < 40; i++) {
            int size = 1 + random.nextInt(i < 20 ? 40 : 300);
            int far = i % 2 == 0 ? random.nextInt(2_000_000) - 1_000_000 : random.nextInt(64) - 32;
            int cx = far, cz = i % 3 == 0 ? -far : far / 3, cy = random.nextInt(300) - 64;
            List<String> ops = new ArrayList<>();
            HashSet<BlockPos> set = new HashSet<>();
            List<BlockPos> added = new ArrayList<>();
            for (int k = 0; k < size; k++) {
                BlockPos pos = new BlockPos(cx + random.nextInt(21) - 10, cy + random.nextInt(31) - 15,
                    cz + random.nextInt(21) - 10);
                set.add(pos); added.add(pos); ops.add("a " + p(pos));
                if (i >= 30 && random.nextInt(4) == 0 && !added.isEmpty()) {
                    BlockPos gone = added.get(random.nextInt(added.size()));
                    set.remove(gone); ops.add("r " + p(gone));
                }
            }
            write(out, ++cases, ops, set);
        }
        // Many positions in few buckets: the bins become trees and back.
        for (int step : new int[] {64, 1024, 65536}) {
            List<String> ops = new ArrayList<>();
            HashSet<BlockPos> set = new HashSet<>();
            List<BlockPos> added = new ArrayList<>();
            for (int k = 0; k < 120; k++) {
                BlockPos pos = new BlockPos(k * step, 70 + (k % 3), -5);
                set.add(pos); added.add(pos); ops.add("a " + p(pos));
            }
            write(out, ++cases, ops, set);
            for (int k = 0; k < 110; k++) {
                BlockPos gone = added.get(random.nextInt(added.size()));
                set.remove(gone); ops.add("r " + p(gone));
            }
            write(out, ++cases, ops, set);
        }
        System.out.print(out);
    }

    static String p(BlockPos pos) {
        return pos.getX() + "," + pos.getY() + "," + pos.getZ();
    }

    static void write(StringBuilder out, int n, List<String> ops, HashSet<BlockPos> set) {
        out.append("case ").append(n).append('\n').append(String.join(" ", ops)).append('\n');
        StringBuilder order = new StringBuilder();
        for (BlockPos pos : set) {
            if (order.length() > 0) order.append(' ');
            order.append(p(pos));
        }
        out.append(order).append('\n');
    }
}
