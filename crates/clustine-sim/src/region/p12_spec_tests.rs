//! Tests of step P12, the Nether's mobs, that need the region's insides, written from
//! the specification by someone who did not write the code (`docs/groundwork/p-plan.md`
//! sections 7 and 18, the review's changes holding; `docs/groundwork/p-brief.md`
//! sections 5.7 to 5.10; the game's bytecode where a rule's detail is wanted): what a
//! mob holds, the effects it has, the memories of its brain and its target, and the
//! spawning's tables. The rest of P12's tests, through the public interface, are in
//! `tests/p12_specification.rs`.
//!
//! A region of one dimension holds the chunks about the origin's, loaded, with a stone
//! floor at y = 40 (feet at 41). Player 1 stands near the origin, creative as every
//! player of Clustine is, and makes mobs with spawn eggs.

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_world::generation::FinalHeightmap;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkArea, ChunkPos, Dimension, Due, EdgeId, Effect, EffectId,
    Effected, Entity, EntityChange, EntityId, EntityIds, EntityUse, FiledEffect, ItemStack,
    PlayerId, Told, ToldWhat, Vec3,
};
use uuid::Uuid;

use crate::api::{
    EnderChest, Face, FaceChange, FaceSeq, HOTBAR_SLOTS, Inventory, Omens, Place, PlayerFace,
    PlayerInput, Pose, WorldClock,
};
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::mobs::brain::MemoryKind;
use crate::mobs::brain::memory::Value;
use crate::mobs::types;
use crate::{
    EdgeState, Entered, Holdings, PlayerChange, PlayerJoin, Region, RegionConfig, RegionState,
    TickInputs, TickOutput, Ticket,
};

const EDGE: EdgeId = EdgeId(7);
const FLOOR: i32 = 40;
const FEET: i32 = FLOOR + 1;

fn bp(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn stack(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("a type")
}

/// A box of `state` from `min` to `max`, both included.
fn filled(min: BlockPos, max: BlockPos, state: BlockState) -> Vec<(BlockPos, BlockState)> {
    let mut found = Vec::new();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                found.push((bp(x, y, z), state));
            }
        }
    }
    found
}

/// A region of `dimension` with `blocks` on the floor, player 1 at `feet` holding
/// `held` in the hotbar from slot 0, at midnight.
struct Scene {
    region: Region,
    me: EntityId,
    input: u64,
    sequence: i32,
    filed: u64,
}

