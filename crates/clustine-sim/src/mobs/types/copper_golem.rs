//! The copper golem (`CopperGolem`, `CopperGolemState`, `CopperGolemOxidationLevels`,
//! `CopperGolemStatueBlock`; `docs/groundwork/n7-brief.md`, section 7.13, with 9.7.5 and
//! 10.7.5): a golem of copper that weathers a stage every 504,000 to 552,000 game ticks
//! and, oxidised, soon turns into a statue of itself where it stands in the air; a
//! honeycomb waxes it, which stops all of that, and an axe takes the wax off or scrapes
//! it back a stage. It is built from a carved pumpkin on a block of copper
//! (`region/copper_golems.rs`), or comes from its egg.
//!
//! Its mind is a brain in the game (`CopperGolemAi`: it sorts the items of copper chests
//! into chests), whose machinery is not built yet: until its provider is wired at
//! [`brain_seam`] a copper golem has no mind and stands where it is put. What the brain
//! reads and sets is kept here already: the first transport cooldown its constructor
//! draws ([`CopperGolemState::transport_cooldown`]), the chest it has open
//! ([`set_opened_chest_pos`], which the chests' openers count, [`has_container_open`]),
//! and its state shown to clients ([`set_state`]).
//!
//! Named differences: lightning strikes nothing yet (the weather is always clear), so
//! none scrapes a golem back; its statue keeps no name (no block entity's components),
//! and a leash it wore goes as the region's removal takes it; it holds nothing a player
//! can take until its brain gives it something to carry, and wears no poppy until N7k's
//! iron golem gives one.

use clustine_blocks::data::{item_in_tag, set};
use clustine_data::{BlockState, EntityTypeInfo, PathType, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::animal::SOUND_SOURCE_PLAYERS;
use super::{Did, Interaction, MobType, Used};
use crate::api::SOUND_SOURCE_BLOCKS;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::ai::navigation::Settings;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::{Event, Removed};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Serialiser};

/// The synced indices of `DATA_WEATHER_STATE` and `COPPER_GOLEM_STATE`.
pub(crate) const WEATHER_INDEX: u8 = 16;
pub(crate) const STATE_INDEX: u8 = 17;

/// `nextWeatheringTick`'s marks: waxed (`IGNORE_WEATHERING_TICK`) and not yet drawn
/// (`UNSET_WEATHERING_TICK`).
pub(crate) const WAXED: i64 = -2;
pub(crate) const UNSET: i64 = -1;

/// `WEATHERING_TICK_FROM` and `WEATHERING_TICK_TO`.
const WEATHERING_FROM: i32 = 504_000;
const WEATHERING_TO: i32 = 552_000;

/// `TURN_TO_STATUE_CHANCE`.
const TURN_TO_STATUE_CHANCE: f32 = 0.0058;

/// `SPAWN_COOLDOWN_MIN` and `SPAWN_COOLDOWN_MAX`: its first transport cooldown.
const SPAWN_COOLDOWN_MIN: i32 = 60;
const SPAWN_COOLDOWN_MAX: i32 = 100;

/// `getContainerInteractionRange()`: how far a chest's recheck looks for it.
pub(crate) const CONTAINER_INTERACTION_RANGE: f64 = 3.0;

/// `WeatheringCopper.WeatherState`, by its ordinal.
pub(crate) const UNAFFECTED: u8 = 0;
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) const EXPOSED: u8 = 1;
pub(crate) const WEATHERED: u8 = 2;
pub(crate) const OXIDIZED: u8 = 3;

/// The stages' names as the blocks of copper begin with them, by ordinal.
pub(crate) const STAGES: [&str; 4] = ["", "exposed_", "weathered_", "oxidized_"];

/// `CopperGolemState`, by its ordinal: what clients animate it by.
pub(crate) const IDLE: u8 = 0;
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) const GETTING_ITEM: u8 = 1;
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) const GETTING_NO_ITEM: u8 = 2;
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) const DROPPING_ITEM: u8 = 3;
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) const DROPPING_NO_ITEM: u8 = 4;

