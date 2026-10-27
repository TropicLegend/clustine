//! What the tests of N9's part A from the specification share (`docs/groundwork/
//! n9-plan.md`, section 14): a stone floor at y = -61 on land held alone or on either
//! side of a pinned line at block x = 16, regions whose own tick numbers and world
//! clocks a test chooses, players who join them and act through their inputs, and the
//! store's part of carrying what one region's tick does to another's entities and
//! players, played by hand.
//!
//! Written from the plan and the public inputs and outputs of a region; the region's
//! insides are read only where a test puts something in place (a mob in a chunk).

use clustine_world::components::{Component, EffectInstance, PotionContents};
use clustine_world::{Due, Effect, EffectId, FiledEffect};

use super::*;
use crate::api::{FaceChange, PlayerFace, WorldClock};

/// The world these tests' regions draw from.
pub(super) const SEED: u64 = 0x5_13A0;

/// The regions on either side of the line at block x = 16, chunk x = 1.
pub(super) const WEST_ID: RegionId = RegionId(0);
pub(super) const EAST_ID: RegionId = RegionId(1);

/// The chunks a region of these tests holds and loads, with `x` in `xs`.
fn columns(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A region that holds and has loaded `held`, each a stone floor, is pinned to
/// `pinned`, has copies of `copied` and believes `foreign` to be `other`'s; its ticks
/// numbered on from `tick`, its world clock at `clock`, its entity ids from the block
/// `block`. It has ticked once.
pub(super) fn region_with(
    held: &[ChunkPos],
    pinned: Vec<ChunkArea>,
    copied: &[ChunkPos],
    other: RegionId,
    block: u32,
    tick: u64,
    clock: WorldClock,
) -> Region {
    let mut state = knowing_the_edges(EntityIds::block(block).expect("a block of ids"));
    state.tick = tick;
    state.clock = clock;
    let holdings = Holdings {
        held: held.to_vec(),
        pinned,
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let chunks = held.iter().map(|position| (*position, floor())).collect();
    let copies = copied.iter().map(|position| (*position, floor())).collect();
    let mut region = Region::restore_loaded(
        RegionConfig {
            return_after: u64::MAX,
            seed: SEED,
            ..config()
        },
        state,
        holdings,
        &tickets,
        chunks,
        copies,
    );
    // The chunks across the line are wanted, so that the region claims them and
    // believes the store that they are the other's: a player who steps into them is
    // let go there.
    region.tick(&TickInputs {
        tickets_added: copied
            .iter()
            .map(|position| (*position, Ticket::Viewer))
            .collect(),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        foreign: copied.iter().map(|position| (*position, other)).collect(),
        ..TickInputs::default()
    });
    region
}

/// The land about the origin, 5 by 5 chunks, held by one region alone.
pub(super) fn alone(tick: u64, clock: WorldClock) -> Region {
    region_with(
        &columns(-2..=2),
        vec![ChunkArea::EVERYWHERE],
        &[],
        EAST_ID,
        0,
        tick,
        clock,
    )
}

/// The land west of the line at block x = 16, with copies of the column east of it.
pub(super) fn west(tick: u64, clock: WorldClock) -> Region {
    region_with(
        &columns(-2..=0),
        vec![ChunkArea {
            min_x: None,
            max_x: Some(1),
        }],
        &columns(1..=1),
        EAST_ID,
        1,
        tick,
        clock,
    )
}

/// The land east of the line, with copies of the column west of it.
pub(super) fn east(tick: u64, clock: WorldClock) -> Region {
    region_with(
        &columns(1..=3),
        vec![ChunkArea {
            min_x: Some(1),
            max_x: None,
        }],
        &columns(0..=0),
        WEST_ID,
        2,
        tick,
        clock,
    )
}

/// A world clock `game_time` ticks into the world, its day as long.
pub(super) fn clock_at(game_time: u64) -> WorldClock {
    WorldClock {
        game_time,
        day: game_time,
    }
}

/// The place a player enters at: standing at `feet`, holding nothing, with `effects`
/// and the ticks since they last slept.
pub(super) fn entering_place(
    feet: Vec3,
    effects: Vec<EffectInstance>,
    time_since_rest: u32,
) -> Place {
    Place {
        pose: Pose::at(feet),
        flying: false,
        inventory: Inventory::default(),
        selected_slot: 0,
        ender: Default::default(),
        experience: Default::default(),
        respawn: None,
        effects,
        time_since_rest,
        raid_omen_position: None,
    }
}

/// Player `number` joins `region` and enters it where `place` says, as the store
/// answers for a player it has a place for. Returns the outputs of the two ticks.
pub(super) fn enter_with(region: &mut Region, number: u128, place: Place) -> Vec<TickOutput> {
    let first = region.tick(&changes(vec![join(number)]));
    let entity = region
        .entering_state(player(number))
        .expect("the join is entering")
        .entity_id;
    let second = region.tick(&TickInputs {
        entered: vec![Entered {
            unit: None,
            player: player(number),
            entity,
            place: Some(place),
            holder: None,
        }],
        ..TickInputs::default()
    });
    assert!(
        region.player(player(number)).is_some(),
        "player {number} entered"
    );
    vec![first, second]
}

/// `input` of player `number` as their edge passes it on to `region`: numbered after
/// every input made before, naming the entity they have there.
pub(super) fn act(region: &Region, number: u128, input: PlayerInput) -> Input {
    let (edge, id, _, serial, input) = numbered(number, input);
    let (entity, _) = region
        .player(id)
        .unwrap_or_else(|| panic!("player {number} is in the region"));
    (edge, id, entity, serial, input)
}

/// One tick of `region` with `inputs` of player `number`.
pub(super) fn acting(region: &mut Region, number: u128, inputs: Vec<PlayerInput>) -> TickOutput {
    let inputs = inputs
        .into_iter()
        .map(|input| act(region, number, input))
        .collect();
    region.tick(&moves(inputs))
}

/// Player `number` holds `stack` in the first slot of the hotbar, set from the creative
/// inventory.
pub(super) fn hold(region: &mut Region, number: u128, stack: ItemStack) -> TickOutput {
    acting(
        region,
        number,
        vec![
            PlayerInput::SetInventorySlot {
                slot: 0,
                stack: Some(stack),
            },
            PlayerInput::SelectSlot { slot: 0 },
        ],
    )
}

/// A use of what the main hand holds, looking `pitch` down.
pub(super) fn use_item(pitch: f32) -> PlayerInput {
    PlayerInput::UseItem {
        rotation: (0.0, pitch),
        sneaking: false,
        sequence: 1,
    }
}

/// `EffectInstance` of `effect` by its name, as an item or a place carries it.
pub(super) fn instance(effect: &str, amplifier: i32, duration: i32) -> EffectInstance {
    EffectInstance {
        effect: effect.to_owned(),
        amplifier,
        duration,
        ambient: false,
        visible: true,
        show_icon: true,
    }
}

/// A stack of `item` (a potion of one of its kinds) whose contents are `effects` alone.
pub(super) fn potion(item: i32, effects: Vec<EffectInstance>) -> ItemStack {
    let mut stack = ItemStack::new(item, 1);
    stack
        .components
        .set(Component::PotionContents(PotionContents {
            potion: None,
            custom_colour: None,
            custom_effects: effects,
            custom_name: None,
        }));
    stack
}

/// The events of player `number` in `output`.
pub(super) fn told_to(output: &TickOutput, number: u128) -> Vec<PlayerEvent> {
    output
        .player_events
        .iter()
        .filter(|(to, _)| *to == player(number))
        .map(|(_, event)| event.clone())
        .collect()
}

/// The effects added for player `number` in `output`, as they were told.
pub(super) fn effects_added(output: &TickOutput, number: u128) -> Vec<EffectInstance> {
    told_to(output, number)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::EffectAdded { instance, .. } => Some(instance),
            _ => None,
        })
        .collect()
}

