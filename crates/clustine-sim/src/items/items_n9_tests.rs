//! Tests of the items' rules in a hand (`docs/groundwork/n9-plan.md`, sections 4.4, 4.6,
//! 4.7 and 4.10, as section 18 amends them), written from the game's classes (n9
//! BowItem, CrossbowItem, TridentItem, ProjectileWeaponItem, the throwables' items,
//! FireworkRocketItem, EnchantmentHelper) and the enchantments' data files: first the
//! pure rules of `items/`, then a player's use of each through a region
//! (`region/throws.rs`), driven by its public inputs alone.
//!
//! What is made is tested as it is made (the spawn's state), not what it does after:
//! the arrows' and the thrown kinds' flight is other parts'.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::components::{Component, Kind, PotionContents};
use clustine_world::{
    Biome, Chunk, ChunkArea, ChunkPos, EdgeId, EntityData, EntityId, EntityIds, Pickup, PlayerId,
    Section, Vec3,
};

use super::{bow, crossbow, enchantments, rockets, throwables, trident};
use crate::api::{EntityKind, EntityState, Face, Inventory, ItemStack, PlayerInput, RegionEvent};
use crate::physics::thrown as flight;
use crate::region::Random;
use crate::{
    EdgeEvent, Entered, Holdings, PlayerChange, PlayerEvent, PlayerJoin, Region, RegionConfig,
    TickInputs, TickOutput, Ticket,
};

fn enchanted(item: i32, enchantments: &[(&str, i32)]) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack.components.set(Component::Enchantments(
        enchantments
            .iter()
            .map(|(name, level)| ((*name).to_owned(), *level))
            .collect(),
    ));
    stack
}

fn intangible(stack: &ItemStack) -> bool {
    stack.components.get(Kind::IntangibleProjectile).is_some()
}

// The bow and `ProjectileWeaponItem`.

#[test]
fn the_bows_power_rises_with_the_ticks_held_and_is_full_at_twenty() {
    // `f = t / 20; (f² + 2f) / 3`, at most 1.
    assert_eq!(bow::power_for_time(5), (0.25_f32 * 0.25 + 0.25 * 2.0) / 3.0);
    assert_eq!(bow::power_for_time(10), (0.5_f32 * 0.5 + 0.5 * 2.0) / 3.0);
    assert_eq!(bow::power_for_time(20), 1.0);
    assert_eq!(bow::power_for_time(100), 1.0);
}

#[test]
fn a_bow_let_go_before_three_ticks_looses_nothing() {
    // 0.07 at two ticks, 0.1075 at three: below a tenth nothing is loosed.
    assert_eq!(bow::release_power(0), None);
    assert_eq!(bow::release_power(2), None);
    assert_eq!(bow::release_power(3), Some(bow::power_for_time(3)));
}

#[test]
fn the_bows_sound_is_pitched_by_its_draw_and_its_power() {
    assert_eq!(bow::shoot_pitch(0.5, 1.0), 1.0 / (0.5 * 0.4 + 1.2) + 0.5);
    assert_eq!(bow::shoot_pitch(0.0, 0.5), 1.0 / 1.2 + 0.25);
}

#[test]
fn a_weapon_fires_arrows_from_anywhere_and_a_crossbow_rockets_from_a_hand() {
    for arrow in [items::ARROW, items::TIPPED_ARROW, items::SPECTRAL_ARROW] {
        assert!(bow::fires_from_hand(items::BOW, arrow));
        assert!(bow::fires_from_inventory(arrow));
    }
    assert!(!bow::fires_from_hand(items::BOW, items::FIREWORK_ROCKET));
    assert!(bow::fires_from_hand(
        items::CROSSBOW,
        items::FIREWORK_ROCKET
    ));
    assert!(!bow::fires_from_inventory(items::FIREWORK_ROCKET));
    assert!(!bow::fires_from_hand(items::BOW, items::SNOWBALL));
    assert_eq!(
        bow::arrow_kind(items::SPECTRAL_ARROW),
        entity_types::SPECTRAL_ARROW
    );
    assert_eq!(bow::arrow_kind(items::TIPPED_ARROW), entity_types::ARROW);
    assert_eq!(bow::arrow_kind(items::ARROW), entity_types::ARROW);
}

#[test]
fn a_creative_player_draws_with_no_arrow_and_anyone_else_needs_one() {
    let arrow = ItemStack::new(items::ARROW, 1);
    assert!(bow::can_draw(None, true));
    assert!(bow::can_draw(Some(&arrow), false));
    assert!(!bow::can_draw(None, false));
}

#[test]
fn a_creative_draw_takes_nothing_and_makes_an_intangible_arrow() {
    let bow = ItemStack::new(items::BOW, 1);
    let arrows = ItemStack::new(items::ARROW, 16);
    let drawn = bow::draw(&bow, &arrows, true);
    assert_eq!(drawn.taken, 0);
    assert_eq!(drawn.projectiles.len(), 1);
    assert_eq!(drawn.projectiles[0].count, 1);
    assert!(intangible(&drawn.projectiles[0]));
}

#[test]
fn a_survival_draw_takes_one_arrow_but_infinity_spares_a_plain_one() {
    let bow = ItemStack::new(items::BOW, 1);
    let arrows = ItemStack::new(items::ARROW, 16);
    let drawn = bow::draw(&bow, &arrows, false);
    assert_eq!(drawn.taken, 1);
    assert_eq!(drawn.projectiles, [ItemStack::new(items::ARROW, 1)]);

    // Infinity sets the use to 0 only for `minecraft:arrow` (its `match_tool`).
    let infinite = enchanted(items::BOW, &[("minecraft:infinity", 1)]);
    let drawn = bow::draw(&infinite, &arrows, false);
    assert_eq!(drawn.taken, 0);
    assert!(intangible(&drawn.projectiles[0]));
    let tipped = ItemStack::new(items::TIPPED_ARROW, 4);
    let drawn = bow::draw(&infinite, &tipped, false);
    assert_eq!(drawn.taken, 1);
    assert!(!intangible(&drawn.projectiles[0]));
}

