// GameSpawn.java: where the game begins to look for a new world's spawn, for
// `tests/game_spawn.rs` to hold `clustine_terrain::spawn` to.
//
// It asks the public classes of the server jar after `Bootstrap.bootStrap()`, as
// `tools/datagen/java/Extract.java` does, and starts no server and makes no world.
// For each seed it does what `NoiseBasedChunkGenerator.getOrigin` does for the
// overworld: the settings' spawn target, a sampler set with caches, and
// `NoiseSpawnFinder.findSpawnPosition`, of which it writes the block.
//
// Needs a JDK 25 and the jar's classes on the class path; `tests/game_spawn.rs` says
// how it was run. What it writes is values computed by Mojang's code: Mojang's data,
// not under the licence of this file (see NOTICE.md at the root of the repository).
//
//   java GameSpawn <count> <output>
//
// The seeds are 0, 13579, and then the first longs of `java.util.Random(13579)`, up
// to the count. The output:
//
//   game-spawn 1
//   seed <seed> <x> <z>        one a seed, in that order

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Random;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.NoiseSpawnFinder;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.densityfunction.SamplerContext;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public final class GameSpawn {
    public static void main(String[] args) throws IOException {
        if (args.length != 2) {
            throw new IllegalArgumentException("usage: java GameSpawn <count> <output>");
        }
        int count = Integer.parseInt(args[0]);

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        HolderLookup.Provider lookup = VanillaRegistries.createWorldLookup();
        HolderGetter<NormalNoise> noises = lookup.lookupOrThrow(Registries.NOISE);
        NoiseGeneratorSettings settings = lookup.lookupOrThrow(Registries.NOISE_SETTINGS)
            .getOrThrow(NoiseGeneratorSettings.OVERWORLD).value();

        long[] seeds = new long[count];
        Random random = new Random(13579);
        for (int i = 0; i < count; i++) {
            seeds[i] = switch (i) {
                case 0 -> 0;
                case 1 -> 13579;
                default -> random.nextLong();
            };
        }

        StringBuilder out = new StringBuilder("game-spawn 1\n");
        for (long seed : seeds) {
            RandomState state = RandomState.create(noises, seed, settings);
            BlockPos found = NoiseSpawnFinder.findSpawnPosition(
                settings.spawnTarget(),
                state.samplersWithContext(SamplerContext.builder().enableCaches().build()));
            out.append("seed ").append(seed).append(' ').append(found.getX()).append(' ')
                .append(found.getZ()).append('\n');
        }
        Files.write(Path.of(args[1]), out.toString().getBytes(StandardCharsets.UTF_8));
        // The game's start leaves threads behind that would keep the JVM alive.
        System.exit(0);
    }
}