/// The effects ended for player `number` in `output`, by name.
pub(super) fn effects_removed(output: &TickOutput, number: u128) -> Vec<String> {
    told_to(output, number)
        .into_iter()
        .filter_map(|event| match event {
            PlayerEvent::EffectRemoved { effect } => Some(effect),
            _ => None,
        })
        .collect()
}

/// Ticks `region` with nothing until `done` holds of a tick's output, at most `most`
/// ticks, and returns how many it ticked, the last among them.
pub(super) fn ticks_until(
    region: &mut Region,
    most: u32,
    mut done: impl FnMut(&TickOutput) -> bool,
) -> Option<u32> {
    (1..=most).find(|_| {
        let output = region.tick(&TickInputs::default());
        done(&output)
    })
}

/// `effects` filed for their holder, numbered from `sequence`.
pub(super) fn filed(effects: Vec<Effect>, sequence: u64) -> Vec<FiledEffect> {
    effects
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
        .collect()
}

/// The face of player `number`, whose stay is the entity `entity` in another region,
/// standing at `feet`, as the store streams it.
pub(super) fn face_of(number: u128, entity: EntityId, feet: [f64; 3], seq: u64) -> FaceChange {
    FaceChange::Face {
        player: player(number),
        entity,
        hops: 0,
        seq: crate::api::FaceSeq(seq, 0),
        face: PlayerFace::of(
            &Pose::at(Vec3::new(feet[0], feet[1], feet[2])),
            false,
            false,
            &Inventory::default(),
            0,
            None,
        ),
    }
}

/// The players `output` let go, each with their transfer.
pub(super) fn departures(output: &TickOutput) -> Vec<(PlayerId, PlayerTransfer)> {
    output
        .durable
        .iter()
        .filter_map(|(_, _, entry)| match entry {
            Durable::Departed {
                player, transfer, ..
            } => Some((*player, transfer.clone())),
            _ => None,
        })
        .collect()
}

/// The last place `outputs` noted for player `number`'s stay, which is what the store
/// keeps of them when they leave.
pub(super) fn last_place(outputs: &[TickOutput], number: u128) -> Option<Place> {
    outputs
        .iter()
        .rev()
        .flat_map(|output| output.stays.iter().rev())
        .find_map(|note| match note {
            StayNote::Has {
                player: of, place, ..
            } if *of == player(number) => Some(place.clone()),
            _ => None,
        })
}
