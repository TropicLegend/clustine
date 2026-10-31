//! `end_spike`: the ten pillars of obsidian round the End's arena, the two lowest but
//! one caged in iron bars, each with bedrock, fire and an end crystal on top
//! (`EndSpikeFeature`, `EndSpikeFeature$EndSpike` and `EndSpikeFeature$SpikeCacheLoader`,
//! read from `d-javap`; the offsets below are their bytecode's).
//!
//! The data's `end_spike` names no spikes, so the world's are worked out from its seed
//! (`getSpikesForLevel`), mortal and without a beam; the fight's respawn names its own,
//! which this reads as well.

use clustine_data::{BlockState, blocks};
use clustine_noise::{LegacyRandom, Random};
use clustine_protocol::nbt::Nbt;
use clustine_structures::place::PlacedEntity;
use clustine_world::BlockPos;

use crate::blocks::read_vec;
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

/// How many spikes the world has (`NUMBER_OF_SPIKES`).
pub const NUMBER_OF_SPIKES: usize = 10;
/// How far from the middle the spikes stand (`SPIKE_DISTANCE`).
pub const SPIKE_DISTANCE: f64 = 42.0;

/// One spike (`EndSpikeFeature$EndSpike`): its centre column, its radius, the y of its
/// top (the first block above the obsidian), and whether it is caged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spike {
    pub centre_x: i32,
    pub centre_z: i32,
    pub radius: i32,
    pub height: i32,
    pub guarded: bool,
}

impl Spike {
    /// `isCenterWithinChunk` (offsets 0-39): whether the centre is in the chunk of
    /// `origin`.
    pub fn is_centre_within_chunk(&self, origin: BlockPos) -> bool {
        origin.x >> 4 == self.centre_x >> 4 && origin.z >> 4 == self.centre_z >> 4
    }

    /// Where the crystal on top stands: the middle of the block above the spike
    /// (`placeSpike`, offsets 568-594).
    pub fn crystal_position(&self) -> [f64; 3] {
        [
            f64::from(self.centre_x) + 0.5,
            f64::from(self.height + 1),
            f64::from(self.centre_z) + 0.5,
        ]
    }

    /// Reads one spike as the codec does (`EndSpike.CODEC`): each member optional,
    /// nought or false where it is missing.
    fn read(reader: &Reader) -> Read<Self> {
        let int = |name: &str| -> Read<i32> {
            match reader.optional(name) {
                Some(value) => value.int(),
                None => Ok(0),
            }
        };
        let spike = Self {
            centre_x: int("centerX")?,
            centre_z: int("centerZ")?,
            radius: int("radius")?,
            height: int("height")?,
            guarded: match reader.optional("guarded") {
                Some(value) => value.boolean()?,
                None => false,
            },
        };
        reader.finish()?;
        Ok(spike)
    }
}

/// The spikes of the world of `seed` (`getSpikesForLevel`, offsets 0-36): the key is
/// the low sixteen bits of the first long of a `java.util.Random` of the seed
/// (`RandomSource.createThreadLocalInstance` makes a `SingleThreadedRandomSource`, the
/// same generator), which the game's cache turns into the spikes.
pub fn spikes_of_seed(seed: i64) -> [Spike; NUMBER_OF_SPIKES] {
    let key = LegacyRandom::from_seed(seed as u64).next_i64() & 0xFFFF;
    spikes_of_key(key)
}