#[test]
fn multishot_draws_three_the_two_copies_intangible() {
    let crossbow = enchanted(items::CROSSBOW, &[("minecraft:multishot", 1)]);
    let arrows = ItemStack::new(items::ARROW, 5);
    let drawn = bow::draw(&crossbow, &arrows, false);
    assert_eq!(drawn.taken, 1);
    assert_eq!(drawn.projectiles.len(), 3);
    assert!(!intangible(&drawn.projectiles[0]));
    assert!(intangible(&drawn.projectiles[1]) && intangible(&drawn.projectiles[2]));
    // Of no ammunition, nothing at all.
    let none = bow::draw(&crossbow, &ItemStack::new(items::ARROW, 0), false);
    assert!(none.projectiles.is_empty());
}

#[test]
fn a_shot_spreads_its_projectiles_at_nought_then_either_side() {
    assert_eq!(bow::angles(1, 0.0), [0.0]);
    assert_eq!(bow::angles(3, 10.0), [0.0, -10.0, 10.0]);
    // Two over 10: the step 20, the offset 10.
    assert_eq!(bow::angles(2, 10.0), [10.0, -10.0]);
}

#[test]
fn a_players_arrow_is_picked_up_by_anyone_but_an_intangible_one_by_creative_players() {
    let arrow = ItemStack::new(items::ARROW, 1);
    assert_eq!(bow::player_arrow_pickup(&arrow), Pickup::Allowed);
    let mut drawn = arrow.clone();
    drawn.components.set(Component::IntangibleProjectile);
    assert_eq!(bow::player_arrow_pickup(&drawn), Pickup::CreativeOnly);
}

// The crossbow.

#[test]
fn a_crossbow_charges_in_twenty_five_ticks_and_quick_charge_takes_five_a_level() {
    let plain = ItemStack::new(items::CROSSBOW, 1);
    assert_eq!(crossbow::charge_duration(&plain), 25);
    for (level, ticks) in [(1, 20), (2, 15), (3, 10), (5, 0), (6, 0)] {
        let quick = enchanted(items::CROSSBOW, &[("minecraft:quick_charge", level)]);
        assert_eq!(crossbow::charge_duration(&quick), ticks, "level {level}");
    }
}

#[test]
fn the_charge_sounds_at_a_fifth_and_a_half_once_each_and_loads_at_the_full() {
    // The k-th use tick sees k − 1 ticks: the start on the 6th (5 / 25 = 0.2), the
    // middle on the 14th (13 / 25), the load on the 26th.
    let plain = ItemStack::new(items::CROSSBOW, 1);
    let starts: Vec<i32> = (0..40)
        .filter(|ticks| crossbow::reaches(*ticks, &plain, crossbow::START_SOUND_AT))
        .collect();
    let middles: Vec<i32> = (0..40)
        .filter(|ticks| crossbow::reaches(*ticks, &plain, crossbow::MIDDLE_SOUND_AT))
        .collect();
    assert_eq!((starts, middles), (vec![5], vec![13]));
    assert!(crossbow::progress(24, &plain) < crossbow::LOADED_AT);
    assert!(crossbow::progress(25, &plain) >= crossbow::LOADED_AT);
}

#[test]
fn quick_charge_starts_with_its_levels_sound_and_has_no_middle() {
    let plain = crossbow::charging_sounds(&ItemStack::new(items::CROSSBOW, 1));
    assert_eq!(
        plain,
        crossbow::ChargingSounds {
            start: Some(sounds::ITEM_CROSSBOW_LOADING_START),
            middle: Some(sounds::ITEM_CROSSBOW_LOADING_MIDDLE),
            end: Some(sounds::ITEM_CROSSBOW_LOADING_END),
        }
    );
    for (level, start) in [
        (1, sounds::ITEM_CROSSBOW_QUICK_CHARGE_1),
        (2, sounds::ITEM_CROSSBOW_QUICK_CHARGE_2),
        (3, sounds::ITEM_CROSSBOW_QUICK_CHARGE_3),
        (4, sounds::ITEM_CROSSBOW_QUICK_CHARGE_3),
    ] {
        let quick = enchanted(items::CROSSBOW, &[("minecraft:quick_charge", level)]);
        assert_eq!(
            crossbow::charging_sounds(&quick),
            crossbow::ChargingSounds {
                start: Some(start),
                middle: None,
                end: Some(sounds::ITEM_CROSSBOW_LOADING_END),
            }
        );
    }
}

#[test]
fn a_crossbow_keeps_its_charge_and_an_empty_one_reads_as_its_default() {
    let plain = ItemStack::new(items::CROSSBOW, 1);
    assert!(!crossbow::is_charged(&plain));
    let arrow = ItemStack::new(items::ARROW, 1);
    let loaded = crossbow::with_charge(&plain, vec![arrow.clone()]);
    assert!(crossbow::is_charged(&loaded));
    assert_eq!(crossbow::charged(&loaded), [arrow]);
    assert_eq!(crossbow::with_charge(&loaded, Vec::new()), plain);
}

#[test]
fn a_rocket_slows_the_whole_shot_and_wears_the_crossbow_by_three() {
    let arrow = ItemStack::new(items::ARROW, 1);
    let rocket = ItemStack::new(items::FIREWORK_ROCKET, 1);
    assert_eq!(crossbow::shooting_power(std::slice::from_ref(&arrow)), 3.15);
    assert_eq!(
        crossbow::shooting_power(&[arrow.clone(), rocket.clone()]),
        1.6
    );
    assert_eq!(crossbow::durability_use(&arrow), 1);
    assert_eq!(crossbow::durability_use(&rocket), 3);
}

#[test]
fn the_first_projectiles_pitch_draws_nothing_and_the_others_one_each() {
    assert_eq!(crossbow::shot_pitch(0, || panic!("no draw")), 1.0);
    assert_eq!(
        crossbow::shot_pitch(1, || 0.5),
        1.0 / (0.5 * 0.5 + 1.8) + 0.63
    );
    assert_eq!(
        crossbow::shot_pitch(2, || 0.5),
        1.0 / (0.5 * 0.5 + 1.8) + 0.43
    );
    assert_eq!(crossbow::loaded_pitch(0.5), 1.0 / (0.5 * 0.5 + 1.0) + 0.2);
}