/// `CopperGolemStatueBlock.Pose` in its order (the block report's: the enum is not in
/// the dumps, the brief's 12.7.5), which `getNextPose` walks and `turnToStatue` draws
/// from.
pub(crate) const POSES: [&str; 4] = ["standing", "sitting", "running", "star"];

/// The version of the copper golem's type state.
const VERSION: u8 = 1;

/// What a copper golem keeps: its weathering (`DATA_WEATHER_STATE`) and its next
/// stage's game time (`nextWeatheringTick`, or [`WAXED`], [`UNSET`]), its state shown
/// to clients, the chest it has open (`openedChestPos`, which the game does not save
/// and Clustine does, so that a chest's recheck in another tick finds it), and the
/// first transport cooldown its constructor drew for its brain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CopperGolemState {
    pub(crate) weather: u8,
    pub(crate) next_weathering: i64,
    pub(crate) state: u8,
    pub(crate) opened_chest: Option<BlockPos>,
    /// `TRANSPORT_ITEMS_COOLDOWN_TICKS` as the constructor drew it, held for the brain to
    /// take as its memory once its provider is wired; a golem read fresh from disk has
    /// none (the brief's 7.0.5, finding 2).
    pub(crate) transport_cooldown: Option<i32>,
}

impl Default for CopperGolemState {
    /// The constructor's: unaffected and not yet drawn for, idle, no chest open.
    fn default() -> Self {
        Self {
            weather: UNAFFECTED,
            next_weathering: UNSET,
            state: IDLE,
            opened_chest: None,
            transport_cooldown: None,
        }
    }
}

/// The copper golem's state of `mob`: a fresh golem's for a record of nothing or one
/// this build cannot read.
pub(crate) fn state_of(mob: &Mob) -> CopperGolemState {
    read(&mob.state)
}

/// The copper golem's state of a type state.
pub(crate) fn read(state: &TypeState) -> CopperGolemState {
    if state.bytes.is_empty() || state.version != VERSION {
        return CopperGolemState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// The type state of `own`: nothing for a fresh golem's.
pub(crate) fn write(own: &CopperGolemState) -> TypeState {
    if *own == CopperGolemState::default() {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
    }
}

/// Changes the golem's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut CopperGolemState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = write(&own);
    }
}

/// `WeatherState.next()`: a stage more, oxidised staying.
pub(crate) fn next_stage(weather: u8) -> u8 {
    (weather + 1).min(OXIDIZED)
}

/// `WeatherState.previous()`: a stage less, unaffected staying.
pub(crate) fn previous_stage(weather: u8) -> u8 {
    weather.saturating_sub(1)
}

/// `setState(state)`, which the brain's transport hooks set (the seam).
pub(crate) fn set_state(mob: &mut Mob, state: u8) {
    update(mob, |own| own.state = state);
}

/// `setOpenedChestPos(pos)`, the brain's at the first tick of its interaction (the seam).
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) fn set_opened_chest_pos(mob: &mut Mob, pos: BlockPos) {
    update(mob, |own| own.opened_chest = Some(pos));
}

/// `clearOpenedChestPos()`, the brain's as it travels again (the seam).
#[allow(
    dead_code,
    reason = "what the brain's transport (part A) sets, which it wires"
)]
pub(crate) fn clear_opened_chest_pos(mob: &mut Mob) {
    update(mob, |own| own.opened_chest = None);
}

/// `hasContainerOpen(counter, pos)`: it has a chest open, at `pos` or at the other half
/// of the double chest it opened. `other_half` is `ChestBlock.getConnectedBlockPos` of
/// the chest at the position it opened, where that is a double chest.
pub(crate) fn has_container_open(
    own: &CopperGolemState,
    pos: BlockPos,
    other_half: impl FnOnce(BlockPos) -> Option<BlockPos>,
) -> bool {
    let Some(opened) = own.opened_chest else {
        return false;
    };
    opened == pos || other_half(opened) == Some(pos)
}

/// The seam of the brain (`customServerAiStep`'s `brain.tick` and
/// `CopperGolemAi.updateActivity`, `setActiveActivityToFirstValid([IDLE])`): nothing
/// yet. The provider (sensors `[NEAREST_LIVING_ENTITIES, HURT_BY]`, CORE and IDLE with
/// `TransportItemsBetweenContainers` at IDLE's first priority) is wired here with part A.
pub(crate) fn brain_seam(tick: &mut MobTick<'_>) {
    let _ = tick;
}