/// `SpikeCacheLoader.load` (offsets 0-163): the numbers 0 to 9 shuffled by a
/// `java.util.Random` of `key` (`Util.toShuffledList`: from the last place down to the
/// second, each swapped with a place drawn below it), then spike `i` at the angle
/// `2(−π + πi/10)` on a circle of 42, its radius `2 + v/3`, its height `76 + 3v` and
/// caged where `v` is 1 or 2, `v` its shuffled number.
pub fn spikes_of_key(key: i64) -> [Spike; NUMBER_OF_SPIKES] {
    let mut random = LegacyRandom::from_seed(key as u64);
    let mut shuffled: [i32; NUMBER_OF_SPIKES] = std::array::from_fn(|i| i as i32);
    for size in (2..=NUMBER_OF_SPIKES).rev() {
        let drawn = random.next_i32_bounded(size as i32) as usize;
        shuffled.swap(size - 1, drawn);
    }
    std::array::from_fn(|i| {
        // `2.0 * (-PI + 0.3141592653589793 * i)`, as the bytecode orders it.
        let angle = 2.0 * (-std::f64::consts::PI + 0.314_159_265_358_979_3 * i as f64);
        let value = shuffled[i];
        Spike {
            centre_x: (SPIKE_DISTANCE * angle.cos()).floor() as i32,
            centre_z: (SPIKE_DISTANCE * angle.sin()).floor() as i32,
            radius: 2 + value / 3,
            height: 76 + value * 3,
            guarded: value == 1 || value == 2,
        }
    })
}

/// The iron bars of a cage at `(dx, dy, dz)` from a spike's top, joined along the
/// cage's walls and roof (`placeSpike`, offsets 235-465); none inside.
pub fn cage_bars(dx: i32, dy: i32, dz: i32) -> Option<BlockState> {
    let wall_x = dx.abs() == 2;
    let wall_z = dz.abs() == 2;
    let roof = dy == 3;
    if !(wall_x || wall_z || roof) {
        return None;
    }
    let along_x = dx == -2 || dx == 2 || roof;
    let along_z = dz == -2 || dz == 2 || roof;
    let flag = |value: bool| if value { "true" } else { "false" };
    let mut state = blocks::IRON_BARS;
    for (name, value) in [
        ("north", along_x && dz != -2),
        ("south", along_x && dz != 2),
        ("west", along_z && dx != -2),
        ("east", along_z && dx != 2),
    ] {
        state = state
            .with_property(name, flag(value))
            .expect("iron bars join on four sides");
    }
    Some(state)
}

/// The tag a spike's crystal is saved with as a run keeps it (`ProtoChunk.addEntity`
/// saves the entity it is given), as far as its load reads it: its type, position and
/// facing, `ShowBottom` as a new crystal has it, and the beam's target where it has
/// one (`EndCrystal.addAdditionalSaveData`, d-javap's EndCrystal.txt, offsets 0-27).
pub fn crystal_tag(position: [f64; 3], yaw: f32, beam_target: Option<BlockPos>) -> Nbt {
    let mut members = vec![
        (
            "id".to_owned(),
            Nbt::String("minecraft:end_crystal".to_owned()),
        ),
        (
            "Pos".to_owned(),
            Nbt::List(position.iter().map(|value| Nbt::Double(*value)).collect()),
        ),
        (
            "Rotation".to_owned(),
            Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]),
        ),
        ("ShowBottom".to_owned(), Nbt::Byte(1)),
    ];
    if let Some(target) = beam_target {
        members.push((
            "beam_target".to_owned(),
            Nbt::IntArray(vec![target.x, target.y, target.z]),
        ));
    }
    Nbt::Compound(members)
}

#[derive(Debug)]
pub struct EndSpike {
    /// The spikes the feature names; none, as the data's, for the world's own.
    spikes: Vec<Spike>,
    /// Whether the crystals cannot be hurt (`setPermanentlyInvulnerable`): false in
    /// the data. A crystal's record has no place for it yet, so it is read and not
    /// kept on the crystal; only the fight's respawn would set it.
    crystal_invulnerable: bool,
    crystal_beam_target: Option<BlockPos>,
}