#[test]
fn a_players_crossbow_shoots_along_the_look_and_turns_the_others_about_the_up_vector() {
    let (yaw, pitch) = (30.0_f32, -20.0_f32);
    let look = crate::mobs::ai::goals::view_vector(pitch, yaw);
    let straight = crossbow::player_shot_vector(yaw, pitch, 0.0);
    assert_eq!(straight.map(f64::to_bits), look.map(f64::to_bits));
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let up = crate::mobs::ai::goals::view_vector(pitch - 90.0, yaw);
    for angle in [-10.0_f32, 10.0] {
        let turned = crossbow::player_shot_vector(yaw, pitch, angle);
        // The game's table of sines leaves the look a little off length one.
        assert!(
            (dot(turned, turned) - dot(look, look)).abs() < 1e-5,
            "{turned:?}"
        );
        let cosine = dot(turned, look) / dot(look, look);
        assert!(
            (cosine - 10.0_f64.to_radians().cos()).abs() < 1e-5,
            "{cosine}"
        );
        // Turned about the up vector, it keeps its part along it.
        assert!((dot(turned, up) - dot(look, up)).abs() < 1e-5);
    }
    let left = crossbow::player_shot_vector(yaw, pitch, -10.0);
    let right = crossbow::player_shot_vector(yaw, pitch, 10.0);
    assert!((left[1] - right[1]).abs() < 1e-6 && left[0] != right[0]);
}

// The trident.

#[test]
fn a_trident_is_thrown_after_ten_ticks_and_not_before() {
    let trident = ItemStack::new(items::TRIDENT, 1);
    assert!(!trident::throws(&trident, 9));
    assert!(trident::throws(&trident, 10));
    let riptide = enchanted(items::TRIDENT, &[("minecraft:riptide", 1)]);
    assert!(!trident::throws(&riptide, 20));
    // One point short of its most, its next wear would break it.
    let mut worn = trident.clone();
    worn.components.set(Component::Damage(249));
    assert!(enchantments::next_damage_will_break(&worn));
    assert!(!trident::throws(&worn, 20));
    assert!(trident::can_raise(&trident, false));
    assert!(!trident::can_raise(&worn, false));
    assert!(!trident::can_raise(&riptide, false) && trident::can_raise(&riptide, true));
}

// The throwables and the rocket.

#[test]
fn each_throwable_has_its_kind_sound_roll_and_power() {
    let row = |item| {
        let throwable = throwables::of(item).expect("a throwable");
        (
            throwable.kind,
            throwable.sound,
            throwable.source,
            throwable.roll,
            throwable.power,
            throwable.uncertainty,
        )
    };
    let (neutral, players) = (6, 7);
    assert_eq!(
        row(items::SNOWBALL),
        (
            entity_types::SNOWBALL,
            sounds::ENTITY_SNOWBALL_THROW,
            neutral,
            0.0,
            1.5,
            1.0
        )
    );
    for egg in [items::EGG, items::BLUE_EGG, items::BROWN_EGG] {
        assert_eq!(
            row(egg),
            (
                entity_types::EGG,
                sounds::ENTITY_EGG_THROW,
                players,
                0.0,
                1.5,
                1.0
            )
        );
    }
    assert_eq!(
        row(items::ENDER_PEARL),
        (
            entity_types::ENDER_PEARL,
            sounds::ENTITY_ENDER_PEARL_THROW,
            neutral,
            0.0,
            1.5,
            1.0
        )
    );
    assert_eq!(
        row(items::SPLASH_POTION),
        (
            entity_types::SPLASH_POTION,
            sounds::ENTITY_SPLASH_POTION_THROW,
            players,
            -20.0,
            0.5,
            1.0
        )
    );
    assert_eq!(
        row(items::LINGERING_POTION),
        (
            entity_types::LINGERING_POTION,
            sounds::ENTITY_LINGERING_POTION_THROW,
            neutral,
            -20.0,
            0.5,
            1.0
        )
    );
    assert_eq!(
        row(items::EXPERIENCE_BOTTLE),
        (
            entity_types::EXPERIENCE_BOTTLE,
            sounds::ENTITY_EXPERIENCE_BOTTLE_THROW,
            neutral,
            -20.0,
            0.7,
            1.0
        )
    );
    assert!(throwables::of(items::STONE).is_none());
    assert!(throwables::of(items::POTION).is_none());
}

#[test]
fn the_pearl_has_a_second_of_cooldown_and_the_others_none() {
    let pearl = ItemStack::new(items::ENDER_PEARL, 1);
    assert_eq!(throwables::use_cooldown_ticks(&pearl), Some(20));
    assert_eq!(
        throwables::use_cooldown_ticks(&ItemStack::new(items::SNOWBALL, 1)),
        None
    );
    let mut removed = pearl.clone();
    removed.components.remove(Kind::UseCooldown);
    assert_eq!(throwables::use_cooldown_ticks(&removed), None);
    let mut longer = pearl;
    longer.components.set(Component::UseCooldown(
        clustine_world::components::UseCooldown {
            seconds: clustine_world::Float(2.5),
            group: None,
        },
    ));
    assert_eq!(throwables::use_cooldown_ticks(&longer), Some(50));
}

#[test]
fn a_rocket_used_on_a_face_starts_fifteen_hundredths_off_it() {
    let at = rockets::placed_at([3.25, 64.0, 7.5], Face::Top);
    assert_eq!(at, [3.25, 64.0 + 0.15, 7.5]);
    let at = rockets::placed_at([3.0, 64.5, 7.5], Face::West);
    assert_eq!(at, [3.0 - 0.15, 64.5, 7.5]);
}

// The enchantments, each from its data file's values.

#[test]
fn power_adds_a_half_a_level_and_a_half_to_an_arrows_damage() {
    let bow = |level| enchanted(items::BOW, &[("minecraft:power", level)]);
    assert_eq!(
        enchantments::power_damage(&ItemStack::new(items::BOW, 1), entity_types::ARROW, 2.0),
        2.0
    );
    assert_eq!(
        enchantments::power_damage(&bow(1), entity_types::ARROW, 2.0),
        3.0
    );
    assert_eq!(
        enchantments::power_damage(&bow(5), entity_types::ARROW, 2.0),
        5.0
    );
    assert_eq!(
        enchantments::power_damage(&bow(5), entity_types::SPECTRAL_ARROW, 2.0),
        5.0
    );
    // Not for what is not in `#arrows`.
    assert_eq!(
        enchantments::power_damage(&bow(5), entity_types::TRIDENT, 8.0),
        8.0
    );
}

#[test]
fn punch_adds_a_level_of_knockback_to_an_arrows() {
    let bow = enchanted(items::BOW, &[("minecraft:punch", 2)]);
    assert_eq!(
        enchantments::punch_knockback(&bow, entity_types::ARROW, 0.0),
        2.0
    );
    assert_eq!(
        enchantments::punch_knockback(&bow, entity_types::SNOWBALL, 0.0),
        0.0
    );
}

