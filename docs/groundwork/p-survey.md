# Survey for the next phase: the Nether and the End

Everything here was read in place, with nothing built, run or changed. "The data" means the 26.3 jar's generated data under `target/datagen/26.3/generated/`. Where I say something is absent, I searched for it and did not find it.

## 0. In short

- **The world has one dimension, and that is fixed in every layer.**
  - The identifiers have no dimension: `ChunkPos`, `BlockPos` and `RegionId` (`crates/clustine-world/src/position.rs:44-47`, `:8-12`, `:232`).
  - The world store's chunk directory is the constant `"overworld"` (`services/worldstore/src/chunks.rs:41-45`).
  - The edge's replica is keyed by `ChunkPos` alone (`services/edge/src/fanout.rs:825`).
  - The login names the overworld as a constant (`services/edge/src/fanout.rs:63`, `:6927-6955`).
  - A player's saved `Place` and hand-over `PlayerTransfer` carry no dimension (`crates/clustine-sim/src/api.rs:89-107`, `:399-436`).
- **Generated entity keys already name a dimension**, in 2 bits (`position.rs:115-152`; `crates/clustine-world/src/generation.rs:317-326`).
- **Terrain is mostly ready.** The Nether's and the End's biomes, fill, surface and carvers exist and are judged against the game's own classes (`crates/clustine-terrain/src/lib.rs:1-45`; `docs/roadmap.md:1996-2042`). Still missing:
  - nothing serves the Nether or the End (`services/worldgen/src/terrain.rs:124-140`);
  - 12 kinds of features are unbuilt (`crates/clustine-features/src/dispatch.rs:29-87`, `features/nether_and_end.rs:8-11`);
  - the placers for the fortress, the Nether fossil and the End city are unbuilt (`crates/clustine-features/src/structures/mod.rs:67-88`).
- **Light already supports a dimension without a sky** (`crates/clustine-light/src/lib.rs:24-27`, `:80-81`; `engine.rs:156`, `:273`). But every caller passes `sky: true`, and the generator wrongly treats the End as skyless (details in section 3).
- **No portal, no Respawn packet, and none of the Nether's or the End's mobs exist.** The rules that differ by dimension are written for the overworld; several comments say so ("no Nether yet").

## 1. Where the code assumes one dimension

### 1.1 Identifiers

| Item | Today | Dimension? |
|---|---|---|
| `BlockPos`, `ChunkPos` | `x,y,z` / `x,z` only (`crates/clustine-world/src/position.rs:8-12`, `:44-47`) | none |
| `ChunkArea` (pins) | x-range over the whole world (`position.rs:311-329`) | none |
| `RegionId` | `u32`, issued by the store (`position.rs:228-232`) | none; a region implies nothing |
| `EntityId`, `EntityIds` | 64-bit keys in blocks of 2^32 (`position.rs:90-107`, `:264-307`) | not needed: keys are unique for the world |
| Generated entity key | `Generated { dimension: u8, chunk, structure, index }`, dimension in 2 bits, at most 4 (`position.rs:115-152`); `dimension_number` maps overworld 0, Nether 1, End 2, else 3 (`generation.rs:317-326`); `Makings::new(dimension, …)` (`generation.rs:266-282`); worldgen passes `dimension_number(self.dimension().name)` (`services/worldgen/src/features.rs:225-229`, `:250`) | **yes, already** |
| `PlayerId`, `EdgeId` | UUID, FNV of a name (`position.rs:224-262`) | not needed |
| Wire ids | per edge, `WireIds` (`services/edge/src/fanout.rs:927-…`) | not needed; ids stay unique per edge |

### 1.2 Region model, coordinator and routing

- `RoutingTable` has one `home` and one `spawn` (`crates/clustine-region/src/lib.rs:24-45`). `RegionRoute` has no dimension (`:13-20`).
- The store's `RegionList` has one `home` and `spawn` (`crates/clustine-rpc/src/messages.rs:1055-1069`). `RegionInfo` holds bounds as a `ChunkBox` and pins as `ChunkArea`, with no dimension (`:1074-1093`).
- `PlayersOf` reports crowds as `(ChunkPos, u32)` (`messages.rs:1101-1118`).
- The coordinator merges and splits by chunk distance alone (`services/coordinator/src/policy.rs:50`, `:161`). Two regions in different dimensions would be merged when their players stand at nearby coordinates. It needs a dimension per region, and an infinite distance between dimensions.
- `RegionConfig` has no dimension (`crates/clustine-sim/src/region.rs:144-185`). Its `spawn` is the home chunk "which a region that holds it never gives back" (`:145-147`), and `lowest_y` comes from the overworld (`bin/clustine/src/cluster/worker.rs:1383-1390`).
- Its sea level is hard-coded, 63 or -63 (`services/worker/src/lib.rs:1068`). Its features are always the overworld's (`services/worker/src/lib.rs:1165-1180`).
- `Restored` carries one `spawn`, one `world` number and one `seed` (`messages.rs:874-886`).
- `Restored.world` seeds random ticks per chunk, section and tick (`crates/clustine-sim/src/region/random_ticks.rs:55-63`). A Nether region and an overworld region at the same coordinates would draw the same streams unless the dimension is mixed in.