impl Scene {
    fn new(
        dimension: Dimension,
        blocks: &[(BlockPos, BlockState)],
        held: &[i32],
        feet: Vec3,
    ) -> Self {
        let rules = dimension.rules();
        let mut inventory = Inventory::with_hotbar([const { None }; HOTBAR_SLOTS]);
        for (slot, item) in held.iter().enumerate() {
            inventory.set(slot, Some(stack(*item)));
        }
        let config = RegionConfig {
            homes: Vec::new(),
            dimension,
            spawn: Some(feet),
            starting_inventory: inventory.clone(),
            return_after: u64::MAX,
            lowest_y: rules.min_y,
            seed: 11,
            game_seed: 0,
            sea_level: if dimension == Dimension::Nether {
                32
            } else {
                63
            },
            spawn_mobs: false,
            light: Default::default(),
            features: Default::default(),
            random_tick_speed: 0,
        };
        let mut state = RegionState::new(EntityIds::block(1).expect("a block"));
        state.edges.insert(
            EDGE,
            EdgeState {
                start: 1,
                ..EdgeState::default()
            },
        );
        let around: Vec<ChunkPos> = (-2..=2)
            .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let held_chunks: Vec<ChunkPos> = (-3..=3)
            .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let holdings = Holdings {
            held: held_chunks,
            pinned: vec![ChunkArea::EVERYWHERE],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut region = Region::restore(config, state, holdings);
        region.tick(&TickInputs {
            tickets_added: around
                .iter()
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            clock: Some(WorldClock {
                game_time: 18_000,
                day: 18_000,
            }),
            ..TickInputs::default()
        });
        let chunk_of = |position: ChunkPos| {
            let mut chunk = Chunk::empty(rules, Biome(0));
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, FLOOR, z, blocks::STONE);
                }
            }
            for (at, state) in blocks {
                if at.chunk() == position {
                    chunk.set(
                        at.x.rem_euclid(16) as usize,
                        at.y,
                        at.z.rem_euclid(16) as usize,
                        *state,
                    );
                }
            }
            chunk
        };
        region.tick(&TickInputs {
            chunks_loaded: around.iter().map(|at| (*at, chunk_of(*at))).collect(),
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            player_changes: vec![PlayerChange::Join(
                EDGE,
                PlayerJoin {
                    player: player(1),
                    name: "Player1".to_owned(),
                    attempt: 7001,
                },
            )],
            ..TickInputs::default()
        });
        let entering = region
            .entering_state(player(1))
            .expect("the join is entering")
            .entity_id;
        region.tick(&TickInputs {
            entered: vec![Entered {
                unit: None,
                player: player(1),
                entity: entering,
                place: Some(Place {
                    dimension,
                    travel: None,
                    seen_credits: false,
                    portal_cooldown: 0,
                    respawn: None,
                    experience: Default::default(),
                    pose: Pose {
                        position: feet,
                        yaw: 0.0,
                        pitch: 0.0,
                        on_ground: true,
                    },
                    flying: false,
                    inventory,
                    selected_slot: 0,
                    ender: EnderChest::default(),
                    effects: Vec::new(),
                    time_since_rest: 0,
                    raid_omen_position: None,
                }),
                holder: None,
            }],
            ..TickInputs::default()
        });
        let me = region.player(player(1)).expect("player 1 entered").0;
        Self {
            region,
            me,
            input: 0,
            sequence: 0,
            filed: 0,
        }
    }

    fn nether(blocks: &[(BlockPos, BlockState)], held: &[i32]) -> Self {
        Self::new(
            Dimension::Nether,
            blocks,
            held,
            Vec3::new(0.5, f64::from(FEET), 0.5),
        )
    }

    fn input(&mut self, input: PlayerInput) -> TickOutput {
        self.input += 1;
        self.region.tick(&TickInputs {
            inputs: vec![(EDGE, player(1), self.me, self.input, input)],
            ..TickInputs::default()
        })
    }

    fn idle(&mut self) -> TickOutput {
        self.region.tick(&TickInputs::default())
    }

    fn idle_until(&mut self, ticks: u32, mut done: impl FnMut(&Scene) -> bool) -> Option<u32> {
        (1..=ticks).find(|_| {
            self.idle();
            done(self)
        })
    }

    fn record(&self, key: EntityId) -> Option<Entity> {
        self.region
            .find_entity(key)
            .map(|(_, entity)| entity.clone())
    }

    fn mob(&self, key: EntityId) -> Mob {
        Mob::decode(&self.record(key).expect("the mob is there").data).expect("a mob")
    }

    /// Writes the mob `key` back after `change`.
    fn change(&mut self, key: EntityId, change: impl FnOnce(&mut Mob)) {
        let (chunk, entity) = self.region.find_entity(key).expect("the mob is there");
        let mut entity = entity.clone();
        let mut mob = Mob::decode(&entity.data).expect("a mob");
        change(&mut mob);
        entity.data = mob.encode();
        self.region
            .chunks
            .get_mut(&chunk)
            .expect("loaded")
            .apply_entity(&EntityChange::Whole(entity));
    }

    fn all(&self) -> Vec<Entity> {
        self.region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter().cloned())
            .collect()
    }

    fn of_kind(&self, id: i32) -> Vec<Entity> {
        self.all()
            .into_iter()
            .filter(|entity| i32::from(entity.kind) == id)
            .collect()
    }

    /// Player 1 uses the egg in hotbar `slot` on the top of the floor at (x, z): the mob
    /// it made.
    fn hatch(&mut self, slot: u8, x: i32, z: i32) -> EntityId {
        let before: Vec<EntityId> = self.all().iter().map(|entity| entity.key).collect();
        self.input(PlayerInput::SelectSlot { slot });
        self.sequence += 1;
        let sequence = self.sequence;
        self.input(PlayerInput::UseItemOn {
            position: bp(x, FLOOR, z),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence,
        });
        let made: Vec<EntityId> = self
            .all()
            .iter()
            .map(|entity| entity.key)
            .filter(|key| !before.contains(key))
            .collect();
        assert_eq!(made.len(), 1, "the egg made one mob: {made:?}");
        made[0]
    }

    /// The memory `memory` of the brain-driven mob `key`.
    fn memory(&self, key: EntityId, memory: MemoryKind) -> Option<Value> {
        let entity = self.record(key)?;
        let provider = types::of(entity.kind).brain().expect("a mob with a brain");
        let mob = Mob::decode(&entity.data).expect("a mob");
        crate::mobs::brain::memories_of(provider, &mob, entity.lived).get(memory)
    }

    /// Ticks left of `effect` on the mob `key`, if it has it.
    fn effect_left(&self, key: EntityId, effect: MobEffect) -> Option<i64> {
        let entity = self.record(key)?;
        let mob = Mob::decode(&entity.data).expect("a mob");
        let active = mob
            .body
            .effects
            .active
            .iter()
            .find(|active| active.effect == effect)?;
        Some(active.top().duration(u64::from(entity.lived)))
    }

    fn file(&mut self, effects: Vec<Effect>) -> TickOutput {
        self.filed += 1;
        let sequence = 800_000 + self.filed;
        self.region.tick(&TickInputs {
            effects: effects
                .into_iter()
                .enumerate()
                .map(|(index, effect)| FiledEffect {
                    id: EffectId {
                        sequence,
                        index: u32::try_from(index).expect("few"),
                    },
                    effect,
                    due: Due::FromTaking,
                })
                .collect(),
            ..TickInputs::default()
        })
    }

    /// The face of survival player `n`, whose stay is `entity`, at `feet` wearing `worn`.
    fn face(&mut self, n: u128, entity: EntityId, feet: Vec3, worn: [Option<ItemStack>; 4]) {
        self.filed += 1;
        let face = PlayerFace {
            position: [feet.x, feet.y, feet.z],
            yaw: 0.0,
            pitch: 0.0,
            eye_height: 1.62,
            main_hand: None,
            off_hand: None,
            worn,
            sneaking: false,
            game_mode: 0,
            sleeping: None,
            vehicle: None,
            alive: true,
            last_hurt_mob: None,
            last_hurt_by: None,
            moved_horizontally: false,
            sprinting: false,
            omens: Omens::default(),
        };
        self.region.tick(&TickInputs {
            faces: vec![FaceChange::Face {
                player: player(n),
                entity,
                hops: 0,
                seq: FaceSeq(700_000, u32::try_from(self.filed).expect("few")),
                face,
            }],
            ..TickInputs::default()
        });
    }
}