impl EndSpike {
    /// The codec (`lambda$static$0`, offsets 0-79): `spikes`, `crystal_invulnerable`
    /// false unless said, `crystal_beam_target` optional.
    pub fn read(reader: &Reader) -> Read<Self> {
        let spikes = reader
            .field("spikes")?
            .list()?
            .iter()
            .map(Spike::read)
            .collect::<Read<Vec<_>>>()?;
        let crystal_invulnerable = match reader.optional("crystal_invulnerable") {
            Some(value) => value.boolean()?,
            None => false,
        };
        let crystal_beam_target = match reader.optional("crystal_beam_target") {
            Some(value) => {
                let (x, y, z) = read_vec(&value)?;
                Some(BlockPos::new(x, y, z))
            }
            None => None,
        };
        reader.finish()?;
        Ok(Self {
            spikes,
            crystal_invulnerable,
            crystal_beam_target,
        })
    }

    /// Whether the crystals are to be made invulnerable, which no record keeps yet.
    pub fn crystal_invulnerable(&self) -> bool {
        self.crystal_invulnerable
    }

    /// `placeSpike` (offsets 0-653).
    fn place_spike(&self, context: &mut Context<'_, '_>, spike: &Spike) {
        let radius = spike.radius;
        let (centre_x, centre_z, height) = (spike.centre_x, spike.centre_z, spike.height);
        // `BlockPos.betweenClosed` from the level's bottom to ten above the top: x
        // fastest, then y, then z. Obsidian within the radius (from the blocks' low
        // corners, `distToLowCornerSqr`) below the top, air elsewhere above y 65.
        let limit = f64::from(radius * radius + 1);
        for z in centre_z - radius..=centre_z + radius {
            for y in context.level.min_y()..=height + 10 {
                for x in centre_x - radius..=centre_x + radius {
                    let dx = f64::from(x - centre_x);
                    let dz = f64::from(z - centre_z);
                    let at = BlockPos::new(x, y, z);
                    if dx * dx + dz * dz <= limit && y < height {
                        context.level.set_block(at, blocks::OBSIDIAN);
                    } else if y > 65 {
                        context.level.set_block(at, blocks::AIR);
                    }
                }
            }
        }
        if spike.guarded {
            for dx in -2..=2 {
                for dz in -2..=2 {
                    for dy in 0..=3 {
                        if let Some(bars) = cage_bars(dx, dy, dz) {
                            let at = BlockPos::new(centre_x + dx, height + dy, centre_z + dz);
                            context.level.set_block(at, bars);
                        }
                    }
                }
            }
        }
        // The crystal (offsets 518-616), facing a yaw drawn from the feature's random,
        // kept by the run as the chunk keeps it: from its saved tag, which its load
        // reads back as a template's entity is read.
        let position = spike.crystal_position();
        let yaw = context.random.next_f32() * 360.0;
        let block = BlockPos::new(centre_x, height + 1, centre_z);
        let crystal = PlacedEntity {
            tag: crystal_tag(position, yaw, self.crystal_beam_target),
            block,
            position,
            yaw,
            pitch: 0.0,
            finalise: false,
        };
        context
            .level
            .make_template_entities(context.settings, vec![crystal]);
        // Bedrock under it and fire in its block (offsets 617-650). `FireBlock.getState`
        // over bedrock, which soul fire does not stand on and whose top is sturdy, is
        // fire's default state (`BaseFireBlock.getState` 0-38,
        // `FireBlock.getStateForPlacement` 0-40).
        context
            .level
            .set_block(block.offset(0, -1, 0), blocks::BEDROCK);
        context.level.set_block(block, blocks::FIRE);
    }

    /// The spikes this places: its own, or the world's where it names none
    /// (`place`, offsets 0-22).
    pub fn spikes(&self, seed: i64) -> Vec<Spike> {
        if self.spikes.is_empty() {
            spikes_of_seed(seed).to_vec()
        } else {
            self.spikes.clone()
        }
    }
}