### 1.3 The world store

- **Layout.** The format describes `chunks/<dimension>/<rx>.<rz>/…`, but "a world has one dimension, and every world is the overworld so far" (`docs/world-format.md:88-89`, `:131-136`). The code fixes it as `const DIMENSION: &str = "overworld"` (`services/worldstore/src/chunks.rs:41-45`; used in `chunk_path`, `:331-336`). So the path already has room for the dimension; only the constant and every `ChunkPos` key need to change.
- **`world_number`** is the FNV-1a hash of the generator's identity (`services/worldstore/src/lib.rs:845-853`). The identity already names the dimension type: `terrain;features …;minecraft:overworld;seed …` (`services/worldgen/src/features.rs:121-141`; format in `docs/adr/0021-generation-is-a-function.md:1019-1030`, where "a world has one").
  - `begin_clocked` takes **one** generator. It derives `world` and `seed` from that generator and hands one `ChunkService` that generator (`lib.rs:892-929`).
  - `meta` has one `generator=` line (`docs/world-format.md:92-109`).
  - If the identity names three dimensions, the world number changes and every existing world is refused (`docs/world-format.md:54-58`).
- **Maps keyed by `ChunkPos` alone**, each of which would need the dimension:
  - the table's grants and owners (`services/worldstore/src/table.rs:151`, `:168`);
  - effect users and effects by chunk (`effects.rs:30`, `:114`);
  - arrivals (`arrivals.rs:12-25`);
  - copies (`copies.rs:17-21`);
  - lends (`lends.rs:27`, `:37`);
  - players' faces (`faces.rs:35`);
  - spawn counts (`spawn_counts.rs:30`);
  - the published points of interest and their margin (`village.rs:42`, `:59`);
  - the chunk service's pending saves and bases (`chunks.rs:187`, `:751-753`).
- **World-wide state that the game keeps per level (per dimension):**
  - the table's single `home_chunk`, `home_region` and `clock` (`crates/clustine-format/src/region.rs:224-238`);
  - the published `WorldPlayer` list, with no dimension (`crates/clustine-sim/src/api.rs:354-363`), which sleep, despawning and patrols read;
  - the mob caps counted across every region (`docs/roadmap.md:422-427`);
  - the trader chance (`region.rs:239-241`).

### 1.4 The edge

- `replica: BTreeMap<ChunkPos, ReplicaChunk>` (`services/edge/src/fanout.rs:825`), each entry with one `served_by` region (`:449`).
- Each `PlayerView` has `region`, `wanted`, `pending`, `sent`, `replace` and `visible`, all chunk-keyed (`fanout.rs:534-603`).
- If two players stood at the same coordinates in two dimensions, they would share one replica entry. This is the edge's main structural change: key by `(dimension, ChunkPos)`, or keep one replica per dimension.
- Subscriptions and their answers name `ChunkPos` only (`crates/clustine-rpc/src/messages.rs:136-148`, `:175-187`, `:236-244`). They can stay so if a region has one dimension, since the link to a region then implies it.
- `spawn_region` is the one region every join goes to (`fanout.rs:775`, `:1014`).
- Light jobs are hard-coded `sky: true`, with the comment "The one dimension an edge serves yet has a sky" (`fanout.rs:5476-5477`).
- Outbox entries: `Durable::Departed { player, transfer, to }` (`api.rs:1017-1022`) and `Misdirected::Arrival` (`api.rs:1112-1121`). These are dimension-free except through `to`, and through `transfer.pose`, which has no dimension (`api.rs:19-27`).

### 1.5 The login

- `login_packet` sends `dimension_names: [overworld]`, the overworld's type id and name, plus `is_flat` and `sea_level` from `LoginWorld` (`services/edge/src/fanout.rs:6927-6955`, constant at `:63`).
- `LoginWorld` is `{ hashed_seed, flat, sea_level }` (`services/edge/src/lib.rs:86-107`), built by `login_of` from the overworld's sea level (`bin/clustine/src/lib.rs:133-147`; `services/worldgen/src/terrain.rs:193-198`). The game sends a sea level per level, and `is_flat` only for a flat overworld.
- `SetDefaultSpawnPosition` names the overworld (`fanout.rs:4814-4815`).
- The clocks sent are the overworld's and the End's (`fanout.rs:4665-4679`), with the End's marked "inferred: Clustine has no End yet".
- **Registries:** every entry of every synchronised registry is sent by name without data (`services/edge/src/configuration.rs:51-66`). So all four dimension types and both world clocks already reach the client.
- `DimensionType` in `clustine-data` has only `name`, `min_y` and `height` (`crates/clustine-data/src/lib.rs:760-766`; `generated/dimension_types.rs:5-10`; emitted by `tools/datagen/src/emit.rs:1575-1585`).
- The protocol models `Login`, `DeathLocation` and `SetDefaultSpawnPosition` with dimension names (`crates/clustine-protocol/src/packets/play.rs:27-65`, `:456-464`).
- **There is no `Respawn` packet.** Only its id exists (`crates/clustine-protocol/src/generated/packet_ids.rs:233`, `:379`).
- The bots read a dimension type only from the login (`tools/botswarm/src/bot.rs:141-150`) and do not handle a Respawn.

