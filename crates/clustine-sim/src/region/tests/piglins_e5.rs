//! The piglins of family E5 of P12 in regions (`docs/groundwork/p-plan.md`, section
//! 18.5, its tests): a gold ingot admired for 119 ticks and bartered for one of the
//! table's outcomes, thrown towards the player it sees; a player's gift; zombification
//! in the overworld only, with the equipment kept and nausea after; and the anger of
//! gold mined by a player, told across a pinned line to the region that holds the
//! piglin, which angers it only at a player it may attack.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{
    Dimension, Effect, Effected, Entity, EntityChange, EntityData, EntityUse, ItemStack, ToldWhat,
};

use super::n9_world::*;
use super::thrown::{lay, mob};
use super::*;
use crate::api::{FaceChange, PlayerFace};
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect as MobEffect;
use crate::mobs::brain::{self, MemoryKind, Value};
use crate::mobs::making::{construct, fresh_record};
use crate::mobs::types::piglin::{self, PIGLIN, PIGLIN_BRUTE};
use crate::mobs::types::{piglin_ai, piglin_barter};
use crate::region::random_ticks::Random;

/// Player `number` standing on the floor at `x`, at the origin's z.
fn standing_in(region: &mut Region, number: u128, x: f64) {
    enter_with(
        region,
        number,
        entering_place(Vec3::new(x, -60.0, 0.5), Vec::new(), 0),
    );
}

/// The piglin of each test, and a brute.
const PIGLIN_KEY: EntityId = EntityId(0x5_0000_0010);
const BRUTE_KEY: EntityId = EntityId(0x5_0000_0011);

/// A gold ingot on the ground.
const INGOT_KEY: EntityId = EntityId(0x5_0000_0020);

/// A mob of the type `kind` made as an egg makes one (its constructor's draws, its brain
/// made), standing at `feet` and kept from despawning, holding `main` in its main hand.
fn made(key: EntityId, kind: u16, feet: [f64; 3], main: Option<ItemStack>) -> Entity {
    let mut own = Random::of_entity(SEED, key, 0);
    let (uuid, mut mob) = construct(kind, &mut own, 0.0);
    mob.body.equipment.slots[usize::from(crate::living::slots::MAIN_HAND)] = main;
    mob.body.flags.persistent = true;
    let mut record = fresh_record(key, kind, uuid, feet, 0.0, &mob);
    record.on_ground = true;
    record
}

fn piglin_at(feet: [f64; 3], main: Option<ItemStack>) -> Entity {
    made(PIGLIN_KEY, PIGLIN, feet, main)
}

/// A stack of `item` lying at `feet`, which may be picked up at once.
fn lying(key: EntityId, feet: [f64; 3], stack: ItemStack) -> Entity {
    Entity {
        portal: Default::default(),
        key,
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0) + 7),
        position: feet,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

/// Every entity of the region's loaded chunks.
fn records(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .collect()
}

/// The items lying in the region but `except`'s, with their records.
fn items_but(region: &Region, except: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(
            |entity| matches!(&entity.data, EntityData::Item { stack, .. } if stack.item != except),
        )
        .collect()
}

fn off_hand(region: &Region) -> Option<ItemStack> {
    piglin::off_hand(&mob(region, PIGLIN_KEY)).cloned()
}

/// The piglin's memories as of its last tick.
fn memories(region: &Region, key: EntityId, provider: &'static brain::Provider) -> brain::Memories {
    let (_, record) = region.find_entity(key).expect("the mob is here");
    brain::memories_of(provider, &mob(region, key), record.lived)
}

/// Whether `stack` is one the barter table can give.
fn a_table_outcome(stack: &ItemStack) -> bool {
    piglin_barter::ENTRIES.iter().any(|entry| {
        let item =
            if entry.item == items::BOOK && entry.function == piglin_barter::Function::SoulSpeed {
                items::ENCHANTED_BOOK
            } else {
                entry.item
            };
        let (low, high) = match entry.function {
            piglin_barter::Function::Count { min, max } => (min, max),
            _ => (1, 1),
        };
        item == stack.item && (low..=high).contains(&stack.count)
    })
}