#[test]
fn flame_sets_an_arrow_alight_for_a_hundred_seconds() {
    let flame = enchanted(items::BOW, &[("minecraft:flame", 1)]);
    assert_eq!(enchantments::projectile_fire_ticks(&flame), 2000);
    assert_eq!(
        enchantments::projectile_fire_ticks(&ItemStack::new(items::BOW, 1)),
        0
    );
}

#[test]
fn infinity_spares_only_plain_arrows() {
    let bow = enchanted(items::BOW, &[("minecraft:infinity", 1)]);
    let plain = ItemStack::new(items::ARROW, 1);
    let spectral = ItemStack::new(items::SPECTRAL_ARROW, 1);
    assert_eq!(enchantments::ammo_use(&bow, &plain, 1), 0);
    assert_eq!(enchantments::ammo_use(&bow, &spectral, 1), 1);
    assert_eq!(
        enchantments::ammo_use(&ItemStack::new(items::BOW, 1), &plain, 1),
        1
    );
}

#[test]
fn multishot_makes_three_over_ten_degrees_and_piercing_a_level_each() {
    let multishot = enchanted(items::CROSSBOW, &[("minecraft:multishot", 1)]);
    assert_eq!(enchantments::projectile_count(&multishot, 1), 3);
    assert_eq!(enchantments::projectile_spread(&multishot, 0.0), 10.0);
    let plain = ItemStack::new(items::CROSSBOW, 1);
    assert_eq!(enchantments::projectile_count(&plain, 1), 1);
    assert_eq!(enchantments::projectile_spread(&plain, 0.0), 0.0);
    for level in 1..=4 {
        let piercing = enchanted(items::CROSSBOW, &[("minecraft:piercing", level)]);
        assert_eq!(enchantments::piercing(&piercing), level);
    }
    assert_eq!(enchantments::piercing(&plain), 0);
}

#[test]
fn loyalty_impaling_and_riptide_read_their_levels() {
    for level in 1..=3 {
        let loyal = enchanted(items::TRIDENT, &[("minecraft:loyalty", level)]);
        assert_eq!(enchantments::loyalty(&loyal), level);
    }
    assert_eq!(enchantments::loyalty(&ItemStack::new(items::TRIDENT, 1)), 0);
    let impaling = enchanted(items::TRIDENT, &[("minecraft:impaling", 5)]);
    // `#sensitive_to_impaling` is `#aquatic`: a cod is, a zombie is not.
    assert_eq!(
        enchantments::impaling_damage(&impaling, entity_types::COD, 8.0),
        8.0 + 12.5
    );
    assert_eq!(
        enchantments::impaling_damage(&impaling, entity_types::ZOMBIE, 8.0),
        8.0
    );
    for (level, strength) in [(1, 1.5), (2, 2.25), (3, 3.0)] {
        let riptide = enchanted(items::TRIDENT, &[("minecraft:riptide", level)]);
        assert_eq!(enchantments::riptide_strength(&riptide), strength);
    }
}

#[test]
fn unbreaking_keeps_wear_off_by_a_draw_a_point_and_nothing_draws_without_it() {
    let plain = ItemStack::new(items::BOW, 1);
    assert_eq!(
        enchantments::durability_change(&plain, 3, || panic!("no draw")),
        3
    );
    // Level 3 on a bow: the chance 3 / 4; each point below it is kept off.
    let unbreaking = enchanted(items::BOW, &[("minecraft:unbreaking", 3)]);
    let mut draws = [0.5_f32, 0.8, 0.74].into_iter();
    let change = enchantments::durability_change(&unbreaking, 3, || draws.next().unwrap());
    assert_eq!(change, 1);
    assert!(draws.next().is_none());
    // On armour the chance is 2L / (10 + 5 (L − 1)): 6 / 20 at level 3.
    let helmet = enchanted(items::IRON_HELMET, &[("minecraft:unbreaking", 3)]);
    let mut draws = [0.29_f32, 0.31].into_iter();
    let change = enchantments::durability_change(&helmet, 2, || draws.next().unwrap());
    assert_eq!(change, 1);
}

#[test]
fn wear_breaks_a_worn_out_stack_and_hurting_without_breaking_stops_one_short() {
    let bow = ItemStack::new(items::BOW, 1);
    let worn = enchantments::hurt_and_break(&bow, 1).expect("still a bow");
    assert_eq!(enchantments::damage(&worn), 1);
    let mut nearly = bow.clone();
    nearly.components.set(Component::Damage(383));
    assert_eq!(enchantments::hurt_and_break(&nearly, 1), None);
    let trident = ItemStack::new(items::TRIDENT, 1);
    let mut nearly = trident.clone();
    nearly.components.set(Component::Damage(248));
    let kept = enchantments::hurt_without_breaking(&nearly, 5);
    assert_eq!(enchantments::damage(&kept), 249);
    // What does not wear is as it was.
    let stone = ItemStack::new(items::STONE, 3);
    assert_eq!(enchantments::hurt_and_break(&stone, 1), Some(stone));
}

#[test]
fn mending_mends_twice_the_experience() {
    let mending = enchanted(items::BOW, &[("minecraft:mending", 1)]);
    assert_eq!(enchantments::mending_repair(&mending, 7), 14);
    assert_eq!(
        enchantments::mending_repair(&ItemStack::new(items::BOW, 1), 7),
        0
    );
}

// A player's uses, through a region.

const EDGE: EdgeId = EdgeId(0x5E4);
const SEED: u64 = 0x5_4004;

/// Where the player stands: on the stone of chunk (0, 0), which ticks with the eight
/// around it loaded.
const SPAWN: Vec3 = Vec3::new(8.5, 64.0, 8.5);

/// An overworld column of stone up to y = 63 and air above.
fn stone_chunk() -> Chunk {
    let sections = (0..24)
        .map(|index| {
            let state = if index < 8 {
                blocks::STONE
            } else {
                blocks::AIR
            };
            Section::filled(state, Biome(0))
        })
        .collect();
    Chunk::from_sections(-64, sections)
}

/// A region of nine chunks with one creative player in it, standing at [`SPAWN`].
struct World {
    region: Region,
    player: PlayerId,
    entity: EntityId,
    number: u64,
}