### 1.6 A player's stay, saved position and hand-over

- `Place` (what the store keeps between stays) has pose, inventory, ender chest, experience, `respawn`, effects and the raid omen, but **no dimension** (`crates/clustine-sim/src/api.rs:89-107`).
- `Respawn` is `{pos, yaw, pitch}` with no dimension (`api.rs:118-124`). The game's respawn config names one.
- `PlayerTransfer` has no dimension (`api.rs:399-436`).
- `Entered { place, holder }` lets the home region pass a returning player straight to the region that holds their place (`api.rs:365-380`). With a place in the Nether, `holder` would have to be a Nether region, and one may not exist.
- `StayNote::Has` carries `Place` (`api.rs:135-160`). It is logged as `LoggedPlace`, with no dimension (`crates/clustine-format/src/log.rs:265-290`), and kept in `regions/players` (`crates/clustine-format/src/region.rs:356-380`; `docs/world-format.md:816-850`).
- `RegionConfig.lowest_y` turns a stored place below the floor into "no place" (`region.rs:152-155`). This is an overworld floor today.
- The edge's hand-over (`services/edge/src/fanout.rs:3222-3330`):
  - It unwants the old view in `from` and wants it in `to` (`:3303-3311`).
  - It sends `PlayerArrive` (`:3318`).
  - It **replays every kept input above `last_input`** to the new region (`:3291-3296`, then the loop after `:3318`). Across a dimension change those inputs carry old-dimension positions.
  - The path for a stay let go before the edge knew its entity sends the whole login (`fanout.rs:4705-4740`, `tell_entered` `:4767-4870`).

### 1.7 Persistence formats and version constants

| Constant | Value | Where | Why it changes |
|---|---|---|---|
| `FORMAT_VERSION` | 18 | `crates/clustine-format/src/lib.rs:63` | chunk path and table (grants by dimension, home per dimension?), `LoggedPlace` and the respawn point gain a dimension, arrivals and effects keyed by dimension. `docs/world-format.md:3` still says 16 (stale) |
| `STATE_FORMAT` | 24 | `services/worker/src/lib.rs:524` | `RegionState` and `StateDelta` gain the dimension and any per-player dimension fields (the test named at `:518-519` fails on a change of shape) |
| `ENTITIES_GENERATION` | 3 | `crates/clustine-sim/src/generation.rs:35` | the Nether's chunks spawn mobs (the End's do not: `services/worldgen/src/features.rs:263-268`); the structures' entities (bastion piglins, End city shulkers, the elytra frame) |
| `clustine_features::GENERATION` | 5 | `crates/clustine-features/src/lib.rs:37` | F7's batch |
| `STRUCTURES_GENERATION` | 1 | `crates/clustine-features/src/lib.rs:41` | the fortress, Nether fossil and End city placers |
| `FEATURES_VERSION` | 4 | `services/worldgen/src/features.rs:39` | any change in what a chunk comes out as |
| `TERRAIN_VERSION` | 2 | `services/worldgen/src/terrain.rs:48` | only if the overworld's terrain changes; a new dimension's generator has an identity of its own |
| `FLAT_VERSION` | 1 | `services/worldgen/src/lib.rs:88` | if the flat world gets a Nether and End |
| `clustine_light::VERSION` | 1 | `crates/clustine-light/src/lib.rs:64` | not needed if the skyless path is unchanged |
| The generator identity | — | `services/worldgen/src/features.rs:121-141` | names one dimension type; a three-dimension world changes the identity and so the world number |

### 1.8 Other hard-coded overworld values

- Mob navigation height limits are the overworld's (`crates/clustine-sim/src/mobs/ai/navigation.rs:362-370`; `random_pos.rs:174`).
- Several fallbacks assume -64 or 319 (`crates/clustine-sim/src/region/falling.rs:605`; `items.rs:152`; `boats.rs:646`).
- Worldgen: `TerrainGenerator::new` is the overworld only (`services/worldgen/src/terrain.rs:121-140`), as are `FeatureSettings::new(…, Dimension::Overworld)` and `makes_mobs(Dimension::Overworld)` (`services/worldgen/src/features.rs:78`, `:236`, `:241`).
- The single binary's `Land` holds one generator (`bin/clustine/src/lib.rs:77-80`, `:91-128`).

## 2. Terrain

**What exists: generic data, with per-dimension emitted code.** It is not a general interpreter of density functions at run time.

- Each dimension's noise router is transpiled to Rust by `cargo datagen`: `OverworldRouter`, `NetherRouter`, `EndRouter` (`crates/clustine-worldgen-data/src/lib.rs:1-20`, `:52-54`; `generated/{overworld,nether,end}/router.rs`). The terrain plan chose this, with tables as the fallback (`docs/groundwork/terrain-plan.md:591-596`).
- Material (surface) rules are emitted per dimension too (`generated/{nether,end}/material_rule.rs`).
- Carvers are data-driven (`clustine_worldgen_data::carvers`).
- Features are read from the data's trees at start-up, per dimension, with kinds implemented in code (`crates/clustine-features/src/settings.rs:65-82`). A test reads every feature of every dimension whole (`settings.rs:306-312`).
- Structures: hard-coded kinds plus jigsaw from the pools (`crates/clustine-worldgen-data/src/structure.rs:147-176`).