/// Section 18.5's tests, and PiglinAi 499–563, 600–671 and 703–766: a gold ingot the
/// piglin touches is taken into its off hand and admired; admiring ends at the forget
/// pass of the 120th tick after (`setMemoryWithExpiry(.., 119)`), when it lets the
/// ingot go and throws one of the barter table's outcomes from 0.3 below its eyes
/// towards a block above the player it sees, at 0.3 each way.
#[test]
fn a_piglin_admires_a_gold_ingot_for_119_ticks_then_throws_a_barter_towards_the_player() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    let feet = [6.5, -60.0, 0.5];
    lay(&mut region, &piglin_at(feet, None));
    lay(
        &mut region,
        &lying(INGOT_KEY, feet, ItemStack::new(items::GOLD_INGOT, 1)),
    );
    let output = region.tick(&TickInputs::default());
    assert_eq!(
        off_hand(&region).map(|stack| stack.item),
        Some(items::GOLD_INGOT),
        "taken into its off hand"
    );
    assert!(
        region.find_entity(INGOT_KEY).is_none(),
        "the ingot is taken"
    );
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::ItemTaken { item, collector, .. } if *item == INGOT_KEY && *collector == PIGLIN_KEY)),
        "shown as taken"
    );
    assert!(
        memories(&region, PIGLIN_KEY, &piglin_ai::PROVIDER)
            .check(MemoryKind::AdmiringItem, brain::Status::Present)
    );
    for tick in 1..120 {
        region.tick(&TickInputs::default());
        assert_eq!(
            off_hand(&region).map(|stack| stack.item),
            Some(items::GOLD_INGOT),
            "still admired at tick {tick}"
        );
        assert!(
            items_but(&region, items::GOLD_INGOT).is_empty(),
            "tick {tick}"
        );
    }
    let before = mob(&region, PIGLIN_KEY);
    let at = region
        .find_entity(PIGLIN_KEY)
        .expect("the piglin")
        .1
        .position;
    region.tick(&TickInputs::default());
    assert_eq!(off_hand(&region), None, "the ingot is let go");
    let thrown = items_but(&region, items::GOLD_INGOT);
    assert_eq!(thrown.len(), 1, "one roll of one pool: {thrown:?}");
    let EntityData::Item { stack, .. } = &thrown[0].data else {
        unreachable!()
    };
    assert!(a_table_outcome(stack), "{stack:?} is of the table");
    // Thrown from where the piglin was at the throw towards the player at the origin's
    // column: its motion's way across points at them.
    let player_at = [0.5, -60.0, 0.5];
    let motion = thrown[0].motion;
    let way = [player_at[0] - at[0], player_at[2] - at[2]];
    let cross = motion[0] * way[1] - motion[2] * way[0];
    assert!(
        cross.abs() < 1e-6,
        "towards the player: {motion:?} along {way:?}"
    );
    assert!(
        motion[0] * way[0] + motion[2] * way[1] > 0.0,
        "towards, not away"
    );
    let _ = before;
}

