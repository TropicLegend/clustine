//! The making of a mob, the same for an egg and for generation
//! (`docs/groundwork/n7l-plan.md`, sections 2.1 and 9 item 11): its own draws at its
//! construction, its health, air and head, and `finalizeSpawn`, the type's, `Mob`'s,
//! then what the type does after its parent's. Free of any region: the egg hands it a
//! player's stream aside as the level's random, generation the run's or the spawn's.

use clustine_data::{attributes as ids, items};
use clustine_world::generation::Draws;
use clustine_world::{Entity, EntityId, ItemStack};
use uuid::Uuid;

use super::Mob;
use super::body::attributes::{ADD_MULTIPLIED_BASE, RANDOM_SPAWN_BONUS, supplied};
use super::body::{Head, Modifier, attribute};
use super::types::{self, Spawning};
use crate::living::slots;
use crate::region::Random;

/// A mob as `EntityType.create` makes it: its UUID and its constructor's yaw drawn from
/// `own`, its brain made (`construct`, its sensors' delays) and the type's constructor's
/// own draws, of its type's full health and air, its head and body turned to `yaw`.
pub(crate) fn construct(kind: u16, own: &mut Random, yaw: f32) -> (Uuid, Mob) {
    let uuid = crate::region::insecure_uuid(own.next_u64(), own.next_u64());
    let mob_type = types::of(kind);
    let mut mob = Mob::fresh();
    mob_type.define_synched_data(&mut mob, own);
    let _constructor_yaw = own.next_float();
    crate::mobs::brain::construct(mob_type, &mut mob, own);
    // `setHealth(getMaxHealth())` in the constructor, of its type's attributes.
    mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap_or(20.0) as f32;
    mob.body.vitals.air = mob_type.max_air();
    mob.body.head = Head {
        head_yaw: yaw,
        body_yaw: yaw,
        still_since: 0,
        ..Head::default()
    };
    (uuid, mob)
}

/// `finalizeSpawn` of a mob of the type `kind`: its type's, `Mob`'s own part, then what
/// its type does after its parent's.
pub(crate) fn finalize(kind: u16, spawning: &mut Spawning<'_>) {
    let mob_type = types::of(kind);
    mob_type.finalize_spawn(spawning);
    // A cave spider's `finalizeSpawn` returns before `Mob`'s.
    if mob_type.mob_finalize_spawn() {
        finalize_mob(kind, spawning.mob, spawning.level);
    }
    mob_type.after_finalize_spawn(spawning);
}

/// `Mob.finalizeSpawn`'s own part, after its type's: the follow range's bonus
/// (`random_spawn_bonus`, a triangle of ±0.11485 about 0 on the base, two `nextDouble`)
/// where it has none, and one hand in twenty the left (`nextFloat() < 0.05f`), drawn
/// from `level`.
pub(crate) fn finalize_mob(kind: u16, mob: &mut Mob, level: &mut dyn Draws) {
    let follow = attribute(ids::FOLLOW_RANGE);
    if let Some(instance) = mob.body.attributes.instance(kind, follow)
        && !instance.has_modifier(RANDOM_SPAWN_BONUS)
    {
        // `RandomSource.triangle(mode, deviation)`.
        let amount = 0.0 + 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        instance.add_modifier(Modifier {
            id: RANDOM_SPAWN_BONUS.to_owned(),
            amount,
            operation: ADD_MULTIPLIED_BASE,
        });
    }
    mob.body.flags.left_handed = level.next_float() < 0.05;
}