impl World {
    fn new() -> Self {
        let around: Vec<ChunkPos> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let config = RegionConfig {
            strongholds: Vec::new(),
            homes: Vec::new(),
            dimension: Default::default(),
            spawn: Some(SPAWN),
            starting_inventory: Inventory::default(),
            return_after: 0,
            lowest_y: -64,
            seed: SEED,
            game_seed: 0,
            sea_level: 63,
            spawn_mobs: true,
            light: Default::default(),
            features: Default::default(),
            random_tick_speed: crate::region::RANDOM_TICK_SPEED,
        };
        let holdings = Holdings {
            held: around.clone(),
            pinned: vec![ChunkArea::EVERYWHERE],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let mut region = Region::new(config, EntityIds::block(0).expect("block 0"), holdings);
        region.tick(&TickInputs {
            tickets_added: around
                .iter()
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            edges: vec![EdgeEvent::Started {
                edge: EDGE,
                start: 1,
            }],
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: around.iter().map(|chunk| (*chunk, stone_chunk())).collect(),
            ..TickInputs::default()
        });
        let player = PlayerId(uuid::Uuid::from_u128(1));
        let mut join = TickInputs::default();
        join.change(PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player,
                name: "Player1".to_owned(),
                attempt: 7001,
            },
        ));
        region.tick(&join);
        let entered: Vec<Entered> = region
            .state()
            .entering
            .iter()
            .map(|(player, entering)| Entered {
                unit: None,
                player: *player,
                entity: entering.entity_id,
                place: None,
                holder: None,
            })
            .collect();
        region.tick(&TickInputs {
            entered,
            ..TickInputs::default()
        });
        let entity = region.player(player).expect("player 1 entered").0;
        World {
            region,
            player,
            entity,
            number: 0,
        }
    }

    /// A tick with the player's `inputs`, in order.
    fn tick(&mut self, inputs: Vec<PlayerInput>) -> TickOutput {
        let mut ticked = TickInputs::default();
        for input in inputs {
            self.number += 1;
            ticked.input(EDGE, self.player, self.entity, self.number, input);
        }
        self.region.tick(&ticked)
    }

    /// `ticks` ticks without inputs; the last one's output.
    fn wait(&mut self, ticks: u32) -> TickOutput {
        let mut output = TickOutput::default();
        for _ in 0..ticks {
            output = self.tick(Vec::new());
        }
        output
    }

    /// The player holding `stack` in the first hotbar slot, and `others` in theirs.
    fn holding(stack: ItemStack, others: &[(u8, ItemStack)]) -> Self {
        let mut world = World::new();
        let mut inputs = vec![
            PlayerInput::SetInventorySlot {
                slot: 0,
                stack: Some(stack),
            },
            PlayerInput::SelectSlot { slot: 0 },
        ];
        for (slot, stack) in others {
            inputs.push(PlayerInput::SetInventorySlot {
                slot: *slot,
                stack: Some(stack.clone()),
            });
        }
        world.tick(inputs);
        world
    }

    fn slot(&self, slot: usize) -> Option<ItemStack> {
        self.region
            .player_state(self.player)
            .expect("the player")
            .inventory
            .0[slot]
            .clone()
    }

    fn feet(&self) -> Vec3 {
        self.region
            .player(self.player)
            .expect("the player")
            .1
            .position
    }
}

/// A use of the held item in the air, looking along `yaw` and `pitch`.
fn use_item(yaw: f32, pitch: f32) -> PlayerInput {
    PlayerInput::UseItem {
        rotation: (yaw, pitch),
        sneaking: false,
        sequence: 1,
    }
}

/// The entities a tick made, as they were made: kind, key, state.
fn made_in(output: &TickOutput) -> Vec<(i32, EntityState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) => match &state.kind {
                EntityKind::Other { kind, .. } => Some((i32::from(*kind), state.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn motion_of(state: &EntityState) -> [f64; 3] {
    match &state.kind {
        EntityKind::Other { motion, .. } => *motion,
        _ => panic!("not an entity of a chunk"),
    }
}

fn data_of(state: &EntityState) -> EntityData {
    match &state.kind {
        EntityKind::Other { data, .. } => data.clone(),
        _ => panic!("not an entity of a chunk"),
    }
}

fn arrow_of(state: &EntityState) -> clustine_world::Arrow {
    match data_of(state) {
        EntityData::Arrow(arrow) => *arrow,
        other => panic!("{other:?} is no arrow"),
    }
}

/// The sounds of a tick: sound, source, volume and pitch.
fn heard(output: &TickOutput) -> Vec<(i32, i32, f32, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                source,
                volume,
                pitch,
                ..
            } => Some((*sound, *source, *volume, *pitch)),
            _ => None,
        })
        .collect()
}

/// The level's random as the region's `n`-th stream drawn aside in the tick `tick`.
fn level(n: i32, tick: u64) -> Random {
    Random::of_section(SEED, ChunkPos::new(0, 0), n, tick)
}

/// The stream of the entity `key` made in the tick `tick`, past its UUID.
fn own_after_uuid(key: EntityId, tick: u64) -> Random {
    let mut own = Random::of_entity_aside(SEED, key, 0, tick);
    own.next_u64();
    own.next_u64();
    own
}

const PLAYERS: i32 = 7;

/// Draws the bow for `ticks` use ticks, then lets it go: the release's output.
fn loosed_after(world: &mut World, ticks: u32, yaw: f32, pitch: f32) -> TickOutput {
    world.tick(vec![use_item(yaw, pitch)]);
    world.wait(ticks - 1);
    world.tick(vec![PlayerInput::ReleaseUse])
}