impl Feature for EndSpike {
    /// `place` (offsets 0-75): each spike whose centre is in the origin's chunk.
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for spike in self.spikes(context.settings.seed()) {
            if spike.is_centre_within_chunk(origin) {
                self.place_spike(context, &spike);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::BlockState;
    use clustine_protocol::nbt::Nbt;
    use clustine_world::generation::Makings;
    use clustine_world::{BlockPos, ChunkPos};
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::context::Level;
    use crate::testing::{ToyMaker, World, settings};

    /// `java.util.Random`, written out here from its specification, so that the
    /// spikes are checked against the game's formula and not against the crate's own
    /// generator.
    struct JavaRandom(i64);

    impl JavaRandom {
        fn new(seed: i64) -> Self {
            Self((seed ^ 0x5DEE_CE66D) & ((1 << 48) - 1))
        }

        fn next(&mut self, bits: u32) -> i32 {
            self.0 = (self.0.wrapping_mul(0x5DEE_CE66D).wrapping_add(0xB)) & ((1 << 48) - 1);
            (self.0 >> (48 - bits)) as i32
        }

        fn next_long(&mut self) -> i64 {
            (i64::from(self.next(32)) << 32).wrapping_add(i64::from(self.next(32)))
        }

        fn next_int(&mut self, bound: i32) -> i32 {
            if bound & -bound == bound {
                return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
            }
            loop {
                let bits = self.next(31);
                let value = bits % bound;
                if bits - value + (bound - 1) >= 0 {
                    return value;
                }
            }
        }
    }

    /// The game's formula, from `getSpikesForLevel` and `SpikeCacheLoader.load`.
    fn formula(seed: i64) -> Vec<(i32, i32, i32, i32, bool)> {
        let key = JavaRandom::new(seed).next_long() & 0xFFFF;
        let mut random = JavaRandom::new(key);
        let mut list: Vec<i32> = (0..10).collect();
        let mut size = list.len();
        while size > 1 {
            let drawn = random.next_int(size as i32) as usize;
            let last = list[size - 1];
            let old = std::mem::replace(&mut list[drawn], last);
            list[size - 1] = old;
            size -= 1;
        }
        (0..10)
            .map(|i| {
                let angle = 2.0 * (-std::f64::consts::PI + 0.314_159_265_358_979_3 * f64::from(i));
                let v = list[i as usize];
                (
                    (42.0 * angle.cos()).floor() as i32,
                    (42.0 * angle.sin()).floor() as i32,
                    2 + v / 3,
                    76 + 3 * v,
                    v == 1 || v == 2,
                )
            })
            .collect()
    }

    #[test]
    fn the_spikes_stand_where_the_game_s_formula_puts_them_for_every_seed_tried() {
        for seed in [0, 1, 13579, -4_172_144_997_902_289_642, i64::MAX, i64::MIN] {
            let ours: Vec<_> = spikes_of_seed(seed)
                .iter()
                .map(|s| (s.centre_x, s.centre_z, s.radius, s.height, s.guarded))
                .collect();
            assert_eq!(ours, formula(seed), "seed {seed}");
        }
    }

    #[test]
    fn the_spikes_ring_the_middle_at_the_same_ten_places_whatever_the_seed() {
        let places: Vec<(i32, i32)> = spikes_of_seed(13579)
            .iter()
            .map(|spike| (spike.centre_x, spike.centre_z))
            .collect();
        assert_eq!(
            places,
            [
                (42, 0),
                (33, 24),
                (12, 39),
                (-13, 39),
                (-34, 24),
                (-42, -1),
                (-34, -25),
                (-13, -40),
                (12, -40),
                (33, -25),
            ]
        );
        // Every height from 76 to 103 by three is one spike's, and two are caged.
        let mut heights: Vec<i32> = spikes_of_seed(13579).iter().map(|s| s.height).collect();
        heights.sort_unstable();
        assert_eq!(heights, (0..10).map(|v| 76 + 3 * v).collect::<Vec<_>>());
        let caged = spikes_of_seed(13579).iter().filter(|s| s.guarded).count();
        assert_eq!(caged, 2);
        for spike in spikes_of_seed(13579) {
            assert_eq!(spike.radius, 2 + (spike.height - 76) / 3 / 3);
            assert_eq!(spike.guarded, [79, 82].contains(&spike.height));
        }
    }

    /// The configured feature with one spike of its own, centred in chunk (0, 0).
    fn one_spike(guarded: bool) -> EndSpike {
        EndSpike {
            spikes: vec![Spike {
                centre_x: 5,
                centre_z: 6,
                radius: 3,
                height: 79,
                guarded,
            }],
            crystal_invulnerable: false,
            crystal_beam_target: None,
        }
    }

    /// Places `feature` at chunk (0, 0)'s corner on end stone up to y 60, with a maker
    /// that keeps what it is handed.
    fn placed(feature: &EndSpike) -> World {
        let mut world = World::flat(blocks::END_STONE, 60);
        let maker = ToyMaker {
            mobs: &[],
            draws: false,
        };
        let settings = settings();
        let mut random = clustine_noise::WorldgenRandom::from_seed(7);
        {
            let mut level = Level::new(&mut world, settings.zoom(), 0, 256)
                .with_maker(&maker, Makings::new(2, ChunkPos::new(0, 0)));
            let mut context = Context {
                level: &mut level,
                random: &mut random,
                settings,
                top: None,
            };
            assert!(feature.place(&mut context, BlockPos::new(0, 0, 0)));
        }
        world
    }

    #[test]
    fn a_spike_is_obsidian_within_its_radius_below_its_top_and_air_round_it_above_65() {
        let world = placed(&one_spike(false));
        let (cx, cz) = (5, 6);
        for x in cx - 3..=cx + 3 {
            for z in cz - 3..=cz + 3 {
                let inside = (x - cx) * (x - cx) + (z - cz) * (z - cz) <= 10;
                for y in [0, 30, 60, 66, 78] {
                    let state = world.get(BlockPos::new(x, y, z));
                    if inside {
                        assert_eq!(state, blocks::OBSIDIAN, "{x} {y} {z}");
                    } else if y > 65 {
                        assert_eq!(state, blocks::AIR, "{x} {y} {z}");
                    } else {
                        assert_eq!(state, blocks::END_STONE, "{x} {y} {z}");
                    }
                }
            }
        }
        // Bedrock on the top's middle, fire over it, air beside them, nothing caged.
        assert_eq!(world.get(BlockPos::new(cx, 79, cz)), blocks::BEDROCK);
        assert_eq!(world.get(BlockPos::new(cx, 80, cz)), blocks::FIRE);
        assert_eq!(world.get(BlockPos::new(cx + 1, 79, cz)), blocks::AIR);
        assert_eq!(world.get(BlockPos::new(cx + 1, 78, cz)), blocks::OBSIDIAN);
        assert_eq!(world.get(BlockPos::new(cx, 89, cz)), blocks::AIR);
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| state.block_id() != blocks::IRON_BARS.block_id())
        );
    }

