//! Tests of riding the camel, the happy ghast and the nautilus
//! (`docs/groundwork/n7j-riding-plan.md`, with its research note's rules from the game's
//! bytecode, `n7j-riding-research.md`): who gets on and how many, where they sit, who
//! steers, the dashes and their cooldowns, the ghast's goggles and its staying still,
//! and where each lets its riders off. The numbers each test expects are worked out here
//! from the game's formulas, not taken from what the region computes.
//!
//! Players 1 and 2 stand at the spawn, (0.5, -60, 0.5), on a stone floor whose top is
//! y = -60, with the 25 chunks within two of chunk (0, 0) loaded and ticking.

use clustine_data::{EntityTypeInfo, entity_types, items, sounds};
use clustine_world::Entity;

use super::redstone::sounds as heard;
use super::*;
use crate::api::{ClickKind, MenuKind};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::types::{camel, happy_ghast, nautilus, tamable};

/// The region with players 1 and 2 at the spawn, on a ticking stone floor.
fn two_players() -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1, 2]);
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    checked(&mut region, &tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| (position, floor()))
        .collect();
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    region
}

/// A mob of `kind` made as the region makes one, `dressed` first, at `position` facing
/// south, laid in its chunk; its key.
fn made(
    region: &mut Region,
    kind: i32,
    position: [f64; 3],
    dressed: impl FnOnce(&mut Mob),
) -> EntityId {
    let mut mob = Mob::fresh();
    dressed(&mut mob);
    let entity = region
        .new_mob(kind as u16, mob, position, 0.0, 0.0)
        .expect("a loaded chunk");
    let key = entity.key;
    let mut output = TickOutput::default();
    region.add_made_mob(entity, &mut output);
    key
}

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .expect("the entity is here")
        .1
        .clone()
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).data).expect("a mob")
}

fn click(n: u128, key: EntityId, sneaking: bool) -> TickInputs {
    moves(vec![numbered(
        n,
        PlayerInput::Interact {
            entity: key,
            location: [0.0, 0.5, 0.0],
            sneaking,
        },
    )])
}

fn riding(region: &Region, n: u128) -> Option<EntityId> {
    region.players[&player(n)]
        .riding
        .map(|riding| riding.vehicle)
}

fn feet(region: &Region, n: u128) -> [f64; 3] {
    let at = region.players[&player(n)].pose.position;
    [at.x, at.y, at.z]
}

fn saddle(mob: &mut Mob) {
    mob.body.equipment.slots[usize::from(slots::SADDLE)] = Some(ItemStack {
        item: items::SADDLE,
        count: 1,
    });
}

/// A grown camel standing as one long stood (`finalizeSpawn`'s, its 52 ticks of standing
/// up long over: the world's clock set on for it), saddled where `saddled`.
fn camel_at(region: &mut Region, saddled: bool) -> EntityId {
    region.clock.game_time = 10_000;
    let now = i64::try_from(region.clock.game_time).unwrap_or(0);
    made(region, entity_types::CAMEL, [2.5, -60.0, 0.5], |mob| {
        camel::stand_up_instantly(mob, now);
        if saddled {
            saddle(mob);
        }
    })
}

/// The feet of a player seated at `point` above the feet of `vehicle`, less their own
/// vehicle point.
fn seated_at(vehicle: &Entity, point: [f64; 3]) -> [f64; 3] {
    let attach = EntityTypeInfo::of(entity_types::PLAYER).vehicle_points[0];
    std::array::from_fn(|axis| (vehicle.position[axis] + point[axis]) - attach[axis])
}

/// `Camel.mobInteract`: a grown camel takes a player on with no saddle, and a second;
/// a third is not taken (the click's `< 2`). A standing camel seats one 0.5 ahead at
/// `2.375 − 0.375 = 2` up, and two at 0.5 ahead and 0.7 behind.
#[test]
fn a_camel_takes_two_players_on_with_no_saddle_one_ahead_of_the_other() {
    let mut region = two_players();
    let camel = camel_at(&mut region, false);
    checked(&mut region, &click(1, camel, false));
    assert_eq!(riding(&region, 1), Some(camel));
    checked(&mut region, &TickInputs::default());
    let vehicle = record(&region, camel);
    assert_eq!(feet(&region, 1), seated_at(&vehicle, [0.0, 2.0, 0.5]));

    checked(&mut region, &click(2, camel, false));
    assert_eq!(riding(&region, 2), Some(camel));
    checked(&mut region, &TickInputs::default());
    let vehicle = record(&region, camel);
    assert_eq!(vehicle.riding.passengers.len(), 2);
    assert_eq!(feet(&region, 1), seated_at(&vehicle, [0.0, 2.0, 0.5]));
    let behind = f64::from(-0.7_f32);
    assert_eq!(feet(&region, 2), seated_at(&vehicle, [0.0, 2.0, behind]));
}