**Built for the Nether and the End:**

- **Biome source:**
  - the Nether by climate (`crates/clustine-terrain/src/biome/mod.rs:260`, `:294-297`);
  - the End by islands (`:198-226`, `:298`), with the End-islands density function (`crates/clustine-worldgen-data/src/density.rs:827-870`).
- **Fill, surface and carvers**, with the legacy random source (`crates/clustine-terrain/src/fill.rs:205`), judged on 5,600 and 4,780 chunks over three seeds and three dimensions (`docs/roadmap.md:2009-2042`; `docs/groundwork/terrain-plan.md:480-483`).
- **Structure starts:** all 52, including the fortress, bastion, Nether fossil and End city (`docs/roadmap.md:2047-2058`; `crates/clustine-structures/src/fortress.rs`, `end_city.rs`, `nether_fossil.rs`; `Structures::new(seed, dimension)` at `crates/clustine-structures/src/lib.rs:264`).
- **Spawn search:** overworld only, from the climate (`crates/clustine-terrain/src/spawn.rs`).

**Missing:**

- **Serving.** No `TerrainGenerator` or `FeatureStages` for another dimension (`services/worldgen/src/terrain.rs:121-140`; `features.rs:77-79`). The scheduler is dimension-generic (it reads `Stages::dimension`, `services/worldgen/src/stages.rs:15`). But its `has_sky` is wrong for the End (section 3).
- **Features (F7).** Twelve kinds are `Unbuilt`: they place nothing and claim success (`crates/clustine-features/src/features/mod.rs:19-45`; `dispatch.rs:34-86`):
  - `bonus_chest`, `chorus_plant`, `delta_feature`, `end_gateway`, `end_island`, `end_platform`;
  - `end_podium`, `end_spike`, `huge_fungus`, `netherrack_replace_blobs`, `random_neighbor_spread`, `void_start_platform`.
  - In the data these drive the Nether's basalt and blackstone blobs, deltas, glowstone, fungi and weeping vines, and the End's spikes, platform, chorus, islands and gateways.
  - The rest of the Nether's vegetation uses kinds already built: `overlay`, `block_column`, `simple_block`, `weighted_random_selector`.
  - The biome lists show what each biome orders: `worldgen/biome/the_end.json` has `end_spike` and `end_platform`; `end_highlands` has `end_gateway_return` and `chorus_plant`.
  - The feature judge is overworld-only (`services/worldgen/tests/game_features.rs:226`).
- **Structures (S3).**
  - Built placers cover jigsaw, so bastions build from their pools, and ruined portals, including the Nether's (`crates/clustine-features/src/structures/mod.rs:67-88`, `:96-140`).
  - The procedural fortress, the Nether fossil and the End city have no placer.
  - The S plan defers them to phase D (`docs/groundwork/s-plan.md:101`).
  - The natural spawner names the fortress's own list but does not ask it (`crates/clustine-sim/src/region/natural_spawning.rs:20`, `:722-728`).
- **The dragon fight's fixed features.** End gateways placed by the fight, the exit podium and the spikes' crystals: ADR-0021 lists "how the End's fixed features are driven" as an open item for phase D (`docs/adr/0021-generation-is-a-function.md:1361-1362`).
- **Where one stands.** No rule exists for the End's spawn platform or a Nether arrival. The terrain plan's D3 proposed Clustine's own rules for a world of one dimension (`docs/groundwork/terrain-plan.md:554`).

**Data in `target/datagen/26.3/generated/` that covers them:**

- `data/minecraft/worldgen/noise_settings/nether.json` and `end.json`. Both have `legacy_random_source: true` and 128 noise blocks of 256. The Nether's `default_fluid` is lava with sea level 32; the End's `disable_mob_generation` is true.
- `multi_noise_biome_source_parameter_list/nether.json`: only the preset; the values are in `reports/biome_parameters`.
- `density_function/nether/base_3d_noise.json`, `density_function/end/{base_3d_noise,islands,sloped_cheese}.json`.
- `carver/nether_cave.json`.
- The biome files of the five Nether and five End biomes.
- `feature/` (chorus, end_* features, gateways, podium, spike, platform, fungi, deltas).
- `structure/{fortress,bastion_remnant,nether_fossil,ruined_portal_nether,end_city}.json`.
- `structure_set/{nether_complexes,nether_fossils,end_cities}.json`; `template_pool/bastion`.
- `world_preset/normal.json`, which names the three dimensions' generators.
- `dimension_type/*.json`, `world_clock/the_end.json`, `tags/timeline/in_{nether,end}.json`.

## 3. Light

- `clustine_light::light(&Around { …, sky })` gives a dimension without a sky no sky light: every section is `Absent` (`crates/clustine-light/src/lib.rs:24-27`, `:74-82`).
  - This "was not compared" with an official Nether (`:26-27`).
  - The edge tests it (`services/edge/src/light.rs:957-990`, `a_dimension_without_a_sky_has_no_sky_light`).