    #[test]
    fn a_caged_spike_is_walled_and_roofed_in_bars_joined_along_the_cage() {
        let world = placed(&one_spike(true));
        let (cx, h, cz) = (5, 79, 6);
        let bars = |dx: i32, dy: i32, dz: i32| world.get(BlockPos::new(cx + dx, h + dy, cz + dz));
        let joined = |state: BlockState| -> [bool; 4] {
            ["north", "south", "west", "east"].map(|name| state.property(name) == Some("true"))
        };
        // A wall's corner joins the two walls it ends; the middle of a wall joins along it.
        assert_eq!(joined(bars(-2, 0, -2)), [false, true, false, true]);
        assert_eq!(joined(bars(0, 1, -2)), [false, false, true, true]);
        assert_eq!(joined(bars(2, 2, 0)), [true, true, false, false]);
        // The roof joins every way but out of the cage.
        assert_eq!(joined(bars(0, 3, 0)), [true, true, true, true]);
        assert_eq!(joined(bars(2, 3, 2)), [true, false, true, false]);
        // Inside the walls below the roof: the bedrock, the fire and air.
        assert_eq!(bars(0, 0, 0), blocks::BEDROCK);
        assert_eq!(bars(0, 1, 0), blocks::FIRE);
        assert_eq!(bars(1, 2, -1), blocks::AIR);
        let count = world
            .writes
            .iter()
            .filter(|(_, state)| state.block_id() == blocks::IRON_BARS.block_id())
            .count();
        // Four layers of the ring of sixteen, and the roof's nine inside it.
        assert_eq!(count, 16 * 4 + 9);
    }