// ------------------------------------------------------------------------------------
// The wither skeleton (brief 5.9; plan 18.3 E3; `WitherSkeleton`).

/// Brief 5.9 (`WitherSkeleton.populateDefaultEquipmentSlots`): a wither skeleton is made
/// with a stone sword in its main hand.
#[test]
fn a_wither_skeleton_is_made_with_a_stone_sword() {
    let mut scene = Scene::nether(&[], &[items::WITHER_SKELETON_SPAWN_EGG]);
    let skeleton = scene.hatch(0, 3, 0);
    let held = scene.mob(skeleton).body.equipment.slots[0].clone();
    assert_eq!(held.map(|stack| stack.item), Some(items::STONE_SWORD));
}

/// Brief 5.9 (`WitherSkeleton.registerGoals`, `doHurtTarget`): a wither skeleton takes
/// piglins as its targets, and its hit that takes gives wither for 200 ticks.
#[test]
fn a_wither_skeleton_attacks_a_piglin_and_its_hit_withers_it_for_200_ticks() {
    let mut scene = Scene::nether(
        &[],
        &[items::WITHER_SKELETON_SPAWN_EGG, items::PIGLIN_SPAWN_EGG],
    );
    let piglin = scene.hatch(1, 3, 3);
    scene.hatch(0, 3, -3);
    let withered = scene.idle_until(400, |scene| {
        scene.effect_left(piglin, MobEffect::WITHER).is_some()
    });
    assert!(withered.is_some(), "the piglin is hit and withered");
    let left = scene
        .effect_left(piglin, MobEffect::WITHER)
        .expect("wither");
    assert!((198..=200).contains(&left), "for 200 ticks: {left} left");
}