- The simulation's incremental engine takes `Lighting::new(sky)` (`crates/clustine-light/src/engine.rs:156`). It lights only block light without a sky (`:273`), and reads 0 for sky light outside the world without a sky (`:456`).
- Callers:
  - the region always uses `Lighting::new(true)` (`crates/clustine-sim/src/region/light.rs:100`), and its oracle check passes `sky: true` (`:338`);
  - the edge always passes `sky: true` (`services/edge/src/fanout.rs:5476-5477`);
  - the flat and preview generators pass `true` (`services/worldgen/src/lib.rs:107`, `:215`; `terrain.rs:359`);
  - only the scheduler asks the dimension (`services/worldgen/src/scheduler/pool.rs:347`).
- **Defect:** `has_sky` returns false for both the Nether and the End (`pool.rs:463-467`, "the data here does not carry it yet"). The 26.3 data says the End has a sky: `"has_skylight": true` (`dimension_type/the_end.json:42`). The Nether has none (`the_nether.json:35`).
- The fallback `sky_light` over heights assumes a sky (`crates/clustine-world/src/light.rs:152-187`). The edge uses it for a chunk without light (`services/edge/src/fanout.rs:885-900`). In the Nether it would show full sky light under the bedrock roof.
- **Time and darkening.** `time::sky_light_level` and `sky_darken` follow the overworld's day timeline (`crates/clustine-sim/src/time.rs:1-45`).
  - In 26.3 the Nether's dimension type fixes `gameplay/sky_light_level` at 4.0 (`the_nether.json:13`), with `has_fixed_time` (`:34`) and only the universal timelines (`tags/timeline/in_nether.json`).
  - The End is `has_fixed_time` with `default_clock: the_end` (`the_end.json:38`, `:41`) and sets no `sky_light_level`. Its default value must be read from the jar.
  - The daylight detector's ticker exists only where the dimension has sky light (`docs/groundwork/n8-plan.md:243`).