#[test]
fn a_bow_let_go_after_five_ten_and_twenty_ticks_looses_arrows_of_the_games_speed() {
    for ticks in [5, 10, 20] {
        let arrows = ItemStack::new(items::ARROW, 16);
        let mut world = World::holding(ItemStack::new(items::BOW, 1), &[(9, arrows.clone())]);
        let output = loosed_after(&mut world, ticks, 30.0, -20.0);
        let power = bow::power_for_time(ticks as i32);
        let made = made_in(&output);
        assert_eq!(made.len(), 1, "{ticks} ticks");
        let (kind, arrow) = &made[0];
        assert_eq!(*kind, entity_types::ARROW);
        // The arrow's own stream: its UUID, then `shootFromRotation` at three times the
        // power and the uncertainty 1.
        let mut own = own_after_uuid(arrow.entity, output.tick);
        let shot =
            flight::shoot_from_rotation(30.0, -20.0, power * 3.0, 1.0, [0.0; 3], true, &mut own);
        assert_eq!(
            motion_of(arrow).map(f64::to_bits),
            shot.motion.map(f64::to_bits)
        );
        let feet = world.feet();
        assert_eq!(
            arrow.pose.position,
            Vec3::new(
                feet.x,
                feet.y + f64::from(1.62_f32) - 0.100_000_001_490_116_12,
                feet.z
            )
        );
        let record = arrow_of(arrow);
        assert_eq!(record.crit, ticks == 20, "crit only at full power");
        // A creative player's bow draws intangible ammunition: picked up only by a
        // creative player, and none taken.
        assert_eq!(record.pickup, Pickup::CreativeOnly);
        assert!(intangible(&record.pickup_stack));
        assert_eq!(record.weapon, Some(ItemStack::new(items::BOW, 1)));
        assert_eq!(record.owner, Some(world.entity));
        assert_eq!(world.slot(9), Some(arrows));
        // `arrow_shoot` after the shot, its pitch of the level's random.
        let drawn = level(1, output.tick).next_float();
        assert_eq!(
            heard(&output),
            [(
                sounds::ENTITY_ARROW_SHOOT,
                PLAYERS,
                1.0,
                bow::shoot_pitch(drawn, power)
            )]
        );
    }
}

#[test]
fn a_bow_let_go_after_two_ticks_looses_nothing_and_is_not_heard() {
    let mut world = World::holding(ItemStack::new(items::BOW, 1), &[]);
    let output = loosed_after(&mut world, 2, 0.0, 0.0);
    assert!(made_in(&output).is_empty());
    assert!(
        heard(&output)
            .iter()
            .all(|(sound, ..)| *sound != sounds::ENTITY_ARROW_SHOOT)
    );
}

#[test]
fn a_creative_bow_with_no_arrows_shoots_a_plain_one_and_a_spectral_one_of_its_kind() {
    let mut world = World::holding(ItemStack::new(items::BOW, 1), &[]);
    let output = loosed_after(&mut world, 20, 0.0, 0.0);
    let made = made_in(&output);
    assert_eq!(made.len(), 1);
    assert_eq!(arrow_of(&made[0].1).pickup_stack.item, items::ARROW);

    // The off hand's arrows come first.
    let spectral = ItemStack::new(items::SPECTRAL_ARROW, 3);
    let off = u8::try_from(clustine_world::OFF_HAND).unwrap();
    let mut world = World::holding(
        ItemStack::new(items::BOW, 1),
        &[
            (off, spectral.clone()),
            (5, ItemStack::new(items::ARROW, 3)),
        ],
    );
    let output = loosed_after(&mut world, 20, 0.0, 0.0);
    let made = made_in(&output);
    assert_eq!(made[0].0, entity_types::SPECTRAL_ARROW);
    assert_eq!(world.slot(clustine_world::OFF_HAND), Some(spectral));
}

#[test]
fn a_flame_bows_arrow_is_alight_from_its_making() {
    let flame = enchanted(items::BOW, &[("minecraft:flame", 1)]);
    let mut world = World::holding(flame, &[]);
    let output = loosed_after(&mut world, 20, 0.0, 0.0);
    let made = made_in(&output);
    let key = made[0].1.entity;
    let record = output
        .entity_changes
        .iter()
        .find_map(|(_, change)| match change {
            clustine_world::EntityChange::Whole(entity) if entity.key == key => Some(entity),
            _ => None,
        })
        .expect("the arrow's making");
    assert_eq!(record.fire_ticks, 2000);
}

/// The crossbow the player holds in the first slot.
fn crossbow_held(world: &World) -> ItemStack {
    world.slot(0).expect("the crossbow")
}

/// The ticks of a use of the crossbow at which each of its sounds was heard, held for
/// `ticks` use ticks.
fn charged_for(world: &mut World, ticks: u32) -> Vec<(u32, i32)> {
    let mut heard_at = Vec::new();
    let output = world.tick(vec![use_item(0.0, 0.0)]);
    for (sound, ..) in heard(&output) {
        heard_at.push((1, sound));
    }
    for tick in 2..=ticks {
        let output = world.tick(Vec::new());
        for (sound, ..) in heard(&output) {
            heard_at.push((tick, sound));
        }
    }
    heard_at
}

#[test]
fn a_crossbow_held_charges_over_its_duration_and_loads_into_the_held_stack() {
    let mut world = World::holding(ItemStack::new(items::CROSSBOW, 1), &[]);
    let heard_at = charged_for(&mut world, 25);
    assert_eq!(
        heard_at,
        [
            (6, sounds::ITEM_CROSSBOW_LOADING_START),
            (14, sounds::ITEM_CROSSBOW_LOADING_MIDDLE),
        ]
    );
    assert!(!crossbow::is_charged(&crossbow_held(&world)));
    // The 26th use tick sees 25: it loads, with the end's sound of the level's random.
    let output = world.tick(Vec::new());
    let drawn = level(1, output.tick).next_float();
    assert_eq!(
        heard(&output),
        [(
            sounds::ITEM_CROSSBOW_LOADING_END,
            PLAYERS,
            1.0,
            crossbow::loaded_pitch(drawn)
        )]
    );
    let loaded = crossbow_held(&world);
    let charge = crossbow::charged(&loaded);
    assert_eq!(charge.len(), 1);
    assert_eq!(charge[0].item, items::ARROW);
    assert!(intangible(&charge[0]));
    // The client is told of the stack it cannot foresee.
    assert!(output.player_events.contains(&(
        world.player,
        PlayerEvent::InventorySlot {
            slot: 0,
            stack: Some(loaded.clone())
        }
    )));
    // Held on, it neither sounds nor loads again.
    let output = world.wait(10);
    assert!(heard(&output).is_empty());
    assert_eq!(crossbow_held(&world), loaded);
}

#[test]
fn quick_charge_loads_the_crossbow_five_ticks_a_level_sooner() {
    let quick = enchanted(items::CROSSBOW, &[("minecraft:quick_charge", 1)]);
    let mut world = World::holding(quick, &[]);
    let heard_at = charged_for(&mut world, 21);
    // 20 ticks: the start at 4 / 20, its level's sound; no middle; loaded on the 21st.
    assert_eq!(
        heard_at,
        [
            (5, sounds::ITEM_CROSSBOW_QUICK_CHARGE_1),
            (21, sounds::ITEM_CROSSBOW_LOADING_END),
        ]
    );
    assert!(crossbow::is_charged(&crossbow_held(&world)));
}