    #[test]
    fn the_crystal_stands_on_the_spike_mortal_without_a_beam_showing_its_base() {
        let world = placed(&one_spike(false));
        assert_eq!(world.made.len(), 1);
        let crystal = &world.made[0];
        assert_eq!(crystal.position, [5.5, 80.0, 6.5]);
        // Its yaw is the feature's first float.
        let mut random = clustine_noise::WorldgenRandom::from_seed(7);
        assert_eq!(crystal.yaw, random.next_f32() * 360.0);
        let (tag, finalise) = ToyMaker::handed(crystal);
        assert!(!finalise);
        let Ok(Some(Nbt::Compound(members))) = clustine_protocol::codec::Reader::new(tag).nbt()
        else {
            panic!("a compound");
        };
        let member = |name: &str| members.iter().find(|(key, _)| key == name).map(|m| &m.1);
        assert_eq!(
            member("id"),
            Some(&Nbt::String("minecraft:end_crystal".into()))
        );
        assert_eq!(member("ShowBottom"), Some(&Nbt::Byte(1)));
        assert_eq!(member("beam_target"), None);
    }

    #[test]
    fn the_data_s_spike_names_none_and_places_the_world_s_only_in_their_chunks() {
        let (_, entry) = clustine_worldgen_data::feature::configured("minecraft:end_spike")
            .expect("the data has the spikes");
        let feature = {
            let reader = Reader::new(&entry.data, entry.name);
            reader.kind().expect("a kind");
            EndSpike::read(&reader).expect("read")
        };
        assert!(feature.spikes.is_empty());
        assert!(!feature.crystal_invulnerable());
        assert_eq!(feature.crystal_beam_target, None);
        assert_eq!(feature.spikes(13579).len(), 10);
        // No spike's centre is in chunk (0, 0): the feature places nothing there.
        let world = placed(&feature);
        assert!(world.writes.is_empty());
        assert!(world.made.is_empty());
    }

    #[test]
    fn spikes_named_by_a_feature_are_read_with_its_beam() {
        static SPIKE: Data = Data::Object(&[
            ("centerX", Data::Number("3")),
            ("centerZ", Data::Number("-4")),
            ("radius", Data::Number("2")),
            ("height", Data::Number("85")),
            ("guarded", Data::Bool(true)),
        ]);
        static FEATURE: Data = Data::Object(&[
            ("spikes", Data::List(std::slice::from_ref(&SPIKE))),
            ("crystal_invulnerable", Data::Bool(true)),
            (
                "crystal_beam_target",
                Data::List(&[Data::Number("0"), Data::Number("128"), Data::Number("0")]),
            ),
        ]);
        let feature = EndSpike::read(&Reader::new(&FEATURE, "test")).expect("read");
        assert_eq!(
            feature.spikes,
            vec![Spike {
                centre_x: 3,
                centre_z: -4,
                radius: 2,
                height: 85,
                guarded: true,
            }]
        );
        assert!(feature.crystal_invulnerable());
        assert_eq!(feature.crystal_beam_target, Some(BlockPos::new(0, 128, 0)));
        let tag = crystal_tag([0.5, 1.0, 0.5], 0.0, feature.crystal_beam_target);
        let Nbt::Compound(members) = tag else {
            panic!("a compound")
        };
        assert!(members.contains(&("beam_target".into(), Nbt::IntArray(vec![0, 128, 0]))));
    }
}