/// `getMoonBrightness()` at the day time `day`: the dimension's brightness of the moon's
/// phase, `(day / 24000) % 8` (`DimensionType.MOON_BRIGHTNESS_PER_PHASE`).
pub(crate) fn moon_brightness(day: u64) -> f32 {
    const PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
    PER_PHASE[((day / 24_000) % 8) as usize]
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// `Mob.EQUIPMENT_POPULATION_ORDER`: the armour's slots from the head down.
const POPULATION_ORDER: [u8; 4] = [slots::HEAD, slots::CHEST, slots::LEGS, slots::FEET];

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// `Mob.getEquipmentForSlot(slot, tier)` (Mob.txt 2430–2561): leather, copper, golden,
/// chainmail, iron and diamond by the tier, for the four armour slots.
fn equipment_for_slot(slot: u8, tier: u32) -> Option<i32> {
    let by_tier = match slot {
        slots::HEAD => [
            items::LEATHER_HELMET,
            items::COPPER_HELMET,
            items::GOLDEN_HELMET,
            items::CHAINMAIL_HELMET,
            items::IRON_HELMET,
            items::DIAMOND_HELMET,
        ],
        slots::CHEST => [
            items::LEATHER_CHESTPLATE,
            items::COPPER_CHESTPLATE,
            items::GOLDEN_CHESTPLATE,
            items::CHAINMAIL_CHESTPLATE,
            items::IRON_CHESTPLATE,
            items::DIAMOND_CHESTPLATE,
        ],
        slots::LEGS => [
            items::LEATHER_LEGGINGS,
            items::COPPER_LEGGINGS,
            items::GOLDEN_LEGGINGS,
            items::CHAINMAIL_LEGGINGS,
            items::IRON_LEGGINGS,
            items::DIAMOND_LEGGINGS,
        ],
        slots::FEET => [
            items::LEATHER_BOOTS,
            items::COPPER_BOOTS,
            items::GOLDEN_BOOTS,
            items::CHAINMAIL_BOOTS,
            items::IRON_BOOTS,
            items::DIAMOND_BOOTS,
        ],
        _ => return None,
    };
    by_tier.get(tier as usize).copied()
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// `Mob.populateDefaultEquipmentSlots(random, difficulty)` (Mob.txt 2347–2429), with
/// `special` the difficulty's special multiplier and `hard` whether the world's is hard:
/// one time in `0.15 * special` armour of a tier drawn (three more draws, each a step up),
/// from the head down, each slot after the first stopped at by a draw, an empty slot
/// filled. At easy `special` is 0: one draw, and nothing worn.
pub(crate) fn populate_default_equipment_slots(
    mob: &mut Mob,
    level: &mut dyn Draws,
    special: f32,
    hard: bool,
) {
    if level.next_float() >= 0.15 * special {
        return;
    }
    let mut tier = level.next_int(3);
    for _ in 0..3 {
        if level.next_float() < 0.1087 {
            tier += 1;
        }
    }
    let stop = if hard { 0.1 } else { 0.25 };
    let mut first = true;
    for slot in POPULATION_ORDER {
        if !first && level.next_float() < stop {
            break;
        }
        first = false;
        let worn = &mut mob.body.equipment.slots[usize::from(slot)];
        if worn.is_none()
            && let Some(item) = equipment_for_slot(slot, tier)
        {
            *worn = Some(ItemStack { item, count: 1 });
        }
    }
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// `Mob.enchantSpawnedEquipment(level, slot, random, chance, difficulty)`: where the slot
/// holds something, a draw against `chance * special`. **Named**: one that passes would
/// enchant the item by the provider `mob_spawn_equipment`, which Clustine has not; at the
/// world's difficulty, easy, `special` is 0 and none passes (`n7k-plan.md`, section 10).
pub(crate) fn enchant_spawned_equipment(
    mob: &Mob,
    slot: u8,
    level: &mut dyn Draws,
    chance: f32,
    special: f32,
) {
    if mob.body.equipment.slots[usize::from(slot)].is_none() {
        return;
    }
    let _enchanted = level.next_float() < chance * special;
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// `Mob.populateDefaultEquipmentEnchantments(level, random, difficulty)` (Mob.txt
/// 2562–2614): the main hand at 0.25, then each armour slot in `EquipmentSlot.VALUES`'s
/// order (feet, legs, chest, head) at 0.5. A type whose weapon's enchantment is its own
/// (the pillager's) calls [`enchant_spawned_equipment`] for the hand itself.
pub(crate) fn populate_default_equipment_enchantments(
    mob: &Mob,
    level: &mut dyn Draws,
    special: f32,
) {
    enchant_spawned_equipment(mob, slots::MAIN_HAND, level, 0.25, special);
    for slot in [slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD] {
        enchant_spawned_equipment(mob, slot, level, 0.5, special);
    }
}

/// A record of a fresh mob of the type `kind`, as `EntityType.create` and `snapTo` leave
/// it: at `position` facing `yaw`, of no age, riding nothing.
pub(crate) fn fresh_record(
    key: EntityId,
    kind: u16,
    uuid: Uuid,
    position: [f64; 3],
    yaw: f32,
    mob: &Mob,
) -> Entity {
    Entity {
        key,
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// A mob of the type `kind` made beside the one `spawning` finalises, as the game makes a
/// jockey's mount or rider in the middle of `finalizeSpawn` (a zombie's chicken, a
/// spider's skeleton): constructed of its own stream, at `position` facing `yaw`,
/// finalised from the same level's random with `reason` and a group of its own,
/// `prepare` done to it (a chicken made a jockey), and handed to the maker to add as
/// `seat` says. False, and nothing drawn for it, where the maker makes none (an egg's
/// mob made in another region's land: named, `n7k-plan.md`, section 10).
pub(crate) fn companion(
    spawning: &mut Spawning<'_>,
    kind: u16,
    position: [f64; 3],
    yaw: f32,
    (reason, seat): (types::SpawnReason, types::CompanionSeat),
    prepare: impl FnOnce(&mut Mob),
) -> bool {
    let Some(companions) = spawning.companions.as_deref_mut() else {
        return false;
    };
    let Some((key, mut own)) = companions.next() else {
        return false;
    };
    let (uuid, mut mob) = construct(kind, &mut own, yaw);
    let mut group = None;
    finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level: &mut *spawning.level,
            own: &mut own,
            biome: spawning.biome,
            at: crate::physics::containing(position),
            day: spawning.day,
            game_time: spawning.game_time,
            reason,
            group: &mut group,
            structure: spawning.structure,
            moon_brightness: spawning.moon_brightness,
            difficulty: spawning.difficulty,
            companions: None,
        },
    );
    prepare(&mut mob);
    let entity = fresh_record(key, kind, uuid, position, yaw, &mob);
    if let Some(companions) = spawning.companions.as_deref_mut() {
        companions.add(types::Companion {
            entity,
            seat,
            own,
            beside: spawning.mob.clone(),
        });
    }
    true
}

#[cfg(test)]
mod equipment_tests {
    use super::*;

    /// A stream of fixed floats and ints, recording each call.
    struct Scripted {
        floats: Vec<f32>,
        ints: Vec<u32>,
        calls: Vec<String>,
    }

    impl Draws for Scripted {
        fn next_int(&mut self, bound: u32) -> u32 {
            self.calls.push(format!("int {bound}"));
            self.ints.remove(0)
        }
        fn next_float(&mut self) -> f32 {
            self.calls.push("float".to_owned());
            self.floats.remove(0)
        }
        fn next_double(&mut self) -> f64 {
            unreachable!("no doubles")
        }
        fn next_bool(&mut self) -> bool {
            unreachable!("no booleans")
        }
    }

    /// A maker of companions that keeps what it is given.
    struct Kept {
        made: Vec<types::Companion>,
        keys: u64,
    }

    impl types::Companions for Kept {
        fn next(&mut self) -> Option<(EntityId, Random)> {
            self.keys += 1;
            let key = EntityId(100 + self.keys);
            Some((key, Random::of_entity(7, key, 0)))
        }

        fn add(&mut self, companion: types::Companion) {
            self.made.push(companion);
        }

        fn nearby(&self, _: &crate::physics::aabb::Aabb) -> Vec<(EntityId, u16, bool)> {
            Vec::new()
        }

        fn ride(&mut self, _: EntityId) {}
    }

    fn spawning_with<'a>(
        mob: &'a mut Mob,
        level: &'a mut Random,
        own: &'a mut Random,
        group: &'a mut Option<types::SpawnGroup>,
        companions: Option<&'a mut dyn types::Companions>,
    ) -> Spawning<'a> {
        Spawning {
            mob,
            level,
            own,
            biome: Some("minecraft:plains"),
            at: clustine_world::BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason: types::SpawnReason::SpawnEgg,
            group,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions,
        }
    }

    #[test]
    fn a_companion_is_made_of_its_own_stream_finalised_from_the_level_s_and_handed_over() {
        let chicken = clustine_data::entity_types::CHICKEN as u16;
        let mut kept = Kept {
            made: Vec::new(),
            keys: 0,
        };
        let mut mob = Mob::fresh();
        let mut level = Random::of_entity(1, EntityId(9), 0);
        let before = level.clone();
        let mut own = Random::of_entity(2, EntityId(9), 0);
        let mut group = None;
        let made = {
            let mut spawning =
                spawning_with(&mut mob, &mut level, &mut own, &mut group, Some(&mut kept));
            companion(
                &mut spawning,
                chicken,
                [1.5, 64.0, 2.5],
                90.0,
                (types::SpawnReason::Jockey, types::CompanionSeat::Vehicle),
                |mob| mob.body.flags.persistent = true,
            )
        };
        assert!(made);
        assert_eq!(kept.made.len(), 1);
        let companion = &kept.made[0];
        assert_eq!(companion.seat, types::CompanionSeat::Vehicle);
        assert_eq!(companion.entity.key, EntityId(101));
        assert_eq!(companion.entity.kind, chicken);
        assert_eq!(companion.entity.position, [1.5, 64.0, 2.5]);
        assert_eq!(companion.entity.yaw, 90.0);
        let decoded = Mob::decode(&companion.entity.data).unwrap();
        assert!(decoded.body.flags.persistent, "prepared after its making");
        // Its own stream moved past its constructor's draws, and the level's past its
        // `finalizeSpawn`'s.
        assert_ne!(companion.own, Random::of_entity(7, EntityId(101), 0));
        assert_ne!(level, before);
    }

    #[test]
    fn without_a_maker_no_companion_is_made_and_nothing_is_drawn() {
        let mut mob = Mob::fresh();
        let mut level = Random::of_entity(1, EntityId(9), 0);
        let before = level.clone();
        let mut own = Random::of_entity(2, EntityId(9), 0);
        let mut group = None;
        let mut spawning = spawning_with(&mut mob, &mut level, &mut own, &mut group, None);
        let chicken = clustine_data::entity_types::CHICKEN as u16;
        assert!(!companion(
            &mut spawning,
            chicken,
            [0.5, 64.0, 0.5],
            0.0,
            (types::SpawnReason::Jockey, types::CompanionSeat::Vehicle),
            |_| {},
        ));
        assert_eq!(level, before);
    }

    #[test]
    fn at_easy_one_draw_is_made_and_nothing_is_worn() {
        let mut mob = Mob::fresh();
        let mut level = Scripted {
            floats: vec![0.0],
            ints: vec![],
            calls: vec![],
        };
        populate_default_equipment_slots(&mut mob, &mut level, 0.0, false);
        assert_eq!(level.calls, ["float"]);
        assert!(mob.body.equipment.slots.iter().all(Option::is_none));
    }

    #[test]
    fn armour_of_a_stepped_up_tier_is_worn_from_the_head_down_until_a_stop() {
        let mut mob = Mob::fresh();
        // Passes, tier 1 (copper), one step up of three (golden); head, chest, then a
        // stop before the legs at normal's 0.25.
        let mut level = Scripted {
            floats: vec![0.1, 0.05, 0.5, 0.5, 0.9, 0.2],
            ints: vec![1],
            calls: vec![],
        };
        populate_default_equipment_slots(&mut mob, &mut level, 1.0, false);
        assert_eq!(
            level.calls,
            [
                "float", "int 3", "float", "float", "float", "float", "float"
            ]
        );
        let worn = |slot: u8| mob.body.equipment.slots[usize::from(slot)].map(|s| s.item);
        assert_eq!(worn(slots::HEAD), Some(items::GOLDEN_HELMET));
        assert_eq!(worn(slots::CHEST), Some(items::GOLDEN_CHESTPLATE));
        assert_eq!(worn(slots::LEGS), None);
        assert_eq!(worn(slots::FEET), None);
    }

    #[test]
    fn a_draw_is_made_for_each_filled_slot_s_enchantment_hand_first_then_feet_up() {
        let mut mob = Mob::fresh();
        mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] = Some(ItemStack {
            item: items::IRON_SWORD,
            count: 1,
        });
        mob.body.equipment.slots[usize::from(slots::HEAD)] = Some(ItemStack {
            item: items::CARVED_PUMPKIN,
            count: 1,
        });
        let mut level = Scripted {
            floats: vec![0.0, 0.0],
            ints: vec![],
            calls: vec![],
        };
        populate_default_equipment_enchantments(&mob, &mut level, 0.0);
        assert_eq!(level.calls, ["float", "float"]);
    }
}