/// PiglinAi 1029–1054 with `canAdmire`: a player's gift of a gold ingot is held and
/// admired, the player keeping theirs (creative), and bartered as one picked up; a
/// second gift while it admires is refused.
#[test]
fn a_gold_ingot_given_by_a_player_is_admired_and_a_second_refused_while_it_is() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    lay(&mut region, &piglin_at([2.5, -60.0, 0.5], None));
    hold(&mut region, 1, ItemStack::new(items::GOLD_INGOT, 3));
    let interact = PlayerInput::Interact {
        entity: PIGLIN_KEY,
        location: [0.0, 1.0, 0.0],
        sneaking: false,
    };
    acting(&mut region, 1, vec![interact.clone()]);
    assert_eq!(
        off_hand(&region).map(|stack| (stack.item, stack.count)),
        Some((items::GOLD_INGOT, 1))
    );
    assert!(
        memories(&region, PIGLIN_KEY, &piglin_ai::PROVIDER)
            .check(MemoryKind::AdmiringItem, brain::Status::Present)
    );
    // A second gift: it admires already, so nothing changes.
    let before = mob(&region, PIGLIN_KEY);
    acting(&mut region, 1, vec![interact]);
    assert_eq!(
        piglin::off_hand(&mob(&region, PIGLIN_KEY)).map(|stack| stack.count),
        piglin::off_hand(&before).map(|stack| stack.count)
    );
    // The first use came before the piglin's tick: admired through that tick and the
    // 118 after it (the second gift's among them), and let go in the 119th after, its
    // barter thrown (`MemorySlot`: 119 lowered to 0 by the 119 brain ticks from the
    // use's own, cleared at the next).
    let mut ticks = 1;
    while off_hand(&region).is_some() {
        region.tick(&TickInputs::default());
        ticks += 1;
        assert!(ticks <= 200, "never bartered");
    }
    assert_eq!(ticks, 119);
    let thrown = items_but(&region, items::GOLD_INGOT);
    assert_eq!(thrown.len(), 1);
    let EntityData::Item { stack, .. } = &thrown[0].data else {
        unreachable!()
    };
    assert!(a_table_outcome(stack), "{stack:?} is of the table");
}

/// A gold ingot thrown on the ground near a piglin that admires one already is not
/// wanted (`isNotHoldingLovedItemInOffHand`), and a stack of nuggets is taken whole
/// into the inventory of a piglin whose golden sword (a loved item) it does not give up
/// for them (`Piglin.canReplaceCurrentItem`, then `putInInventory`).
#[test]
fn a_piglin_takes_nuggets_whole_into_its_inventory_and_no_second_ingot_while_it_admires() {
    let mut region = alone(0, clock_at(0));
    let feet = [6.5, -60.0, 0.5];
    let sword = ItemStack::new(items::GOLDEN_SWORD, 1);
    lay(&mut region, &piglin_at(feet, Some(sword.clone())));
    lay(
        &mut region,
        &lying(INGOT_KEY, feet, ItemStack::new(items::GOLD_INGOT, 2)),
    );
    region.tick(&TickInputs::default());
    assert_eq!(off_hand(&region).map(|stack| stack.count), Some(1));
    let left = records(&region)
        .into_iter()
        .find(|entity| entity.key == INGOT_KEY)
        .expect("one ingot is left lying");
    assert!(matches!(&left.data, EntityData::Item { stack, .. } if stack.count == 1));
    let at = region
        .find_entity(PIGLIN_KEY)
        .expect("the piglin")
        .1
        .position;
    let nuggets = EntityId(0x5_0000_0021);
    lay(
        &mut region,
        &lying(nuggets, at, ItemStack::new(items::GOLD_NUGGET, 5)),
    );
    region.tick(&TickInputs::default());
    assert!(region.find_entity(nuggets).is_none(), "taken whole");
    assert_eq!(
        piglin::read(&mob(&region, PIGLIN_KEY)).inventory,
        vec![ItemStack::new(items::GOLD_NUGGET, 5)]
    );
    assert_eq!(
        mob(&region, PIGLIN_KEY).body.equipment.slots[0],
        Some(sword),
        "its sword kept"
    );
    assert!(
        region.find_entity(INGOT_KEY).is_some(),
        "the second ingot is not wanted while the first is admired"
    );
}