/// Brief 5.9 (`WitherSkeleton.getArrow`): a wither skeleton's arrows burn
/// (`igniteForSeconds(100)`, 2000 ticks): one holding a bow shoots burning arrows at a
/// piglin.
#[test]
fn a_wither_skeleton_with_a_bow_shoots_burning_arrows() {
    let mut scene = Scene::nether(
        &[],
        &[items::WITHER_SKELETON_SPAWN_EGG, items::PIGLIN_SPAWN_EGG],
    );
    let skeleton = scene.hatch(0, 3, -3);
    scene.change(skeleton, |mob| {
        mob.body.equipment.slots[0] = Some(stack(items::BOW));
    });
    // Out of melee reach: the piglin stands well away.
    scene.hatch(1, 0, 3);
    let shot = scene.idle_until(400, |scene| !scene.of_kind(entity_types::ARROW).is_empty());
    assert!(shot.is_some(), "the skeleton shoots");
    let arrow = scene.of_kind(entity_types::ARROW).remove(0);
    assert!(
        arrow.fire_ticks > 1_900,
        "the arrow burns for 100 seconds: {}",
        arrow.fire_ticks
    );
}

// ------------------------------------------------------------------------------------
// Conversions (plan 18.5 their 5; brief 5.7; `AbstractPiglin.finishConversion`,
// `Hoglin.finishConversion`).

/// Plan 18.5 (their 5) and 18.5's tests: a piglin that turns zombified in the overworld
/// keeps its equipment (`convertTo(…, ConversionParams.single(this, true, true), …)`)
/// and the zombified piglin is given nausea for 200 ticks; a hoglin's zoglin is given
/// nausea too.
#[test]
fn a_zombified_piglin_keeps_the_piglin_s_equipment_and_is_given_nausea() {
    let mut scene = Scene::new(
        Dimension::Overworld,
        &[],
        &[items::PIGLIN_SPAWN_EGG, items::HOGLIN_SPAWN_EGG],
        Vec3::new(0.5, f64::from(FEET), 0.5),
    );
    let piglin = scene.hatch(0, 3, 3);
    let hoglin = scene.hatch(1, -3, 3);
    scene.change(piglin, |mob| {
        mob.body.equipment.slots[0] = Some(stack(items::GOLDEN_SWORD));
        mob.body.equipment.slots[5] = Some(stack(items::GOLDEN_HELMET));
    });
    let turned = scene.idle_until(400, |scene| {
        scene.record(piglin).is_none() && scene.record(hoglin).is_none()
    });
    assert!(turned.is_some(), "both turn");
    let zombified = scene
        .of_kind(entity_types::ZOMBIFIED_PIGLIN)
        .first()
        .map(|entity| entity.key)
        .expect("a zombified piglin");
    let worn = scene.mob(zombified).body.equipment.slots.clone();
    assert_eq!(
        worn[0].as_ref().map(|stack| stack.item),
        Some(items::GOLDEN_SWORD)
    );
    assert_eq!(
        worn[5].as_ref().map(|stack| stack.item),
        Some(items::GOLDEN_HELMET)
    );
    let nausea = MobEffect::named("minecraft:nausea").expect("nausea");
    let left = scene.effect_left(zombified, nausea).expect("nausea");
    assert!((190..=200).contains(&left), "{left}");
    let zoglin = scene
        .of_kind(entity_types::ZOGLIN)
        .first()
        .map(|entity| entity.key)
        .expect("a zoglin");
    let left = scene
        .effect_left(zoglin, nausea)
        .expect("the zoglin's nausea");
    assert!((190..=200).contains(&left), "{left}");
}

// ------------------------------------------------------------------------------------
// Piglins' anger (brief 5.7; plan 18.5 their 7, 18.6; `PiglinAi.angerNearbyPiglins`,
// `setAngerTarget`).

/// Survival player 2's stay.
const TWO: EntityId = EntityId(900_002);

/// What a region filing a [`Told`] of player 2's anger sends the piglin `piglin`.
fn anger_of_two(scene: &Scene, piglin: EntityId, eyes: [f64; 3], only_if_seen: bool) -> Effect {
    let at = scene.record(piglin).expect("the piglin").position;
    Effect {
        at: bp(
            at[0].floor() as i32,
            at[1].floor() as i32,
            at[2].floor() as i32,
        ),
        what: Effected::OnEntity {
            key: piglin,
            by: EntityUse::Told(Told {
                what: ToldWhat::AngerAt { only_if_seen },
                of: TWO,
                kind: kind(entity_types::PLAYER),
                player: Some(player(2).0),
                eyes: eyes.map(clustine_world::Double),
            }),
        },
    }
}

fn golden_helmet() -> [Option<ItemStack>; 4] {
    [None, None, None, Some(stack(items::GOLDEN_HELMET))]
}