/// The copper golem.
pub(crate) struct CopperGolem;

/// The sounds of its weathering (`CopperGolemOxidationLevels`): hurt, death and step;
/// unaffected and exposed share theirs.
fn oxidation_sounds(weather: u8) -> [i32; 3] {
    match weather {
        WEATHERED => [
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_HURT,
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_DEATH,
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_STEP,
        ],
        OXIDIZED => [
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_HURT,
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_DEATH,
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_STEP,
        ],
        _ => [
            sounds::ENTITY_COPPER_GOLEM_HURT,
            sounds::ENTITY_COPPER_GOLEM_DEATH,
            sounds::ENTITY_COPPER_GOLEM_STEP,
        ],
    }
}

/// `nextIntBetweenInclusive(504000, 552000)`: the game ticks to its next stage.
fn weathering_ticks(level: &mut Random) -> i64 {
    i64::from(level.next_int_between(WEATHERING_FROM, WEATHERING_TO))
}

/// What `updateWeathering` decided beyond the state it changed: whether it was oxidised
/// as it began, so that it may turn into a statue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Weathered {
    pub(crate) was_oxidised: bool,
}

/// `updateWeathering(level, random, t)` of the state alone, of the level's random at the
/// game time `t` (the brief's 7.13.2): waxed, nothing; not yet drawn, its next stage
/// drawn from now; at or past its next stage and not oxidised, a stage more, the next
/// drawn from the last one's time (so that a golem long unticked catches up a stage a
/// tick), or 0 once oxidised.
pub(crate) fn update_weathering(
    own: &mut CopperGolemState,
    level: &mut Random,
    t: i64,
) -> Option<Weathered> {
    if own.next_weathering == WAXED {
        return None;
    }
    if own.next_weathering == UNSET {
        own.next_weathering = t + weathering_ticks(level);
        return None;
    }
    let was_oxidised = own.weather == OXIDIZED;
    if t >= own.next_weathering && !was_oxidised {
        let next = next_stage(own.weather);
        own.weather = next;
        own.next_weathering = if next == OXIDIZED {
            0
        } else {
            own.next_weathering + weathering_ticks(level)
        };
    }
    Some(Weathered { was_oxidised })
}

/// `Direction.fromYRot(yaw)`'s name: the quarter turn nearest, south first.
pub(crate) fn facing_of_yaw(yaw: f32) -> &'static str {
    let quarter = (f64::from(yaw) / 90.0 + 0.5).floor() as i64 & 3;
    ["south", "west", "north", "east"][quarter as usize]
}

/// The oxidised statue (`Blocks.COPPER_GOLEM_STATUE.weathering().oxidized()`, the
/// unwaxed one) in the pose of the ordinal `pose`, facing `facing`.
pub(crate) fn statue(pose: usize, facing: &str) -> BlockState {
    let state = set(
        blocks::OXIDIZED_COPPER_GOLEM_STATUE,
        "copper_golem_pose",
        POSES[pose % POSES.len()],
    );
    set(state, "facing", facing)
}

/// `CopperGolemStatueBlock.updatePose`'s next state: the statue in the pose after its
/// own (`Pose.getNextPose`, the last back to the first).
pub(crate) fn next_pose(state: BlockState) -> BlockState {
    let at = state
        .property("copper_golem_pose")
        .and_then(|pose| POSES.iter().position(|known| *known == pose))
        .unwrap_or(0);
    set(state, "copper_golem_pose", POSES[(at + 1) % POSES.len()])
}

/// `tick()` after the golem's whole tick, on the server: its weathering, of the level's
/// random at the world's game time, and an oxidised golem in the air at its feet
/// turning into a statue one tick in about 172.
fn after_tick(tick: &mut MobTick<'_>) {
    let now = tick.world.game_time();
    let mut own = state_of(&tick.mob);
    let weathered = update_weathering(&mut own, tick.draws, now);
    tick.mob.state = write(&own);
    if !weathered.is_some_and(|weathered| weathered.was_oxidised) {
        return;
    }
    // `canTurnToStatue`: air at its block, and only then the draw.
    let at = tick.living.body.block_position();
    if !tick.world.state(at).props().is_air() {
        return;
    }
    if tick.draws.next_float() > TURN_TO_STATUE_CHANCE {
        return;
    }
    turn_to_statue(tick, at);
}