/// `AbstractPiglin.customServerAiStep` (127–259): where `piglins_zombify` holds (the
/// overworld), its time counts each AI step, and in the step it passes 300 it is heard
/// (its converted sound) and becomes a zombified piglin, which keeps its equipment and
/// is given nausea for 200 ticks (`Local::Convert`'s after).
#[test]
fn a_piglin_in_the_overworld_is_zombified_past_300_ticks_keeping_its_equipment_with_nausea() {
    let mut region = alone(0, clock_at(0));
    let mut record = piglin_at(
        [4.5, -60.0, 4.5],
        Some(ItemStack::new(items::GOLDEN_SWORD, 1)),
    );
    let mut body = Mob::decode(&record.data).expect("a mob");
    body.body.equipment.slots[usize::from(crate::living::slots::HEAD)] =
        Some(ItemStack::new(items::GOLDEN_HELMET, 1));
    record.data = body.encode();
    lay(&mut region, &record);
    for tick in 1..=300 {
        region.tick(&TickInputs::default());
        assert!(
            region.find_entity(PIGLIN_KEY).is_some(),
            "a piglin at {tick}"
        );
        assert_eq!(
            piglin::read(&mob(&region, PIGLIN_KEY)).time_in_overworld,
            tick
        );
    }
    let output = region.tick(&TickInputs::default());
    assert!(
        region.find_entity(PIGLIN_KEY).is_none(),
        "the piglin is gone"
    );
    let zombified = records(&region)
        .into_iter()
        .find(|entity| i32::from(entity.kind) == entity_types::ZOMBIFIED_PIGLIN)
        .expect("a zombified piglin is made");
    let made = Mob::decode(&zombified.data).expect("a mob");
    assert_eq!(
        made.body.equipment.slots[0]
            .as_ref()
            .map(|stack| stack.item),
        Some(items::GOLDEN_SWORD)
    );
    assert_eq!(
        made.body.equipment.slots[usize::from(crate::living::slots::HEAD)]
            .as_ref()
            .map(|stack| stack.item),
        Some(items::GOLDEN_HELMET)
    );
    let nausea = MobEffect::named("minecraft:nausea").expect("nausea");
    assert!(made.body.effects.has(nausea), "nausea after");
    assert!(
        output.events.iter().any(|event| matches!(
            event,
            RegionEvent::Sound { sound, .. } if *sound == sounds::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED
        )),
        "heard converting"
    );
}

/// An immune piglin (`IsImmuneToZombification`) is never converting.
#[test]
fn an_immune_piglin_counts_no_time_in_the_overworld() {
    let mut region = alone(0, clock_at(0));
    let mut record = piglin_at([4.5, -60.0, 4.5], None);
    let mut body = Mob::decode(&record.data).expect("a mob");
    piglin::update(&mut body, |own| own.immune = true);
    record.data = body.encode();
    lay(&mut region, &record);
    for _ in 0..320 {
        region.tick(&TickInputs::default());
    }
    let own = piglin::read(&mob(&region, PIGLIN_KEY));
    assert_eq!(own.time_in_overworld, 0);
    assert!(own.immune);
}

/// A region of the Nether, as `region/dimension_d2_tests.rs` makes one: the 5×5 chunks
/// about the origin with a stone floor three above its lowest block.
fn nether() -> (Region, f64) {
    let dimension = Dimension::Nether;
    let rules = dimension.rules();
    let floor_y = rules.min_y + 3;
    let biome = crate::spawning::rules::biome_named("minecraft:nether_wastes").expect("a biome");
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, biome);
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, floor_y, z, blocks::STONE);
                }
            }
            (*position, chunk)
        })
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let state = crate::state::RegionState::new(EntityIds::block(3).expect("a block of ids"));
    let mut region = Region::restore_loaded(
        RegionConfig {
            dimension,
            lowest_y: rules.min_y,
            sea_level: 32,
            return_after: u64::MAX,
            seed: SEED,
            ..config()
        },
        state,
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    );
    region.tick(&TickInputs::default());
    (region, f64::from(floor_y + 1))
}

/// Where `piglins_zombify` does not hold (the Nether), the time stays at nothing and the
/// piglin stays one.
#[test]
fn a_piglin_in_the_nether_is_never_zombified() {
    let (mut region, y) = nether();
    lay(&mut region, &piglin_at([4.5, y, 4.5], None));
    for _ in 0..400 {
        region.tick(&TickInputs::default());
    }
    assert!(region.find_entity(PIGLIN_KEY).is_some(), "still a piglin");
    assert_eq!(piglin::read(&mob(&region, PIGLIN_KEY)).time_in_overworld, 0);
    assert!(
        records(&region)
            .iter()
            .all(|entity| i32::from(entity.kind) != entity_types::ZOMBIFIED_PIGLIN)
    );
}