/// Plan 18.5 (their 7) and 18.6: a piglin is told across a line to be angry at a player
/// (a guarded block broken, a container opened where the player's region is): it is
/// angry at them (`ANGRY_AT`), though they wear gold; told so only where it sees them
/// (`onlyIfSeen`, a container opened), with a wall between them it is not.
#[test]
fn a_piglin_told_across_a_line_is_angry_at_the_player_where_it_must_and_can_see_them() {
    let wall = filled(bp(-3, FEET, 6), bp(9, FEET + 3, 6), blocks::STONE);
    for (only_if_seen, behind_the_wall, angry) in [
        (false, true, true),
        (true, false, true),
        (true, true, false),
    ] {
        let mut scene = Scene::nether(&wall, &[items::PIGLIN_SPAWN_EGG]);
        let piglin = scene.hatch(0, 3, 3);
        let feet = if behind_the_wall {
            Vec3::new(3.5, f64::from(FEET), 9.5)
        } else {
            Vec3::new(6.5, f64::from(FEET), 3.5)
        };
        scene.face(2, TWO, feet, golden_helmet());
        scene.idle();
        assert_eq!(
            scene.memory(piglin, MemoryKind::AngryAt),
            None,
            "not angry at a player in gold"
        );
        let eyes = [feet.x, feet.y + 1.62, feet.z];
        let told = anger_of_two(&scene, piglin, eyes, only_if_seen);
        scene.file(vec![told]);
        scene.idle();
        let now = scene.memory(piglin, MemoryKind::AngryAt);
        if angry {
            assert_eq!(
                now,
                Some(Value::Uuid(player(2).0.as_u128())),
                "only if seen {only_if_seen}, behind the wall {behind_the_wall}"
            );
        } else {
            assert_eq!(now, None, "it cannot see them behind the wall");
        }
    }
}

/// Brief 5.7 (`PiglinAi.setAngerTarget` asks `isEntityAttackableIgnoringLineOfSight`):
/// a creative player, as every player of Clustine is, angers no piglin by mining gold
/// beside it.
#[test]
fn a_creative_player_mining_gold_angers_no_piglin() {
    let gold = bp(2, FEET, 0);
    let mut scene = Scene::nether(&[(gold, blocks::GOLD_BLOCK)], &[items::PIGLIN_SPAWN_EGG]);
    let piglin = scene.hatch(0, 3, 3);
    scene.sequence += 1;
    let sequence = scene.sequence;
    scene.input(PlayerInput::Dig {
        position: gold,
        sequence,
    });
    assert_ne!(
        scene.region.chunks[&gold.chunk()]
            .get(2, FEET, 0)
            .expect("in the world"),
        blocks::GOLD_BLOCK,
        "mined"
    );
    scene.idle();
    assert_eq!(scene.memory(piglin, MemoryKind::AngryAt), None);
}

// ------------------------------------------------------------------------------------
// Zombified piglins (brief 5.8; `ZombifiedPiglin.alertOthers`).

/// Brief 5.8: a zombified piglin hurt by a mob is angry at it and alerts the others of
/// its kind within its follow range's box: a zoglin that attacks one has another, which
/// it never touched, take it as its target.
#[test]
fn a_zombified_piglin_s_anger_spreads_to_the_others_about_it() {
    let mut scene = Scene::nether(
        &[],
        &[
            items::ZOMBIFIED_PIGLIN_SPAWN_EGG,
            items::ZOMBIFIED_PIGLIN_SPAWN_EGG,
            items::ZOGLIN_SPAWN_EGG,
        ],
    );
    let near = scene.hatch(0, 3, 3);
    let far = scene.hatch(1, -3, -3);
    let zoglin = scene.hatch(2, 3, 2);
    let alerted = scene.idle_until(300, |scene| {
        scene.mob(far).body.combat.target == Some(zoglin)
    });
    assert!(alerted.is_some(), "the far one is alerted");
    assert_eq!(
        scene.mob(far).body.combat.last_hurt_by,
        None,
        "the far one was never hurt itself"
    );
    let hit_first = scene.mob(near).body.combat.last_hurt_by.map(|(by, _)| by);
    assert_eq!(
        hit_first,
        Some(zoglin),
        "the near one was hurt by the zoglin"
    );
}

// ------------------------------------------------------------------------------------
// The spawning's tables (plan 18.2, 18.5 their 8, 18.6).