/// `getControllingPassenger`: a saddled camel's first player steers it; an unsaddled
/// one goes its own way with them on.
#[test]
fn a_saddled_camel_is_steered_by_its_first_rider_and_a_bare_one_is_not() {
    for saddled in [true, false] {
        let mut region = two_players();
        let camel = camel_at(&mut region, saddled);
        checked(&mut region, &click(1, camel, false));
        let vehicle = record(&region, camel);
        let driven = matches!(
            region.ridden(&vehicle, &mob(&region, camel)),
            crate::mobs::tick::Ridden::Driven { .. }
        );
        assert_eq!(driven, saddled);
    }
}

fn riding_jump(n: u128, power: i32) -> TickInputs {
    moves(vec![numbered(n, PlayerInput::RidingJump { power })])
}

/// `handleStartJump` on the server: the dash's sound at the camel's volume (0.8) with
/// its voice's pitch, `DASH` set, and its cooldown of 55 begun from the command, so
/// that the ready sound comes on the 55th tick counting the command's own; a second
/// command sounds again but does not begin the cooldown anew.
#[test]
fn a_camels_dash_sounds_at_once_and_is_ready_again_on_the_fifty_fifth_tick() {
    let mut region = two_players();
    let camel = camel_at(&mut region, true);
    checked(&mut region, &click(1, camel, false));
    let output = checked(&mut region, &riding_jump(1, 60));
    let dashes: Vec<_> = heard(&output)
        .into_iter()
        .filter(|(sound, ..)| *sound == sounds::ENTITY_CAMEL_DASH)
        .collect();
    assert_eq!(dashes.len(), 1, "{:?}", heard(&output));
    let (_, volume, pitch, _) = dashes[0];
    assert_eq!(volume, 0.8);
    assert!((0.8..=1.2).contains(&pitch), "{pitch}");
    assert!(camel::camel_of(&mob(&region, camel)).dashing);

    let again = checked(&mut region, &riding_jump(1, 60));
    assert!(
        heard(&again)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ENTITY_CAMEL_DASH),
        "the server tests no cooldown before it"
    );
    let mut ready_at = None;
    for tick in 3..=60 {
        let output = checked(&mut region, &TickInputs::default());
        if heard(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ENTITY_CAMEL_DASH_READY)
        {
            ready_at = Some(tick);
            break;
        }
    }
    assert_eq!(ready_at, Some(55));
}

/// `canJump`: an unsaddled camel does not dash.
#[test]
fn an_unsaddled_camel_does_not_dash() {
    let mut region = two_players();
    let camel = camel_at(&mut region, false);
    checked(&mut region, &click(1, camel, false));
    let output = checked(&mut region, &riding_jump(1, 60));
    assert!(
        !heard(&output)
            .iter()
            .any(|(sound, ..)| *sound == sounds::ENTITY_CAMEL_DASH)
    );
    assert!(!camel::camel_of(&mob(&region, camel)).dashing);
}

/// A grown happy ghast wearing a harness, on the floor beside the spawn, within reach.
fn ghast_at(region: &mut Region) -> EntityId {
    made(
        region,
        entity_types::HAPPY_GHAST,
        [4.5, -60.0, 0.5],
        |mob| {
            mob.body.equipment.slots[usize::from(slots::BODY)] = Some(ItemStack {
                item: items::WHITE_HARNESS,
                count: 1,
            });
        },
    )
}