/// The face of player `number` as the store streams it, in the game mode `game_mode`.
fn face_in(number: u128, entity: EntityId, feet: [f64; 3], game_mode: u8) -> FaceChange {
    let FaceChange::Face {
        player,
        entity,
        hops,
        seq,
        face,
    } = face_of(number, entity, feet, 1)
    else {
        unreachable!()
    };
    FaceChange::Face {
        player,
        entity,
        hops,
        seq,
        face: PlayerFace { game_mode, ..face },
    }
}

/// What a region tick sent of a telling to the mob `key`.
fn told_to(output: &TickOutput, key: EntityId) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| matches!(&effect.what, Effected::OnEntity { key: to, by: EntityUse::Told(_) } if *to == key))
        .cloned()
        .collect()
}

/// The two regions of a pinned line at block x = 16: player 1 west of it, by a block of
/// gold, and a piglin and a brute of the east's within 16 of them, which the west sees
/// in its copy.
fn gold_by_the_line() -> (Region, Region, BlockPos) {
    let mut west = west(10, clock_at(0));
    let mut east = east(10, clock_at(0));
    standing_in(&mut west, 1, 14.5);
    let gold = BlockPos::new(13, -61, 0);
    let mut output = TickOutput::default();
    west.set_block(gold, blocks::GOLD_BLOCK, &mut output);
    let piglin = piglin_at([20.5, -60.0, 0.5], None);
    let brute = made(BRUTE_KEY, PIGLIN_BRUTE, [21.5, -60.0, 1.5], None);
    for record in [&piglin, &brute] {
        lay(&mut east, record);
        west.copies
            .get_mut(&ChunkPos::new(1, 0))
            .expect("the column east of the line is copied")
            .apply_entity(&EntityChange::Whole(record.clone()));
    }
    (west, east, gold)
}

/// PiglinAi 1005–1027 (`angerNearbyPiglins(level, player, false)`) from a player's break
/// of a block piglins guard: the piglin across the line, of the west's copy, is told by
/// an effect for its holder, and not the brute (`getEntitiesOfClass(Piglin.class)`).
/// The east takes the telling and angers the piglin at a player it may attack (one of
/// survival, as the face it keeps says), for 600 ticks; a creative player it may not
/// attack angers it not (`setAngerTarget`'s `isEntityAttackableIgnoringLineOfSight`).
#[test]
fn gold_mined_angers_a_piglin_across_the_line_at_a_player_it_may_attack_and_no_brute() {
    for game_mode in [0_u8, 1] {
        let (mut west, mut east, gold) = gold_by_the_line();
        let (entity, _) = west.player(player(1)).expect("player 1 is in the west");
        let dig = act(
            &west,
            1,
            PlayerInput::Dig {
                position: gold,
                sequence: 1,
            },
        );
        let output = west.tick(&moves(vec![dig]));
        assert_eq!(west.block(gold), Some(blocks::AIR), "the gold is broken");
        let sent = told_to(&output, PIGLIN_KEY);
        assert_eq!(sent.len(), 1, "{:?}", output.effects);
        assert!(told_to(&output, BRUTE_KEY).is_empty(), "no brute is told");
        let Effected::OnEntity {
            by: EntityUse::Told(told),
            ..
        } = &sent[0].what
        else {
            unreachable!()
        };
        assert_eq!(
            told.what,
            ToldWhat::AngerAt {
                only_if_seen: false
            }
        );
        assert_eq!(told.of, entity);
        assert_eq!(told.player, Some(player(1).0));

        east.tick(&TickInputs {
            faces: vec![face_in(1, entity, [14.5, -60.0, 0.5], game_mode)],
            effects: filed(sent, 1),
            ..TickInputs::default()
        });
        let angry = memories(&east, PIGLIN_KEY, &piglin_ai::PROVIDER).get(MemoryKind::AngryAt);
        if game_mode == 0 {
            assert_eq!(
                angry,
                Some(Value::Uuid(player(1).0.as_u128())),
                "angry at them"
            );
        } else {
            assert_eq!(angry, None, "a creative player is not attacked");
        }
        assert_eq!(
            memories(
                &east,
                BRUTE_KEY,
                &crate::mobs::types::piglin_brute::PROVIDER
            )
            .get(MemoryKind::AngryAt),
            None
        );
    }
}