/// Plan 18.5 (their 8): the ghast's cluster is 1 (`Ghast.getMaxSpawnClusterSize`).
#[test]
fn a_ghast_spawns_in_clusters_of_one() {
    assert_eq!(
        crate::spawning::rules::max_spawn_cluster_size(kind(entity_types::GHAST)),
        1
    );
    assert_eq!(
        crate::spawning::rules::max_spawn_cluster_size(kind(entity_types::ZOMBIFIED_PIGLIN)),
        4
    );
}

/// Plan 18.5 (their 8): the hoglin is counted by its class, an animal, not a monster.
#[test]
fn a_hoglin_is_counted_as_an_animal() {
    use crate::mobs::body::ladder::{Family, of};
    assert_eq!(of(kind(entity_types::HOGLIN)), Family::Animal);
    assert_eq!(of(kind(entity_types::ZOGLIN)), Family::Monster);
    assert_eq!(of(kind(entity_types::PIGLIN)), Family::Monster);
}

/// Plan 18.5 (their 8) and 18.6: the light the spawner reads lerps in the level's
/// ambient light (`DimensionType.ambientLight`, the Nether's 0.1):
/// `g + 0.1·(1 − g)` with `g = f / (4 − 3f)`, `f` the brightness over 15.
#[test]
fn the_nether_s_ambient_light_is_in_the_light_the_spawner_reads() {
    use crate::generation::spawning::magic_value;
    for brightness in [0u8, 7, 15] {
        let f = f32::from(brightness) / 15.0;
        let g = f / (4.0 - 3.0 * f);
        let nether = g + 0.1 * (1.0 - g);
        assert!(
            (magic_value(Dimension::Nether, brightness) - nether).abs() < 1.0e-6,
            "{brightness}"
        );
        assert!((magic_value(Dimension::Overworld, brightness) - g).abs() < 1.0e-6);
    }
    assert!((magic_value(Dimension::Nether, 0) - 0.1).abs() < 1.0e-6);
}

/// A column of the spawning's view: the floor at y = 40 and `at_feet` above it.
struct Column {
    at_feet: BlockState,
}

impl crate::generation::SpawnView for Column {
    fn position(&self) -> ChunkPos {
        ChunkPos::new(0, 0)
    }

    fn max_y(&self) -> i32 {
        255
    }

    fn sea_level(&self) -> i32 {
        32
    }

    fn dimension(&self) -> Dimension {
        Dimension::Nether
    }

    fn block(&self, at: BlockPos) -> BlockState {
        match at.y {
            FLOOR => blocks::STONE,
            FEET => self.at_feet,
            _ => blocks::AIR,
        }
    }

    fn height(&self, _: FinalHeightmap, _: i32, _: i32) -> i32 {
        FEET + 1
    }

    fn sky_light(&self, _: BlockPos) -> u8 {
        0
    }

    fn block_light(&self, _: BlockPos) -> u8 {
        0
    }

    fn zoomed_biome(&self, _: BlockPos) -> Biome {
        Biome(0)
    }

    fn noise_biome(&self, _: BlockPos) -> Biome {
        Biome(0)
    }
}

/// Plan 18.5 (their 8): the strider's walk target value is 10 in lava and 0 elsewhere
/// (`Strider.getWalkTargetValue` of a fresh strider).
#[test]
fn a_strider_values_lava_at_ten_and_anything_else_at_nothing() {
    use crate::generation::spawning::walk_target_value;
    let strider = kind(entity_types::STRIDER);
    let at = bp(0, FEET, 0);
    let lava = Column {
        at_feet: blocks::LAVA,
    };
    let air = Column {
        at_feet: blocks::AIR,
    };
    assert_eq!(walk_target_value(&lava, strider, at), 10.0);
    assert_eq!(walk_target_value(&air, strider, at), 0.0);
}

/// One position of a level for a spawn rule: lava from y = 41 up to `lava_top`, then
/// `above` over it.
struct LavaColumn {
    lava_top: i32,
    above: BlockState,
}

impl crate::spawning::rules::SpawnWorld for LavaColumn {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(if pos.y <= FLOOR {
            blocks::STONE
        } else if pos.y <= self.lava_top {
            blocks::LAVA
        } else if pos.y == self.lava_top + 1 {
            self.above
        } else {
            blocks::AIR
        })
    }

    fn light(&self, _: BlockPos) -> Option<crate::spawning::rules::Light> {
        Some(crate::spawning::rules::Light { sky: 0, block: 15 })
    }

    fn sky_darken(&self) -> i32 {
        11
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        Some(Biome(0))
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(self.lava_top + 1)
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        6_000
    }

    fn dimension(&self) -> Dimension {
        Dimension::Nether
    }
}