/// `HappyGhast.mobInteract` and `addPassenger`: a harnessed ghast takes a player on
/// without turning them, its goggles coming down at the first rider only; a second
/// gets on too. `removePassenger`: one getting off holds it still for ten ticks, when
/// nobody steers it, and the goggles go up only when the last is off. Each is let off
/// on its top, at its middle.
#[test]
fn a_ghast_takes_riders_on_with_its_goggles_and_stays_still_ten_ticks_after_one_gets_off() {
    let mut region = two_players();
    let ghast = ghast_at(&mut region);
    let yaw = region.players[&player(1)].pose.yaw;
    let output = checked(&mut region, &click(1, ghast, false));
    assert_eq!(riding(&region, 1), Some(ghast));
    assert_eq!(region.players[&player(1)].pose.yaw, yaw, "not turned");
    let goggles = |output: &TickOutput, sound| {
        heard(output)
            .iter()
            .filter(|(heard, ..)| *heard == sound)
            .count()
    };
    assert_eq!(
        goggles(&output, sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN),
        1
    );
    let output = checked(&mut region, &click(2, ghast, false));
    assert_eq!(riding(&region, 2), Some(ghast));
    assert_eq!(
        goggles(&output, sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN),
        0
    );
    let vehicle = record(&region, ghast);
    assert!(matches!(
        region.ridden(&vehicle, &mob(&region, ghast)),
        crate::mobs::tick::Ridden::Driven { .. }
    ));

    // The second rider sneaks off: on its top, the ghast still and not driven.
    let sneak = moves(vec![numbered(
        2,
        PlayerInput::Keys(crate::api::Keys {
            sneak: true,
            ..Default::default()
        }),
    )]);
    let output = checked(&mut region, &sneak);
    assert_eq!(riding(&region, 2), None);
    assert_eq!(
        goggles(&output, sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP),
        0
    );
    let vehicle = record(&region, ghast);
    let top = region.entity_box(&vehicle).max[1];
    assert_eq!(
        feet(&region, 2),
        [vehicle.position[0], top, vehicle.position[2]]
    );
    let still = happy_ghast::read(&mob(&region, ghast));
    assert!(still.on_still_timeout());
    assert!(!matches!(
        region.ridden(&vehicle, &mob(&region, ghast)),
        crate::mobs::tick::Ridden::Driven { .. }
    ));

    // The last off: the goggles go up.
    let sneak = moves(vec![numbered(
        1,
        PlayerInput::Keys(crate::api::Keys {
            sneak: true,
            ..Default::default()
        }),
    )]);
    let output = checked(&mut region, &sneak);
    assert_eq!(riding(&region, 1), None);
    assert_eq!(
        goggles(&output, sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP),
        1
    );
}

/// `canAddPassenger`: a happy ghast takes four, a camel three (the game's `<= 2`), any
/// other mob one.
#[test]
fn a_ghast_seats_four_a_camel_three_and_another_mob_one() {
    use crate::mobs::types::of;
    let fresh = Mob::fresh();
    let seats = |kind: i32| {
        (0..6)
            .take_while(|taken| of(kind as u16).can_add_passenger(&fresh, *taken))
            .count()
    };
    assert_eq!(seats(entity_types::HAPPY_GHAST), 4);
    assert_eq!(seats(entity_types::CAMEL), 3);
    assert_eq!(seats(entity_types::NAUTILUS), 1);
    assert_eq!(seats(entity_types::PIG), 1);
}

/// `isFlyingVehicle`: a grown happy ghast is one, so its driver is never kicked for
/// floating and its moves gather no fall; a ghastling and a pig are not.
#[test]
fn a_grown_ghast_is_a_flying_vehicle_and_a_ghastling_and_a_pig_are_not() {
    use crate::mobs::types::of;
    let grown = Mob::fresh();
    let mut baby = Mob::fresh();
    crate::mobs::types::animal::update(&mut baby, |animal| animal.age = -24_000);
    assert!(of(entity_types::HAPPY_GHAST as u16).flying_vehicle(&grown));
    assert!(!of(entity_types::HAPPY_GHAST as u16).flying_vehicle(&baby));
    assert!(!of(entity_types::PIG as u16).flying_vehicle(&grown));
}

/// A tame nautilus, saddled where `saddled`, by player 1.
fn nautilus_at(region: &mut Region, saddled: bool) -> EntityId {
    made(region, entity_types::NAUTILUS, [2.5, -60.0, 0.5], |mob| {
        nautilus::update(mob, |own| tamable::tame(&mut own.tame, Some(1)));
        if saddled {
            saddle(mob);
        }
    })
}