/// A piglin that admires an item is not idle, and is not angered by gold mined
/// (`angerNearbyPiglins`' `isIdle`), judged where it is told.
#[test]
fn a_piglin_admiring_an_item_is_not_angered_by_gold_mined() {
    let (mut west, mut east, gold) = gold_by_the_line();
    let (entity, _) = west.player(player(1)).expect("player 1 is in the west");
    // The piglin admires: its brain is made to by a gift of the region's own.
    let mut record = east.find_entity(PIGLIN_KEY).expect("the piglin").1.clone();
    let mut body = Mob::decode(&record.data).expect("a mob");
    let mut memories_now = brain::memories_of(&piglin_ai::PROVIDER, &body, record.lived);
    memories_now.set_with_expiry(MemoryKind::AdmiringItem, Value::Bool(true), 119);
    brain::write_memories(&piglin_ai::PROVIDER, &mut body, memories_now);
    record.data = body.encode();
    lay(&mut east, &record);
    // A tick of its own makes admiring its activity.
    east.tick(&TickInputs::default());
    assert!(!piglin_ai::is_idle(&mob(&east, PIGLIN_KEY)));
    let dig = act(
        &west,
        1,
        PlayerInput::Dig {
            position: gold,
            sequence: 1,
        },
    );
    let output = west.tick(&moves(vec![dig]));
    let sent = told_to(&output, PIGLIN_KEY);
    east.tick(&TickInputs {
        faces: vec![face_in(1, entity, [14.5, -60.0, 0.5], 0)],
        effects: filed(sent, 1),
        ..TickInputs::default()
    });
    assert_eq!(
        memories(&east, PIGLIN_KEY, &piglin_ai::PROVIDER).get(MemoryKind::AngryAt),
        None
    );
}

/// PiglinAi 1073–1146 (`wasHurtBy`): a player's hit on a piglin that admires a gold
/// ingot ends its admiring and takes the ingot from it without a barter (an adult's
/// currency is neither worn nor kept), and disables admiring for 400 ticks, so that a
/// gold ingot given after is refused.
#[test]
fn a_player_s_hit_ends_the_admiring_without_a_barter_and_refuses_gold_after() {
    let mut region = alone(0, clock_at(0));
    standing_in(&mut region, 1, 0.5);
    lay(&mut region, &piglin_at([2.5, -60.0, 0.5], None));
    hold(&mut region, 1, ItemStack::new(items::GOLD_INGOT, 3));
    let interact = PlayerInput::Interact {
        entity: PIGLIN_KEY,
        location: [0.0, 1.0, 0.0],
        sneaking: false,
    };
    acting(&mut region, 1, vec![interact.clone()]);
    assert_eq!(
        off_hand(&region).map(|stack| stack.item),
        Some(items::GOLD_INGOT)
    );
    acting(
        &mut region,
        1,
        vec![PlayerInput::Attack { entity: PIGLIN_KEY }],
    );
    assert_eq!(off_hand(&region), None, "the ingot is gone");
    assert!(
        items_but(&region, -1).is_empty(),
        "nothing is thrown or dropped"
    );
    let memories_now = memories(&region, PIGLIN_KEY, &piglin_ai::PROVIDER);
    assert!(memories_now.check(MemoryKind::AdmiringDisabled, brain::Status::Present));
    assert!(!memories_now.check(MemoryKind::AdmiringItem, brain::Status::Present));
    assert_eq!(
        memories_now.get(MemoryKind::AngryAt),
        None,
        "a creative player"
    );
    acting(&mut region, 1, vec![interact]);
    assert_eq!(
        off_hand(&region),
        None,
        "a gift is refused while admiring is disabled"
    );
}