/// `turnToStatue(level)`: the oxidised statue at its block in a pose of its own draw,
/// facing its way, with every update; what it is sure to drop (what it carried, its
/// poppy) laid where it stood; and it gone, with its sound.
fn turn_to_statue(tick: &mut MobTick<'_>, at: BlockPos) {
    let pose = tick.draws.next_int(POSES.len() as u32) as usize;
    let state = statue(pose, facing_of_yaw(tick.yaw));
    tick.out.local.push(Local::SetBlock {
        pos: at,
        state,
        // `setBlockAndUpdate`: `UPDATE_ALL`.
        flags: 3,
    });
    // `dropPreservedEquipment`: each slot whose drop is guaranteed.
    let position = tick.living.body.position;
    for slot in 0..tick.mob.body.equipment.slots.len() {
        let Some(stack) = tick.mob.body.equipment.slots[slot] else {
            continue;
        };
        if stack.count > 0 && tick.mob.body.equipment.drop_chance(slot) > 1.0 {
            tick.mob.body.equipment.slots[slot] = None;
            tick.out.local.push(Local::Drop {
                stack,
                at: position,
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            });
        }
    }
    tick.out.removed = Some(Removed::Discarded);
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_COPPER_GOLEM_BECOME_STATUE,
        volume: 1.0,
        pitch: 1.0,
    });
}

/// `BehaviorUtils.throwItem(golem, stack, player.position())`: from its eyes less 0.3,
/// at 0.3 a tick towards the player's feet.
fn throw_towards(used: &Used<'_>, stack: ItemStack) -> Did {
    let eye = f64::from(EntityTypeInfo::of(entity_types::COPPER_GOLEM).eye_height);
    let [x, y, z] = used.position;
    let towards = [used.from[0] - x, used.from[1] - y, used.from[2] - z];
    let length =
        (towards[0] * towards[0] + towards[1] * towards[1] + towards[2] * towards[2]).sqrt();
    let motion = if length < 1.0e-5 {
        [0.0; 3]
    } else {
        towards.map(|axis| axis / length * 0.300_000_011_920_928_96)
    };
    Did::Throw {
        stack,
        at: [x, y + eye - 0.300_000_011_920_928_96, z],
        pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
        motion,
    }
}

/// A sound of a use, heard by everyone, at full volume and pitch.
fn sound(sound: i32, source: i32, at: Vec3) -> Did {
    Did::Sound {
        sound,
        source,
        at,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    }
}