/// Answers every draw with its lowest.
struct Lowest;

impl clustine_world::generation::Draws for Lowest {
    fn next_int(&mut self, _: u32) -> u32 {
        0
    }

    fn next_float(&mut self) -> f32 {
        0.0
    }

    fn next_double(&mut self) -> f64 {
        0.0
    }

    fn next_bool(&mut self) -> bool {
        false
    }
}

/// Brief 5.9 (`Strider.checkStriderSpawnRules`): a strider spawns in lava where, going
/// up through the lava, the first block that is not lava is air; not where it is capped.
#[test]
fn a_strider_spawns_in_lava_open_to_the_air_and_not_under_a_cap() {
    let strider = kind(entity_types::STRIDER);
    let at = bp(0, FEET, 0);
    let open = LavaColumn {
        lava_top: FEET + 2,
        above: blocks::AIR,
    };
    let capped = LavaColumn {
        lava_top: FEET + 2,
        above: blocks::NETHERRACK,
    };
    assert!(crate::spawning::rules::check_spawn_rules(
        strider,
        &open,
        at,
        &mut Lowest
    ));
    assert!(!crate::spawning::rules::check_spawn_rules(
        strider,
        &capped,
        at,
        &mut Lowest
    ));
}

// ------------------------------------------------------------------------------------
// Piglins' targets (brief 5.7: `isWearingSafeArmor`; plan 18.3 E5; `PiglinBrute`).

/// Whether a piglin takes survival player 2, standing near it wearing `worn`, as its
/// target within 200 ticks.
fn piglin_targets_two(worn: [Option<ItemStack>; 4]) -> bool {
    let mut scene = Scene::nether(&[], &[items::PIGLIN_SPAWN_EGG]);
    let piglin = scene.hatch(0, 3, 3);
    scene.face(2, TWO, Vec3::new(-3.5, f64::from(FEET), -3.5), worn);
    scene
        .idle_until(200, |scene| {
            scene.memory(piglin, MemoryKind::AttackTarget) == Some(Value::Entity(TWO))
        })
        .is_some()
}

/// Brief 5.7 (`PiglinAi.findNearestValidAttackTarget`): a piglin takes a survival
/// player who wears no gold as its target.
#[test]
fn a_piglin_attacks_a_player_who_wears_no_gold() {
    assert!(piglin_targets_two([None, None, None, None]));
}

/// Brief 5.7 (`isWearingSafeArmor`, `#piglin_safe_armor` in any armour slot; the
/// sensor's `NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD`): a piglin leaves a player in a
/// golden helmet alone.
#[test]
#[ignore = "departs from the specification: a piglin takes a survival player wearing a \
            golden helmet as its target"]
fn a_piglin_leaves_a_player_in_gold_alone() {
    assert!(!piglin_targets_two(golden_helmet()));
}

/// Plan 18.3 E5 (`PiglinBrute.populateDefaultEquipmentSlots`): a piglin brute is made
/// with a golden axe.
#[test]
fn a_piglin_brute_is_made_with_a_golden_axe() {
    let mut scene = Scene::nether(&[], &[items::PIGLIN_BRUTE_SPAWN_EGG]);
    let brute = scene.hatch(0, 3, 0);
    let held = scene.mob(brute).body.equipment.slots[0].clone();
    assert_eq!(held.map(|stack| stack.item), Some(items::GOLDEN_AXE));
}

/// `PiglinBruteAi`: a piglin brute is not won over by gold: it takes a survival player
/// in a golden helmet as its target all the same.
#[test]
fn a_piglin_brute_attacks_a_player_even_in_gold() {
    let mut scene = Scene::nether(&[], &[items::PIGLIN_BRUTE_SPAWN_EGG]);
    let brute = scene.hatch(0, 3, 3);
    scene.face(
        2,
        TWO,
        Vec3::new(-3.5, f64::from(FEET), -3.5),
        golden_helmet(),
    );
    let targeted = scene.idle_until(200, |scene| {
        scene.memory(brute, MemoryKind::AttackTarget) == Some(Value::Entity(TWO))
    });
    assert!(targeted.is_some(), "the brute targets them");
}