/// `broadcastAngerTarget`'s telling (`Local::Tell`): the region's own piglin told of
/// another's target takes it where it may attack it (`setAngerTargetIfCloserThanCurrent`)
/// at its next step; one only the region's copy shows is told through an effect for
/// its holder.
#[test]
fn a_piglin_told_of_another_s_target_is_angry_at_it_and_one_across_the_line_is_sent_it() {
    let mut region = alone(0, clock_at(0));
    let feet = [10.5, -60.0, 0.5];
    region.tick(&TickInputs {
        faces: vec![face_in(2, FAR_PLAYER, feet, 0)],
        ..TickInputs::default()
    });
    lay(&mut region, &piglin_at([4.5, -60.0, 0.5], None));
    let told = clustine_world::Told {
        what: ToldWhat::TargetOf,
        of: FAR_PLAYER,
        kind: entity_types::PLAYER as u16,
        player: Some(player(2).0),
        eyes: [
            clustine_world::Double(feet[0]),
            clustine_world::Double(feet[1] + 1.62),
            clustine_world::Double(feet[2]),
        ],
    };
    let mut output = TickOutput::default();
    region.apply_locals(
        BRUTE_KEY,
        vec![crate::mobs::tick::Local::Tell {
            key: PIGLIN_KEY,
            told: Box::new(told.clone()),
        }],
        &mut output,
    );
    assert_eq!(
        piglin::read(&mob(&region, PIGLIN_KEY)).pending.len(),
        1,
        "kept"
    );
    region.tick(&TickInputs::default());
    assert_eq!(
        memories(&region, PIGLIN_KEY, &piglin_ai::PROVIDER).get(MemoryKind::AngryAt),
        Some(Value::Uuid(player(2).0.as_u128()))
    );
    assert!(
        piglin::read(&mob(&region, PIGLIN_KEY)).pending.is_empty(),
        "taken"
    );

    let mut west = west(10, clock_at(0));
    west.copies
        .get_mut(&ChunkPos::new(1, 0))
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(piglin_at([20.5, -60.0, 0.5], None)));
    let mut output = TickOutput::default();
    west.apply_locals(
        BRUTE_KEY,
        vec![crate::mobs::tick::Local::Tell {
            key: PIGLIN_KEY,
            told: Box::new(told),
        }],
        &mut output,
    );
    let output = west.tick(&TickInputs::default());
    assert_eq!(
        told_to(&output, PIGLIN_KEY).len(),
        1,
        "{:?}",
        output.effects
    );
}

/// The stay of a survival player of another region, whose face a test's region keeps.
const FAR_PLAYER: EntityId = EntityId(0x9_0000_0001);

/// PiglinAi 155–191 and `CrossbowAttack` (n7): a piglin with a crossbow angry at a player
/// it may attack (of survival, a face the region keeps) takes them as its target, and
/// within its crossbow's range charges it (`IS_CHARGING_CROSSBOW` shown while it does),
/// waits, and shoots an arrow.
#[test]
fn a_piglin_with_a_crossbow_angry_at_a_player_charges_and_shoots_an_arrow() {
    let mut region = alone(0, clock_at(0));
    let feet = [10.5, -60.0, 0.5];
    region.tick(&TickInputs {
        faces: vec![face_in(2, FAR_PLAYER, feet, 0)],
        ..TickInputs::default()
    });
    let mut record = piglin_at([4.5, -60.0, 0.5], Some(ItemStack::new(items::CROSSBOW, 1)));
    let mut body = Mob::decode(&record.data).expect("a mob");
    let mut angry = brain::memories_of(&piglin_ai::PROVIDER, &body, 0);
    angry.set_with_expiry(
        MemoryKind::AngryAt,
        Value::Uuid(player(2).0.as_u128()),
        piglin_ai::ANGER_DURATION,
    );
    brain::write_memories(&piglin_ai::PROVIDER, &mut body, angry);
    record.data = body.encode();
    lay(&mut region, &record);
    let mut charged = false;
    let mut shot = None;
    for tick in 1..=120 {
        region.tick(&TickInputs::default());
        charged |= piglin::read(&mob(&region, PIGLIN_KEY)).charging;
        if records(&region)
            .iter()
            .any(|entity| matches!(entity.data, EntityData::Arrow(_)))
        {
            shot = Some(tick);
            break;
        }
    }
    assert_eq!(
        memories(&region, PIGLIN_KEY, &piglin_ai::PROVIDER).get(MemoryKind::AttackTarget),
        Some(Value::Entity(FAR_PLAYER)),
        "its target"
    );
    assert!(charged, "seen charging");
    let shot = shot.expect("an arrow is shot");
    // Charged over 25 ticks of its use and waiting 20 to 39 after: past 45 at least.
    assert!(shot > 45, "shot at {shot}");
}