/// `CopperGolem.mobInteract` (the brief's 7.13.3), in its order. A creative player's
/// honeycomb is kept and their axe not worn, and every player is one.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let held = used.held.filter(|stack| stack.count > 0);
    let hand = usize::from(slots::MAIN_HAND);
    if held.is_none()
        && let Some(carried) = used.mob.body.equipment.slots[hand].filter(|stack| stack.count > 0)
    {
        let thrown = throw_towards(used, carried);
        used.did.push(thrown);
        used.mob.body.equipment.slots[hand] = None;
        return Interaction::Success;
    }
    let antenna = usize::from(slots::SADDLE);
    let ready_for_shearing = used.mob.body.equipment.slots[antenna].is_some_and(|stack| {
        stack.count > 0 && item_in_tag(stack.item, "minecraft:shearable_from_copper_golem")
    });
    if held.is_some_and(|stack| stack.item == items::SHEARS) && ready_for_shearing {
        // `shear`: its sound, the antenna emptied and dropped half a block up and more.
        used.did.push(sound(
            sounds::ENTITY_COPPER_GOLEM_SHEAR,
            SOUND_SOURCE_PLAYERS,
            used.position,
        ));
        if let Some(poppy) = used.mob.body.equipment.slots[antenna].take() {
            let [x, y, z] = used.position;
            used.did.push(Did::Drop {
                stack: poppy,
                at: [x, y + 1.5, z],
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                push: None,
            });
        }
        used.did.push(Did::GameEvent("minecraft:shear"));
        return Interaction::Success;
    }
    let block = crate::physics::containing(used.position);
    let own = state_of(used.mob);
    if held.is_some_and(|stack| stack.item == items::HONEYCOMB) && own.next_weathering != WAXED {
        used.did.push(Did::LevelEvent {
            kind: 3003,
            pos: block,
            data: 0,
        });
        let centre = [
            f64::from(block.x) + 0.5,
            f64::from(block.y) + 0.5,
            f64::from(block.z) + 0.5,
        ];
        used.did.push(sound(
            sounds::ITEM_HONEYCOMB_WAX_ON,
            SOUND_SOURCE_BLOCKS,
            centre,
        ));
        update(used.mob, |own| own.next_weathering = WAXED);
        return Interaction::SuccessServer;
    }
    let axe = held.is_some_and(|stack| item_in_tag(stack.item, "minecraft:axes"));
    if axe && own.next_weathering == WAXED {
        used.did.push(sound(
            sounds::ITEM_AXE_SCRAPE,
            SOUND_SOURCE_NEUTRAL,
            used.position,
        ));
        used.did.push(Did::LevelEvent {
            kind: 3004,
            pos: block,
            data: 0,
        });
        update(used.mob, |own| own.next_weathering = UNSET);
        return Interaction::SuccessServer;
    }
    if axe && own.weather != UNAFFECTED {
        used.did.push(sound(
            sounds::ITEM_AXE_SCRAPE,
            SOUND_SOURCE_NEUTRAL,
            used.position,
        ));
        used.did.push(Did::LevelEvent {
            kind: 3005,
            pos: block,
            data: 0,
        });
        update(used.mob, |own| {
            own.next_weathering = UNSET;
            own.weather = previous_stage(own.weather);
        });
        return Interaction::SuccessServer;
    }
    Interaction::Pass
}

/// The first transport cooldown its constructor draws (`nextInt(60, 100)`), of its own
/// random.
pub(crate) fn first_transport_cooldown(own: &mut Random) -> i32 {
    SPAWN_COOLDOWN_MIN + own.next_int((SPAWN_COOLDOWN_MAX - SPAWN_COOLDOWN_MIN) as u32) as i32
}

impl MobType for CopperGolem {
    /// What its constructor sets of its AI: a path of up to 48 blocks that opens doors,
    /// fire beside its way costly and fire itself barred.
    fn register_goals(&self, setup: &mut Setup) {
        let mut navigation = Settings::default();
        navigation.set_required_path_length(48.0);
        navigation.flags.can_open_doors = true;
        setup.config.navigation = navigation;
        setup.set_malus(PathType::FireInNeighbor, 16.0);
        setup.set_malus(PathType::DamagingInNeighbor, 16.0);
        setup.set_malus(PathType::Fire, -1.0);
    }

    /// Its constructor: kept wherever it is, idle, and its first transport cooldown
    /// drawn.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        mob.body.flags.persistent = true;
        let cooldown = first_transport_cooldown(own);
        update(mob, |state| state.transport_cooldown = Some(cooldown));
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        brain_seam(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        after_tick(tick);
    }

    /// `actuallyHurt`: idle again (after the body's in the game, which reads nothing of
    /// it).
    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        set_state(mob, IDLE);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    fn spawn_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_COPPER_GOLEM_SPAWN)
    }

    fn step_sound(&self, mob: &Mob) -> Option<i32> {
        Some(oxidation_sounds(state_of(mob).weather)[2])
    }

    /// `playSound(step, 1, 1)`.
    fn step_volume(&self) -> f32 {
        1.0
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(oxidation_sounds(state_of(mob).weather)[0])
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(oxidation_sounds(state_of(mob).weather)[1])
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        vec![
            (
                WEATHER_INDEX,
                DataShown::Serialised(Serialiser::WeatheringCopperState, i32::from(own.weather)),
                own.weather == UNAFFECTED,
            ),
            (
                STATE_INDEX,
                DataShown::Serialised(Serialiser::CopperGolemState, i32::from(own.state)),
                own.state == IDLE,
            ),
        ]
    }
}