- **What is needed:**
  - the dimension's `has_skylight` carried from the data;
  - `sky` per region and per edge replica entry, passed to every caller above;
  - the region's darkening taken from the dimension (fixed in the Nether and the End);
  - the fallback light made skyless where there is no sky;
  - a light comparison of a Nether chunk with the official server (`bin/clustine/tests/oracle.rs:1500` is the overworld's).

## 4. Rules that differ by dimension, today

In 26.3 the dimension type's old fields (`ultrawarm`, `natural`, `piglin_safe`, `bed_works`, `has_raids`) are **environment attributes**: `the_nether.json:3-28` and `overworld.json:3-45`. Clustine's `DimensionType` carries none of them (`crates/clustine-data/src/lib.rs:760-766`). The table maps the old names to the new ones.

| Rule (26.3 data) | Clustine today |
|---|---|
| Water placed in the Nether (`gameplay/water_evaporates`, `the_nether.json:20`) | Absent everywhere. "Water does not evaporate where it is dispensed, as Clustine has no dimension where it does" (`crates/clustine-sim/src/region/dispensers/behaviours.rs:29-30`); ice melting (`random_ticks/melting.rs:58-61`); a stalactite's fluid (`random_ticks/minerals.rs:523-526`). The bucket has no evaporation branch (`crates/clustine-sim/src/placement/bucket.rs`). No wet sponge code was found |
| Lava's speed and spread (`gameplay/fast_lava`, `the_nether.json:10`) | Fixed to the overworld: tick delay 30, drop-off 2, slope distance 2 (`crates/clustine-blocks/src/fluid.rs:11-14`, `:63-84`) |
| Beds (`gameplay/bed_rule`, `straw_bed_rule`: never sleep or set spawn, destroyed on use, `the_nether.json:4-8`, `the_end.json:20-24`) | Overworld rules are constants (`crates/clustine-sim/src/region/sleep.rs:6-9`, `:170-190`); no explosion |
| Respawn anchors (`respawn_anchor_works`, Nether only) | Only the dispenser's charge (`dispensers/behaviours.rs:1010`) and the comparator signal (`crates/clustine-blocks/src/signal.rs:273`); using one: absent |
| Compasses and clocks | Nothing server-side. A lodestone POI type exists (`crates/clustine-world/src/poi.rs:37`); lodestone tracking is absent. `DeathLocation` is modelled but always `None` (`fanout.rs:6950`). The spawn's dimension is hard-coded (`fanout.rs:4815`) |
| Fire | Infiniburn is fixed to `#infiniburn_overworld` (`crates/clustine-sim/src/region/random_ticks/fire.rs:49-51`), against the data's `#infiniburn_nether` and `#infiniburn_end` (`the_nether.json:37`, `the_end.json:44`). Fire in an obsidian frame makes no portal (`fire.rs:22-23`; `tnt.rs:17`) |
| Spawning tables by biome | Nether and End biome lists are in `clustine-data` (`crates/clustine-data/src/generated/spawns.rs:76`, `:210`, `:984`, `:1415`, `:1705`, `:503-525`, `:1289`, `:1636`). Their types are refused: "the types of the Nether's lists … are refused here" (`crates/clustine-sim/src/spawning/rules.rs:14-19`, `:210-212`). The monster light rule is fixed to the overworld: block limit 0, uniform 0-7 (`rules.rs:255-268`), against the Nether's 7 and 15 and the End's 15 and 0 (`the_nether.json:40-41`, `the_end.json:47-48`) |
| `natural` / `piglins_zombify` / `nether_portal_spawns_piglin` | Absent. Lightning turning a pig into a zombified piglin is named as missing (`crates/clustine-sim/src/mobs/types/pig.rs:8`) |
| `has_raids` → `gameplay/can_start_raid` false in the Nether (`the_nether.json:9`) | Raids start wherever an omen runs out. There is no dimension check (`crates/clustine-sim/src/region/raid_tick.rs`) |
| `snow_golem_melts` (Nether) | Biome list only, with "Clustine has no Nether" named (`crates/clustine-sim/src/mobs/types/snow_golem.rs:12`, `:47`, `:101`) |
| `coordinate_scale` (Nether 8, `the_nether.json:31`) | Absent (no portals) |
| `min_y` and `height`, `logical_height` (Nether 128) | `min_y`/`height` emitted; `logical_height` absent. The navigation limits are the overworld's (`navigation.rs:362-370`) |
| `ambient_light`, `cardinal_light`, `skybox`, visual attributes | Client-side, from the registry the client already has; nothing server-side needed |
| Time of day (`has_fixed_time`, `default_clock`, timelines) | One world clock of game time and the overworld's day (`crates/clustine-sim/src/api.rs:1308-1335`), kept per world in the table (`crates/clustine-format/src/region.rs:235-238`). The custom spawners (phantoms, patrols, cats, siege, trader) run in every region and choose from the world's player table (`crates/clustine-sim/src/region/custom_spawners.rs:1-40`); the game gives them to the overworld only (to be confirmed in the jar). Sleep counts every player of the world (`sleep.rs:1-5`; `api.rs:352-363`) |

## 5. Mechanics present today, and the gaps

**What can be reused:**

- **Teleports within a region:** the enderman's, chorus fruit's and ender pearl's `teleportTo` (`crates/clustine-sim/src/region/teleports.rs:1-17`, `:238-260`). A pearl's landing reaches its owner's region (`crates/clustine-sim/src/region/thrown.rs:22-34`).
- **Walking hand-overs**, with durability: `Departed` → edge → `PlayerArrive`, with inputs replayed (`api.rs:1017-1022`; `fanout.rs:3222-3330`). The login path also places a returning player in another region (`fanout.rs:4705-4740`).
- **Entity crossing as arrivals** filed in the store by chunk (`services/worldstore/src/arrivals.rs:1-25`).
- **The player list** spans every player of the world (`fanout.rs:4835-4867`), as the game's does across dimensions.
- **Points of interest:** the nether portal is a type derived from the block (`crates/clustine-world/src/poi.rs:36`, `:214-215`), so a portal index can be built on it. The published margin is only five chunks and in memory only (`services/worldstore/src/village.rs:1-30`). The game's portal search reaches 128 blocks in the overworld.
- **The client protocol:** the loading screen, game event 13 and `PlayerLoaded` are understood (`docs/protocol-26.3.md:98-105`). The edge ignores `PlayerLoaded` (`services/edge/src/play.rs:325`).

**Absent:**

- portals of any kind: "there are no portals yet" (`fire.rs:22-23`; `minecarts.rs:487`; `riding.rs:927-930` checks only the `portals` tag on dismount);
- the Respawn packet;
- any change of dimension;
- the Nether's and the End's mobs. The dispatch ends at the overworld's types (`crates/clustine-sim/src/mobs/types.rs:1392-1463`): no ghast, blaze, magma cube, wither skeleton, zombified piglin, piglin, piglin brute, hoglin, zoglin, strider, shulker, ender dragon, end crystal or wither;
- the dragon fight;
- End gateways;
- eyes of ender.

**What moving a player between dimensions needs:**

- **The edge:**
  - A Respawn packet, with the dimension type id, the name, the hashed seed, the sea level, `is_flat` and the data to keep.
  - Then the game's `ServerPlayer.teleport` sequence, from memory, **to be read from the jar before it is designed**: difficulty, abilities, position, level info (time, default spawn, game event 13), inventory and effects.
  - Then clear `sent`/`pending`/`replace`/`visible` without sending `UnloadChunk` or `RemoveEntities` for the old dimension: the client dropped its level with the Respawn.
  - Re-want the view in the new dimension.
  - Recompute the light job's `sky` per dimension.
  - Drop the kept inputs from the old dimension rather than replay them (`fanout.rs:3291-3296`). The client's moves before it confirms the new teleport are already ignored (`fanout.rs:76-82`).
  - Filter late `TickDelta` and snapshots of the old region. With a replica keyed by `ChunkPos` they would match chunks of the new view.
- **The regions:**
  - A travel that is a `Departed` to a region of another dimension. The region learns the target region from a store query in **another** dimension; `Claim` names only chunks (`messages.rs:427`).
  - What happens when no region holds the target: the store must create a region and the coordinator assign it, as the home region is made today (`docs/world-format.md:879-884`). Meanwhile the player waits up to the edge's patience, 20 s (`services/edge/src/lib.rs:117-119`).
  - Portal search and creation at the destination need the destination's chunks.
  - Entities through portals: an arrival filed in another dimension.
- **The store:**
  - dimension in `Place`, in the respawn point and in every chunk key;
  - a per-dimension home, or none;
  - a portal index per dimension;
  - mob caps, faces, the world-player list and sleep per dimension.

## 6. Suggested steps

The terrain plan already uses D1-D4 (`docs/groundwork/terrain-plan.md:548-555`), and F7 and S3 exist. The names below continue as P0 to P11 and say which earlier rows they absorb.

| # | What it changes | How it is tested | Delegable? |
|---|---|---|---|
| P0 | A brief from the jar, read only, as for N-phases: `ServerPlayer.teleport`/`TeleportTransition`, `Entity.teleport` across levels, `PortalShape`, `NetherPortalBlock`, `PortalForcer`, `EndPortalBlock`, `EndGatewayBlock`, `EndDragonFight`, `SpikeFeature`, the environment attributes' defaults, which spawners each `ServerLevel` has, the per-level caps and sleep. Plus a dump of the official server's packets on `/execute in minecraft:the_nether run tp …` | Review | yes (readers) |
| P1 | Shared types and data. A `Dimension` type in `clustine-world`, agreeing with `dimension_number` (`generation.rs:317-326`) and `clustine_terrain::biome::Dimension` (`crates/clustine-terrain/src/biome/mod.rs:63-95`). Datagen emits the whole dimension type plus the attributes the simulation reads (`tools/datagen/src/emit.rs:1575-1585`). Fixes `has_sky` (`pool.rs:463-467`) | `datagen --check`; a test that the generated table equals the JSON | after the type is fixed, yes |
| P2 | **The contracts (the lead's).** A dimension in `RegionInfo`, `RegionList`, `Restored`, `RegionRoute`, `PlayersOf`, `WorldPlayer`, `Place`, `Respawn`, `PlayerTransfer` and the travel variant of `Departed`. Store requests and answers for a chunk of another dimension. `RegionConfig.dimension`. Raises `STATE_FORMAT` and `FORMAT_VERSION`. Old worlds refused as at N1 and N6 (`docs/roadmap.md:131-133`, `:217-218`) unless the owner says otherwise | The byte-shape test (`services/worker/src/lib.rs:518-519`); format round trips | **no** |
| P3 | The store: chunks under `chunks/<dimension>/` (`chunks.rs:41-45`), table grants and every chunk-keyed map by dimension, a generator per dimension, `meta`, the world number, region creation in a dimension nobody holds, per-dimension publications (caps, faces, POIs, world players) | The store's unit tests; kill tests at every step (as in `services/worldstore/src/kill*.rs`); tests from the specification by someone else | partly. The creation and the kill tests are the lead's |
| P4 | Coordinator: never merge across dimensions; assign new regions; pins for the overworld only (`policy.rs:50`, `:161`) | Coordinator unit tests; `bin/clustine/tests/follows.rs` and `merges.rs` with players in two dimensions | yes, with a brief |
| P5 | Serving the land: `TerrainGenerator::new(seed, dimension)`, `FeatureStages` per dimension, sea level per dimension (`terrain.rs:121-140`, `:193-198`; `features.rs:77-79`), `makes_mobs` per dimension. F7's twelve kinds (absorbs F7) | The Java judges already cover fill, surface and carvers in all three dimensions; extend `services/worldgen/tests/game_features.rs` (overworld-only at `:226`) to the Nether and the End | yes: one subagent per batch |
| P6 | Structures: fortress, Nether fossil and End city placers; bastions checked (absorbs S3's remainder) | `game_structures.rs`, `game_bent.rs`, the feature judge with structures on | yes |
| P7 | **The edge (the lead's).** Replica and views keyed by dimension (`fanout.rs:825`, `:534-603`). Login names the stay's dimension with all three in `dimension_names`. `LoginWorld` per dimension (`services/edge/src/lib.rs:86-107`). The Respawn packet in `clustine-protocol`, and the change sequence. Inputs before the change dropped. Late reports of the old region filtered. `sky` per dimension. Bots decode Respawn (`tools/botswarm/src/bot.rs:141-150`) | `services/edge/src/script.rs` scenarios; `bin/clustine/tests/oracle.rs`'s join comparison (`:36`, `:80-95`) for a player saved in the Nether; a new comparison of the Respawn sequence; light comparison of a Nether chunk | **no** |
| P8 | Simulation rules by dimension: navigation heights (`navigation.rs:362-370`), sea level (`worker/src/lib.rs:1068`), infiniburn (`fire.rs:49-51`), fast lava (`fluid.rs:63-84`), evaporation (bucket, dispenser, ice, sponge), bed rules and the bed's explosion, respawn anchors, raids refused (`can_start_raid`), custom spawners in the overworld only, sleep counting the overworld's players, spawn light limits (`spawning/rules.rs:255-268`), skylight and darkening per dimension (`region/light.rs:100`; `time.rs`) | Unit tests from the specification by someone else; `placing_oracle.rs` scenes in the Nether (water evaporates, lava flows fast) | yes, per rule family, after P2 |
| P9 | **Nether portals and travel (the lead's for the travel).** Frame lighting, portal breaking, portal time and cooldown, the store-side portal index, search and creation at the destination, the hand-over to a region of another dimension, items and mobs through portals as arrivals in another dimension | End-to-end bots: build, light, step in, arrive, come back; kill every process at every step of a travel (as N3's 88 tests did, `docs/roadmap.md:157-167`); comparisons of the packets | the travel no; frame and block rules yes |
| P10 | The End: eyes of ender, the frame and the End portal, arrival on the platform (`end_platform` feature), the return; the gateways later | End-to-end bots; comparisons of the arrival | after P9 |
| P11 | The Nether's and the End's mobs, by families (ghast and fireballs, blaze, magma cube, wither skeleton, zombified piglin, piglins with their brain and bartering, hoglin and zoglin, strider, shulker and bullets), their spawning tables, and the structures' entities (raises `ENTITIES_GENERATION`). Then, if in scope, the dragon fight and the wither | Per-family tests from the specification; `mobs_oracle.rs`-style comparisons | yes, as in N7 and N9 |

**Order and risk.**

- P1 and P2 come first and are pushed before any subagent starts, since worktrees start from `origin/main` (`CLAUDE.md`, "Things that cost time before").
- These carry ordering risk and are not delegated:
  - P2's contracts;
  - P3's region creation and its interaction with take-overs;
  - all of P7;
  - P9's travel, where the durable `Departed`, the edge's Respawn and the arrival must be ordered as `enter_by_arrival` is (`fanout.rs:4697-4704`).
- P5 and P6 can run beside P3, P4 and P7, since they touch only the generation crates.
- Each step needs tests written from the specification by someone who did not write the code (`CLAUDE.md`).
- The comparisons can drive the official server's console (`tools/botswarm/src/oracle.rs:228`), so `/execute in …` teleports are available. Its seeded and flat worlds both have a normal Nether and End (`oracle.rs:43-53`).

## 7. Open questions for the owner

1. **One dimension per region, or regions that span dimensions?**
   - One per region keeps the simulation's chunk maps unchanged. But regions must be created for a dimension on demand, and every travel is a hand-over.
   - Spanning dimensions would reuse splits to make new regions, but every chunk-keyed map in `clustine-sim` changes.
   - The recommendation is one per region.
2. **Do a dimension's regions run on the same workers?** Should load or the coordinator decide, and should `--pin` apply only to the overworld?
3. **What does a world made before this phase become?** Refused and made again, as at N1 and N6, or opened with its chunks kept? `chunks/overworld/` already matches, but the table, players file, state and generator identity change, and so does the world number (`services/worldstore/src/lib.rs:845-853`).
4. **The flat world.** The game's flat world has a normal Nether and End of its seed. Should Clustine's flat world (seed 0, `bin/clustine/src/lib.rs:142-146`) get the Nether and End of seed 0, or none?
5. **Is the dragon fight, with its crystals, gateways, exit portal and egg, in scope of the first steps?** Is the wither?
6. **The terrain plan's D3**, a world of one dimension chosen when it is made (`docs/groundwork/terrain-plan.md:554`, owner's question 6 at `:659`). Is it still wanted as a stepping stone or for testing, or is it dropped for portals?
7. **Creative-only behaviour.** Players are creative and invulnerable. Should a bed's explosion in the Nether, a respawn anchor's spawn, death and the respawn screen (`enable_respawn_screen`, the Respawn packet's other use) wait for survival mode?
8. **Returning players.** Where does a player whose stay was in the Nether or the End enter when no region holds that chunk? A region made for them, or the overworld spawn? Is it acceptable to wait on the loading screen while a region is created and assigned, up to 20 s of patience?
9. **Portal search across regions.** A store-side index of portal points per dimension, kept durably, against reading destination chunks (they may be ungenerated or held elsewhere). How far may a travel wait for it?
10. **Per-dimension state.** Should mob caps, sleep, raids, the siege, patrols, phantoms and the trader follow the game's per-level rules? That means each dimension has its own caps and sleep counts only overworld players. Or should a named simplification stay?
11. **The clock.** The game shares game time across dimensions; Clustine keeps one world clock (`crates/clustine-format/src/region.rs:235-238`). Should what the edge sends per dimension be checked against the official server in the Nether and the End, since the End's clock is only inferred today (`fanout.rs:4668-4670`)?

### Critical files for implementation

- /home/claude/clustine/services/edge/src/fanout.rs
- /home/claude/clustine/services/worldstore/src/lib.rs (with `chunks.rs` and `table.rs` beside it)
- /home/claude/clustine/crates/clustine-sim/src/api.rs
- /home/claude/clustine/crates/clustine-rpc/src/messages.rs
- /home/claude/clustine/services/worldgen/src/features.rs (with `terrain.rs` and `scheduler/pool.rs`)