#[test]
fn a_crossbow_let_go_early_loads_nothing_and_its_release_runs_one_more_tick() {
    let mut world = World::holding(ItemStack::new(items::CROSSBOW, 1), &[]);
    charged_for(&mut world, 10);
    world.tick(vec![PlayerInput::ReleaseUse]);
    assert!(!crossbow::is_charged(&crossbow_held(&world)));
    // Let go after 25 ticks, the release's extra tick sees 25 and loads it.
    let mut world = World::holding(ItemStack::new(items::CROSSBOW, 1), &[]);
    charged_for(&mut world, 25);
    let output = world.tick(vec![PlayerInput::ReleaseUse]);
    assert!(crossbow::is_charged(&crossbow_held(&world)));
    assert_eq!(
        heard(&output)
            .iter()
            .map(|(sound, ..)| *sound)
            .collect::<Vec<_>>(),
        [sounds::ITEM_CROSSBOW_LOADING_END]
    );
}

/// The player holding a crossbow charged with `charge`, looking along `yaw` and
/// `pitch`: the output of the use that shoots it.
fn shot_of(
    crossbow: ItemStack,
    charge: Vec<ItemStack>,
    yaw: f32,
    pitch: f32,
) -> (World, TickOutput) {
    let loaded = crossbow::with_charge(&crossbow, charge);
    let mut world = World::holding(loaded, &[]);
    let output = world.tick(vec![use_item(yaw, pitch)]);
    (world, output)
}

#[test]
fn a_charged_crossbow_shoots_one_arrow_along_the_look_crit_and_is_emptied() {
    let mut arrow = ItemStack::new(items::ARROW, 1);
    arrow.components.set(Component::IntangibleProjectile);
    let (world, output) = shot_of(ItemStack::new(items::CROSSBOW, 1), vec![arrow], -45.0, 10.0);
    let made = made_in(&output);
    assert_eq!(made.len(), 1);
    let (kind, shot) = &made[0];
    assert_eq!(*kind, entity_types::ARROW);
    let mut own = own_after_uuid(shot.entity, output.tick);
    let direction = crossbow::player_shot_vector(-45.0, 10.0, 0.0);
    let expected = flight::shoot(direction, 3.15, 1.0, &mut own);
    assert_eq!(
        motion_of(shot).map(f64::to_bits),
        expected.motion.map(f64::to_bits)
    );
    let record = arrow_of(shot);
    assert!(record.crit, "a player's crossbow shoots crit");
    assert_eq!(record.hit_sound, "minecraft:item.crossbow.hit");
    assert_eq!(record.pickup, Pickup::CreativeOnly);
    assert_eq!(record.weapon, Some(ItemStack::new(items::CROSSBOW, 1)));
    assert!(!crossbow::is_charged(&crossbow_held(&world)));
    // One `crossbow_shoot`, at 1 for the first: no draw.
    assert_eq!(
        heard(&output),
        [(sounds::ITEM_CROSSBOW_SHOOT, PLAYERS, 1.0, 1.0)]
    );
    // And the use does not begin: the shot is the use.
    assert!(
        world
            .region
            .player_state(world.player)
            .expect("the player")
            .using
            .is_none()
    );
}

#[test]
fn multishot_shoots_its_three_at_their_angles_each_heard_with_the_shooters_pitch() {
    let multishot = enchanted(items::CROSSBOW, &[("minecraft:multishot", 1)]);
    let arrows = bow::draw(&multishot, &ItemStack::new(items::ARROW, 1), true).projectiles;
    assert_eq!(arrows.len(), 3);
    let (world, output) = shot_of(multishot.clone(), arrows, 20.0, -5.0);
    let made = made_in(&output);
    assert_eq!(made.len(), 3);
    for ((_, shot), angle) in made.iter().zip([0.0_f32, -10.0, 10.0]) {
        let mut own = own_after_uuid(shot.entity, output.tick);
        let direction = crossbow::player_shot_vector(20.0, -5.0, angle);
        let expected = flight::shoot(direction, 3.15, 1.0, &mut own);
        assert_eq!(
            motion_of(shot).map(f64::to_bits),
            expected.motion.map(f64::to_bits),
            "at {angle}"
        );
        assert!(arrow_of(shot).crit);
    }
    // The pitches after the first of the shooter's own random, in turn.
    let mut shooter = Random::of_entity_aside(SEED, world.entity, 0, output.tick);
    let second = crossbow::shot_pitch(1, || shooter.next_float());
    let third = crossbow::shot_pitch(2, || shooter.next_float());
    assert_eq!(
        heard(&output),
        [
            (sounds::ITEM_CROSSBOW_SHOOT, PLAYERS, 1.0, 1.0),
            (sounds::ITEM_CROSSBOW_SHOOT, PLAYERS, 1.0, second),
            (sounds::ITEM_CROSSBOW_SHOOT, PLAYERS, 1.0, third),
        ]
    );
    let emptied = crossbow_held(&world);
    assert_eq!(emptied, multishot);
}

#[test]
fn a_piercing_crossbows_arrow_pierces_by_its_level() {
    let piercing = enchanted(items::CROSSBOW, &[("minecraft:piercing", 3)]);
    let (_, output) = shot_of(piercing, vec![ItemStack::new(items::ARROW, 1)], 0.0, 0.0);
    let made = made_in(&output);
    let record = arrow_of(&made[0].1);
    assert_eq!(record.pierce_level, 3);
    // An arrow loaded by a survival player keeps its pickup.
    assert_eq!(record.pickup, Pickup::Allowed);
}

#[test]
fn a_trident_let_go_after_ten_ticks_is_thrown_and_not_after_nine() {
    let trident = ItemStack::new(items::TRIDENT, 1);
    let mut world = World::holding(trident.clone(), &[]);
    world.tick(vec![use_item(0.0, 0.0)]);
    world.wait(8);
    let output = world.tick(vec![PlayerInput::ReleaseUse]);
    assert!(made_in(&output).is_empty());
    assert!(heard(&output).is_empty());

    let mut world = World::holding(trident.clone(), &[]);
    world.tick(vec![use_item(10.0, -30.0)]);
    world.wait(9);
    let output = world.tick(vec![PlayerInput::ReleaseUse]);
    let made = made_in(&output);
    assert_eq!(made.len(), 1);
    let (kind, thrown) = &made[0];
    assert_eq!(*kind, entity_types::TRIDENT);
    let mut own = own_after_uuid(thrown.entity, output.tick);
    let shot = flight::shoot_from_rotation(10.0, -30.0, 2.5, 1.0, [0.0; 3], true, &mut own);
    assert_eq!(
        motion_of(thrown).map(f64::to_bits),
        shot.motion.map(f64::to_bits)
    );
    let record = arrow_of(thrown);
    assert_eq!(record.pickup, Pickup::CreativeOnly);
    assert_eq!(record.pickup_stack, trident);
    assert_eq!(record.weapon, None);
    // `trident_throw` at the trident, without a draw; the creative player keeps theirs.
    assert_eq!(
        heard(&output),
        [(sounds::ITEM_TRIDENT_THROW, PLAYERS, 1.0, 1.0)]
    );
    assert_eq!(world.slot(0), Some(trident));
}