/// A crowd of the Nether's piglins (with a crossbow, a sword, a baby) and a brute,
/// beside a soul torch they shun, a gold ingot and porkchops on the ground, live a
/// minute of their brains among each other: none is lost, nor converted, the ingot is
/// taken, the brute keeps its axe, and none is angry at anything.
#[test]
fn piglins_and_a_brute_live_a_minute_together_in_the_nether() {
    let (mut region, y) = nether();
    let at = |x: f64, z: f64| [x, y, z];
    let floor = y as i32 - 1;
    let mut output = TickOutput::default();
    region.set_block(
        BlockPos::new(2, floor + 1, 9),
        blocks::SOUL_TORCH,
        &mut output,
    );
    let crowd = [
        made(
            PIGLIN_KEY,
            PIGLIN,
            at(4.5, 4.5),
            Some(ItemStack::new(items::CROSSBOW, 1)),
        ),
        made(
            EntityId(0x5_0000_0012),
            PIGLIN,
            at(6.5, 4.5),
            Some(ItemStack::new(items::GOLDEN_SWORD, 1)),
        ),
        made(
            BRUTE_KEY,
            PIGLIN_BRUTE,
            at(4.5, 7.5),
            Some(ItemStack::new(items::GOLDEN_AXE, 1)),
        ),
    ];
    for record in &crowd {
        lay(&mut region, record);
    }
    let mut baby = made(EntityId(0x5_0000_0013), PIGLIN, at(7.5, 7.5), None);
    let mut body = Mob::decode(&baby.data).expect("a mob");
    piglin::set_baby(&mut body, true);
    baby.data = body.encode();
    lay(&mut region, &baby);
    lay(
        &mut region,
        &lying(
            INGOT_KEY,
            at(5.5, 2.5),
            ItemStack::new(items::GOLD_INGOT, 1),
        ),
    );
    lay(
        &mut region,
        &lying(
            EntityId(0x5_0000_0022),
            at(8.5, 2.5),
            ItemStack::new(items::PORKCHOP, 2),
        ),
    );
    for _ in 0..1200 {
        region.tick(&TickInputs::default());
    }
    for key in [PIGLIN_KEY, EntityId(0x5_0000_0012), EntityId(0x5_0000_0013)] {
        assert!(
            region.find_entity(key).is_some(),
            "{key:?} is a piglin still"
        );
        assert_eq!(
            memories(&region, key, &piglin_ai::PROVIDER).get(MemoryKind::AngryAt),
            None
        );
    }
    assert_eq!(
        mob(&region, BRUTE_KEY).body.equipment.slots[0]
            .as_ref()
            .map(|stack| stack.item),
        Some(items::GOLDEN_AXE)
    );
    assert!(
        region.find_entity(INGOT_KEY).is_none(),
        "the ingot is taken"
    );
    assert!(
        records(&region)
            .iter()
            .all(|entity| i32::from(entity.kind) != entity_types::ZOMBIFIED_PIGLIN),
        "none converts in the Nether"
    );
}