/// `AbstractNautilus.mobInteract`: a tame nautilus takes a player on, saddled or not; a
/// second click by another finds it full. Saddled, its rider steers it.
#[test]
fn a_tame_nautilus_takes_one_player_on_and_steers_only_saddled() {
    for saddled in [true, false] {
        let mut region = two_players();
        let key = nautilus_at(&mut region, saddled);
        checked(&mut region, &click(1, key, false));
        assert_eq!(riding(&region, 1), Some(key));
        checked(&mut region, &click(2, key, false));
        assert_eq!(riding(&region, 2), None, "one seat");
        let vehicle = record(&region, key);
        let driven = matches!(
            region.ridden(&vehicle, &mob(&region, key)),
            crate::mobs::tick::Ridden::Driven { .. }
        );
        assert_eq!(driven, saddled);
    }
}

/// `handleStartJump` of a saddled nautilus: the dash's sound by the medium (out of
/// water, `dash_land`) at volume 1, and `DASH`.
#[test]
fn a_saddled_nautilus_dashes_with_its_sound_on_land() {
    let mut region = two_players();
    let key = nautilus_at(&mut region, true);
    checked(&mut region, &click(1, key, false));
    let output = checked(&mut region, &riding_jump(1, 60));
    let dash: Vec<_> = heard(&output)
        .into_iter()
        .filter(|(sound, ..)| *sound == sounds::ENTITY_NAUTILUS_DASH_LAND)
        .collect();
    assert_eq!(dash.len(), 1, "{:?}", heard(&output));
    assert_eq!(dash[0].1, 1.0);
}

/// The menu the player `n` was shown opening in `output`: its serial, kind and slots.
fn menu_opened(output: &TickOutput, n: u128) -> Option<(u64, MenuKind, usize)> {
    output
        .player_events
        .iter()
        .find_map(|(id, event)| match event {
            PlayerEvent::MenuOpened {
                serial,
                kind,
                slots,
                ..
            } if *id == player(n) => Some((*serial, *kind, slots.len())),
            _ => None,
        })
}

fn container_click(serial: u64, slot: i16) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::ContainerClick {
            serial,
            slot,
            button: 0,
            kind: ClickKind::Pickup,
        },
    )])
}

/// `Camel.mobInteract`'s first rule and `openCustomInventoryScreen`: a sneaking click
/// opens a grown camel's menu of its saddle and body slots and no chest; a saddle put in
/// its slot is worn, sounding the camel's own saddle sound (`getEquipSound`) once for
/// `ArmorSlot.setByPlayer` and once for `setItemSlot`.
#[test]
fn a_sneaking_click_opens_a_camels_menu_and_a_saddle_put_there_sounds_as_the_camels() {
    let mut region = two_players();
    let camel = camel_at(&mut region, false);
    checked(&mut region, &super::redstone::holding(items::SADDLE));
    let output = checked(&mut region, &click(1, camel, true));
    let (serial, kind, slots) = menu_opened(&output, 1).expect("the camel's menu opens");
    assert_eq!(
        kind,
        MenuKind::Mount {
            columns: 0,
            entity: camel
        }
    );
    assert_eq!(slots, 2 + 36);
    checked(&mut region, &container_click(serial, 2 + 27));
    let output = checked(&mut region, &container_click(serial, 0));
    assert_eq!(
        mob(&region, camel).body.equipment.slots[usize::from(slots::SADDLE)],
        Some(ItemStack {
            item: items::SADDLE,
            count: 1
        })
    );
    let saddled = heard(&output)
        .into_iter()
        .filter(|(sound, ..)| *sound == sounds::ENTITY_CAMEL_SADDLE)
        .count();
    assert_eq!(saddled, 2);
}

/// The inventory key on a nautilus its rider rides (`HasCustomInventoryScreen`): its
/// menu, which a second player's sneaking click does not open while another rides it.
#[test]
fn the_inventory_key_opens_a_ridden_nautilus_menu_for_its_rider_only() {
    let mut region = two_players();
    let key = nautilus_at(&mut region, false);
    checked(&mut region, &click(1, key, false));
    assert_eq!(riding(&region, 1), Some(key));
    let output = checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::OpenVehicleInventory)]),
    );
    let (_, kind, slots) = menu_opened(&output, 1).expect("its menu opens");
    assert_eq!(
        kind,
        MenuKind::Mount {
            columns: 0,
            entity: key
        }
    );
    assert_eq!(slots, 2 + 36);
    let output = checked(&mut region, &click(2, key, true));
    assert_eq!(menu_opened(&output, 2), None);
}