/// The throw of `stack` along `yaw` and `pitch`: the world after it and its output.
fn thrown(stack: ItemStack, yaw: f32, pitch: f32) -> (World, TickOutput) {
    let mut world = World::holding(stack, &[]);
    let output = world.tick(vec![use_item(yaw, pitch)]);
    (world, output)
}

#[test]
fn each_throwable_is_thrown_with_its_kind_record_sound_and_shot() {
    let mut splash = ItemStack::new(items::SPLASH_POTION, 1);
    splash
        .components
        .set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:healing".to_owned()),
            ..PotionContents::default()
        }));
    let cases = [
        ItemStack::new(items::SNOWBALL, 16),
        ItemStack::new(items::EGG, 16),
        ItemStack::new(items::BLUE_EGG, 16),
        ItemStack::new(items::BROWN_EGG, 16),
        splash,
        ItemStack::new(items::LINGERING_POTION, 1),
        ItemStack::new(items::EXPERIENCE_BOTTLE, 64),
        ItemStack::new(items::ENDER_PEARL, 16),
    ];
    for stack in cases {
        let throwable = throwables::of(stack.item).expect("a throwable");
        let (world, output) = thrown(stack.clone(), 30.0, -20.0);
        // The sound first, of the level's random.
        let drawn = level(1, output.tick).next_float();
        assert_eq!(
            heard(&output),
            [(
                throwable.sound,
                throwable.source,
                0.5,
                throwables::pitch(drawn)
            )],
            "{stack:?}"
        );
        let made = made_in(&output);
        assert_eq!(made.len(), 1, "{stack:?}");
        let (kind, projectile) = &made[0];
        assert_eq!(*kind, throwable.kind);
        assert_eq!(
            data_of(projectile),
            EntityData::Thrown {
                stack: stack.with_count(1),
                owner: Some(world.entity),
                left_owner: false,
                has_been_shot: false,
            }
        );
        // `shootFromRotation` with the kind's roll: its height by pitch and roll.
        let mut own = own_after_uuid(projectile.entity, output.tick);
        let expected = if throwable.roll == 0.0 {
            flight::shoot_from_rotation(30.0, -20.0, throwable.power, 1.0, [0.0; 3], true, &mut own)
                .motion
        } else {
            use crate::mobs::ai::maths::{RADIANS, table_cos, table_sin};
            let x = -table_sin(30.0 * RADIANS) * table_cos(-20.0 * RADIANS);
            let y = -table_sin((-20.0 + throwable.roll) * RADIANS);
            let z = table_cos(30.0 * RADIANS) * table_cos(-20.0 * RADIANS);
            flight::shoot(
                [f64::from(x), f64::from(y), f64::from(z)],
                throwable.power,
                1.0,
                &mut own,
            )
            .motion
        };
        assert_eq!(
            motion_of(projectile).map(f64::to_bits),
            expected.map(f64::to_bits),
            "{stack:?}"
        );
        // A creative player keeps the stack.
        assert_eq!(world.slot(0), Some(stack.clone()));
    }
}

#[test]
fn a_thrown_pearl_puts_pearls_on_a_second_of_cooldown_and_the_next_is_not_thrown() {
    let pearl = ItemStack::new(items::ENDER_PEARL, 16);
    let (mut world, output) = thrown(pearl.clone(), 0.0, 0.0);
    assert!(output.player_events.contains(&(
        world.player,
        PlayerEvent::Cooldown {
            group: "minecraft:ender_pearl".to_owned(),
            ticks: 20
        }
    )));
    let output = world.tick(vec![use_item(0.0, 0.0)]);
    assert!(made_in(&output).is_empty());
    assert!(heard(&output).is_empty());
    // Its second over, another flies.
    world.wait(19);
    let output = world.tick(vec![use_item(0.0, 0.0)]);
    assert_eq!(made_in(&output).len(), 1);
    // The others have none.
    let (_, output) = thrown(ItemStack::new(items::SNOWBALL, 16), 0.0, 0.0);
    assert!(
        output
            .player_events
            .iter()
            .all(|(_, event)| !matches!(event, PlayerEvent::Cooldown { .. }))
    );
}

#[test]
fn an_off_hand_throw_throws_what_the_off_hand_holds() {
    let off = u8::try_from(clustine_world::OFF_HAND).unwrap();
    let mut world = World::new();
    world.tick(vec![PlayerInput::SetInventorySlot {
        slot: off,
        stack: Some(ItemStack::new(items::EGG, 4)),
    }]);
    let output = world.tick(vec![PlayerInput::WithOffHand(Box::new(use_item(0.0, 0.0)))]);
    let made = made_in(&output);
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].0, entity_types::EGG);
}

#[test]
fn a_rocket_used_on_a_block_is_made_on_its_face_and_a_creative_player_keeps_the_stack() {
    // `FireworkRocketItem.useOn`: the clicked point moved 0.15 along the face.
    let rocket = ItemStack::new(items::FIREWORK_ROCKET, 3);
    let mut world = World::holding(rocket.clone(), &[]);
    let output = world.tick(vec![PlayerInput::UseItemOn {
        position: clustine_world::BlockPos::new(9, 63, 8),
        face: Face::Top,
        cursor: [0.5, 1.0, 0.5],
        sneaking: false,
        sequence: 1,
    }]);
    let made = made_in(&output);
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(made[0].0, entity_types::FIREWORK_ROCKET);
    let at = made[0].1.pose.position;
    assert_eq!((at.x, at.z), (9.5, 8.5));
    assert!((at.y - 64.15).abs() < 1e-9, "{at:?}");
    assert_eq!(world.slot(0), Some(rocket));
}
