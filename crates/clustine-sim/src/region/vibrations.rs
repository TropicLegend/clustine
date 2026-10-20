//! Game events, the vibrations that sculk sensors hear (step N6f of
//! `docs/groundwork/n6-plan.md`; `docs/groundwork/n6-brief.md`, sections 8.0 to 8.7), as
//! the 26.3 jar's `GameEvent`, `GameEventDispatcher`, `VibrationSystem` and
//! `VibrationSelector` have them. The game calls them game events; here they are
//! vibrations, as the protocol's `GameEvent` is the packet of a change of the game's
//! state.
//!
//! A post (`Level.gameEvent(event, position, context)`) goes at once to every listener
//! about it, in the order of their positions: each sensor's block entity whose block is
//! within its radius of the event's block (8, or 16 for a calibrated sensor), measured
//! between blocks. A sensor takes the event up as a candidate when it holds no
//! vibration, the event is one it hears (`#vibrations`), its source neither steps
//! carefully for an event that sneaking hides nor dampens it, its block is not of wool,
//! the sensor hears it ([`Region::sensor_can_receive`]) and not all six of the lines
//! from the event's block to the sensor's, each nudged a little along one direction,
//! meet wool. Of the candidates of one tick it keeps the closest, and of two as close
//! the one of the higher frequency. Its ticker takes the candidate up in the next tick
//! (or the same one, for what a player's packet posts between ticks), shows its particle
//! travelling for `floor(distance)` ticks, and receives it when the travel is done.
//!
//! The order among listeners does not matter, as each has a selector of its own and
//! changes nothing the others read.
//!
//! A player's steps and landings are posted from the moves their client reports
//! ([`Region::player_moved`]), as the game's `handleMovePlayer` runs `Entity.move` and
//! `doCheckFallDamage` on them, with sneaking from the keys the client last sent.
//! Named differences: the region does not collide a player's move, so it takes the
//! client's move whole (the game would clip one that ran into a block); the block a
//! player stands on is the one under their feet, with no supporting block beside it
//! found at an edge; a player is in water where the block at their feet holds water;
//! what the game keeps of a player's steps and fall (`moveDist`, `nextStep`,
//! `fallDistance`) is kept in memory only, so a player restored, handed over, merged or
//! split starts with none, as one who joins does.
//!
//! At a line (`docs/groundwork/n6-plan.md`, decision 10): an event is heard only by the
//! region's own sensors, in the chunks it has loaded, as the game's dispatch finds
//! listeners only in loaded chunks and a copy has none. A sensor of another region's
//! within reach of an event does not hear it (named): the effect that would carry the
//! event to the sensor's holder is a later step's. The occlusion reads the blocks along
//! the lines in the region's chunks and copies, and a block it can read in neither does
//! not occlude. A sensor's wait for its 3×3 chunks to tick is read as its own chunk
//! ticking (ADR-0025, section 3), which its ticker already asks.

use clustine_blocks::data::{has_water, in_tag, is_class};
use clustine_data::{BlockClass, BlockState, TAGS, blocks};
use clustine_world::{BlockPos, Double, EntityId, Float, PlayerId};

use super::Region;
use super::sculk::{self, Heard, listener_position};
use crate::api::{RegionEvent, TickOutput};
use crate::physics::body::climbable;
use crate::physics::walk::traverse;

/// A game event, by its id in the game's registry `minecraft:game_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Vibration(u8);

/// The names of the game events by id, in the order of the registry.
const NAMES: [&str; 61] = [
    "minecraft:block_activate",
    "minecraft:block_attach",
    "minecraft:block_change",
    "minecraft:block_close",
    "minecraft:block_deactivate",
    "minecraft:block_destroy",
    "minecraft:block_detach",
    "minecraft:block_open",
    "minecraft:block_place",
    "minecraft:bounce",
    "minecraft:container_close",
    "minecraft:container_open",
    "minecraft:drink",
    "minecraft:eat",
    "minecraft:elytra_glide",
    "minecraft:entity_damage",
    "minecraft:entity_die",
    "minecraft:entity_dismount",
    "minecraft:entity_interact",
    "minecraft:entity_mount",
    "minecraft:entity_place",
    "minecraft:entity_action",
    "minecraft:equip",
    "minecraft:explode",
    "minecraft:flap",
    "minecraft:fluid_pickup",
    "minecraft:fluid_place",
    "minecraft:hit_ground",
    "minecraft:instrument_play",
    "minecraft:item_interact_finish",
    "minecraft:item_interact_start",
    "minecraft:jukebox_play",
    "minecraft:jukebox_stop_play",
    "minecraft:lightning_strike",
    "minecraft:note_block_play",
    "minecraft:prime_fuse",
    "minecraft:projectile_land",
    "minecraft:projectile_shoot",
    "minecraft:sculk_sensor_tendrils_clicking",
    "minecraft:shear",
    "minecraft:shriek",
    "minecraft:splash",
    "minecraft:step",
    "minecraft:swim",
    "minecraft:teleport",
    "minecraft:unequip",
    "minecraft:resonate_1",
    "minecraft:resonate_2",
    "minecraft:resonate_3",
    "minecraft:resonate_4",
    "minecraft:resonate_5",
    "minecraft:resonate_6",
    "minecraft:resonate_7",
    "minecraft:resonate_8",
    "minecraft:resonate_9",
    "minecraft:resonate_10",
    "minecraft:resonate_11",
    "minecraft:resonate_12",
    "minecraft:resonate_13",
    "minecraft:resonate_14",
    "minecraft:resonate_15",
];

/// `VibrationSystem.VIBRATION_FREQUENCY_FOR_EVENT`, by id: 0 for an event no sensor
/// hears, and the resonances' own.
const FREQUENCIES: [u8; 61] = [
    10, 10, 11, 9, 9, 12, 9, 10, 13, 2, // block_activate .. bounce
    9, 10, 8, 8, 4, 7, 15, 5, 6, 6, // container_close .. entity_mount
    14, 4, 5, 15, 1, 12, 13, 2, 3, 3, // entity_place .. item_interact_finish
    0, 0, 0, 14, 10, 10, 2, 3, 0, 6, // item_interact_start .. shear
    0, 2, 1, 1, 14, 4, // shriek .. unequip
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, // resonate_1 .. 15
];

impl Vibration {
    pub(crate) const BLOCK_ACTIVATE: Vibration = Vibration(0);
    pub(crate) const BLOCK_ATTACH: Vibration = Vibration(1);
    pub(crate) const BLOCK_CHANGE: Vibration = Vibration(2);
    pub(crate) const BLOCK_CLOSE: Vibration = Vibration(3);
    pub(crate) const BLOCK_DEACTIVATE: Vibration = Vibration(4);
    pub(crate) const BLOCK_DESTROY: Vibration = Vibration(5);
    pub(crate) const BLOCK_DETACH: Vibration = Vibration(6);
    pub(crate) const BLOCK_OPEN: Vibration = Vibration(7);
    pub(crate) const BLOCK_PLACE: Vibration = Vibration(8);
    pub(crate) const CONTAINER_CLOSE: Vibration = Vibration(10);
    pub(crate) const CONTAINER_OPEN: Vibration = Vibration(11);
    pub(crate) const ENTITY_DISMOUNT: Vibration = Vibration(17);
    pub(crate) const ENTITY_PLACE: Vibration = Vibration(20);
    pub(crate) const ENTITY_MOUNT: Vibration = Vibration(19);
    pub(crate) const EXPLODE: Vibration = Vibration(23);
    pub(crate) const FLUID_PICKUP: Vibration = Vibration(25);
    pub(crate) const FLUID_PLACE: Vibration = Vibration(26);
    pub(crate) const HIT_GROUND: Vibration = Vibration(27);
    pub(crate) const JUKEBOX_PLAY: Vibration = Vibration(31);
    pub(crate) const JUKEBOX_STOP_PLAY: Vibration = Vibration(32);
    pub(crate) const NOTE_BLOCK_PLAY: Vibration = Vibration(34);
    pub(crate) const PRIME_FUSE: Vibration = Vibration(35);
    pub(crate) const SCULK_SENSOR_TENDRILS_CLICKING: Vibration = Vibration(38);
    pub(crate) const SHRIEK: Vibration = Vibration(40);
    pub(crate) const STEP: Vibration = Vibration(42);
    pub(crate) const SWIM: Vibration = Vibration(43);
    const RESONATE_1: u8 = 46;

    /// `VibrationSystem.getResonanceEventByFrequency`: the resonance of `frequency`,
    /// 1 to 15.
    pub(crate) fn resonance(frequency: u8) -> Option<Vibration> {
        (1..=15)
            .contains(&frequency)
            .then(|| Vibration(Self::RESONATE_1 + frequency - 1))
    }

    /// Its name in the registry, as a saved vibration names it.
    pub(crate) fn name(self) -> &'static str {
        NAMES[usize::from(self.0)]
    }

    /// The event of the registry name `name`.
    pub(crate) fn named(name: &str) -> Option<Vibration> {
        let id = NAMES.iter().position(|known| *known == name)?;
        Some(Vibration(u8::try_from(id).expect("61 events")))
    }

    /// `VibrationSystem.getGameEventFrequency`.
    pub(crate) fn frequency(self) -> u8 {
        FREQUENCIES[usize::from(self.0)]
    }

    /// `GameEvent.notificationRadius`: how far about the event the dispatch looks.
    pub(crate) fn radius(self) -> i32 {
        match self {
            Vibration::JUKEBOX_PLAY | Vibration::JUKEBOX_STOP_PLAY => 10,
            Vibration::SHRIEK => 32,
            _ => 16,
        }
    }

    /// Whether it is in the game event tag `name`.
    fn in_tag(self, name: &str) -> bool {
        TAGS.iter()
            .find(|tags| tags.registry == "minecraft:game_event")
            .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
            .is_some_and(|tag| tag.entries.contains(&i32::from(self.0)))
    }
}

/// The entity a vibration comes of, as a listener reads it: its key, whether it steps
/// carefully (`isSteppingCarefully`: a player with the shift key down) and whether it
/// dampens vibrations (an item of wool). No entity of Clustine's is a spectator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) entity: EntityId,
    pub(crate) careful: bool,
    pub(crate) dampens: bool,
}

impl Source {
    /// An entity that neither steps carefully nor dampens.
    pub(crate) fn of(entity: EntityId) -> Source {
        Source {
            entity,
            careful: false,
            dampens: false,
        }
    }

    /// An item entity of `item`, which dampens where the item is of wool.
    pub(crate) fn item(entity: EntityId, item: i32) -> Source {
        Source {
            dampens: clustine_blocks::data::item_in_tag(item, "minecraft:dampens_vibrations"),
            ..Source::of(entity)
        }
    }
}

/// `GameEvent.Context`: the entity the event comes of and the block it affects, either
/// of which may be missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Context {
    pub(crate) source: Option<Source>,
    pub(crate) affected: Option<BlockState>,
}

impl Context {
    pub(crate) fn of(source: Option<Source>, affected: Option<BlockState>) -> Context {
        Context { source, affected }
    }
}

/// What the game keeps of a player's steps and fall, in memory only: `moveDist`,
/// `nextStep` and `fallDistance`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Steps {
    moved: f32,
    next: f32,
    fall: f64,
}

impl Steps {
    /// `fallDistance` as the player's moves left it, which a critical hit reads.
    pub(crate) fn fall(&self) -> f64 {
        self.fall
    }
}

impl Default for Steps {
    fn default() -> Steps {
        Steps {
            moved: 0.0,
            next: 1.0,
            fall: 0.0,
        }
    }
}

/// The middle of the block at `pos` (`Vec3.atCenterOf`), where a post of a block's
/// position is made.
pub(crate) fn block_centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// `BlockPos.containing`.
fn containing(at: [f64; 3]) -> BlockPos {
    BlockPos::new(
        at[0].floor() as i32,
        at[1].floor() as i32,
        at[2].floor() as i32,
    )
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// `BlockPos.distSqr`, between two blocks.
fn block_distance_sqr(a: BlockPos, b: BlockPos) -> i64 {
    let d = |a: i32, b: i32| i64::from(a - b).pow(2);
    d(a.x, b.x) + d(a.y, b.y) + d(a.z, b.z)
}

/// `VibrationSystem.User.isValidVibration`, the default every sensor keeps: an event in
/// `#vibrations`, of no entity that steps carefully for an event sneaking hides nor that
/// dampens, and affecting no block that dampens.
fn is_valid(event: Vibration, context: &Context) -> bool {
    if !event.in_tag("minecraft:vibrations") {
        return false;
    }
    if let Some(source) = context.source {
        if source.careful && event.in_tag("minecraft:ignore_vibrations_sneaking") {
            return false;
        }
        if source.dampens {
            return false;
        }
    }
    !context
        .affected
        .is_some_and(|state| in_tag(state, "minecraft:dampens_vibrations"))
}

/// `VibrationSelector.shouldReplaceVibration`: nothing held, or a candidate of the same
/// tick strictly closer, or as close and of a strictly higher frequency.
fn replaces(held: Option<&(Heard, i64)>, candidate: &Heard, tick: i64) -> bool {
    let Some((held, held_tick)) = held else {
        return true;
    };
    if *held_tick != tick {
        return false;
    }
    let (new, old) = (candidate.distance.0, held.distance.0);
    if new < old {
        return true;
    }
    if new > old {
        return false;
    }
    // `fcmpl`/`fcmpg`: a NaN on either side is neither closer nor as close.
    new == old && candidate.event.frequency() > held.event.frequency()
}

impl Region {
    /// The game time a vibration posted now counts from: a player's packet is handled
    /// between two ticks, at the time of the one before, and anything else in the tick.
    /// It is the world clock's (`docs/groundwork/n7-plan.md`, section 7.4.5), so that a
    /// candidate kept in a sensor means the same after a merge, a split or a restore in
    /// another region, whose own count of ticks is another; an effect's tick is the
    /// region's, as many ticks before the clock's now as before its own.
    fn game_time(&self) -> i64 {
        let now = self.clock_now();
        self.made_in
            .map_or(now, |made_in| now - (self.tick as i64 - made_in))
    }

    /// The world clock's game time of this tick.
    fn clock_now(&self) -> i64 {
        i64::try_from(self.clock.game_time).unwrap_or(i64::MAX)
    }

    /// `Level.gameEvent(event, position, context)`: every sensor of the region's about
    /// `at` judges the event.
    pub(crate) fn vibrate(&mut self, event: Vibration, at: [f64; 3], context: Context) {
        if !at.iter().all(|value| value.is_finite()) {
            return;
        }
        let origin = containing(at);
        for pos in self.sensors_about(origin, event.radius()) {
            let Some(state) = self.own_block(pos) else {
                continue;
            };
            let radius = i64::from(sculk::listener_radius(state));
            if block_distance_sqr(pos, origin) <= radius * radius {
                self.handle_vibration(pos, state, event, at, &context);
            }
        }
    }

    /// [`Self::vibrate`] at the middle of the block at `pos`.
    pub(crate) fn vibrate_at(&mut self, event: Vibration, pos: BlockPos, context: Context) {
        self.vibrate(event, block_centre(pos), context);
    }

    /// `VibrationSystem.Listener.handleGameEvent` of the sensor `state` at `pos`.
    fn handle_vibration(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        event: Vibration,
        at: [f64; 3],
        context: &Context,
    ) {
        let Some(sensor) = self.sensor(pos) else {
            return;
        };
        if sensor.current.is_some() || !is_valid(event, context) {
            return;
        }
        if !self.sensor_can_receive(pos, state, containing(at), event) {
            return;
        }
        if self.occluded(at, listener_position(pos)) {
            return;
        }
        let source = context.source.map(|source| source.entity);
        self.schedule_vibration(pos, event, at, source);
    }

    /// `VibrationSystem.Listener.scheduleVibration`: the event becomes the sensor's
    /// candidate of this tick if it replaces the one it has.
    pub(super) fn schedule_vibration(
        &mut self,
        pos: BlockPos,
        event: Vibration,
        at: [f64; 3],
        source: Option<EntityId>,
    ) {
        let Some(mut sensor) = self.sensor(pos) else {
            return;
        };
        let heard = Heard {
            event,
            distance: Float(distance(at, listener_position(pos)) as f32),
            pos: at.map(Double),
            source,
        };
        let tick = self.game_time();
        if replaces(sensor.selector.as_ref(), &heard, tick) {
            sensor.selector = Some((heard, tick));
            self.set_sensor(pos, sensor);
        }
    }

    /// `VibrationSystem.Listener.isOccluded`: whether every one of the six lines from the
    /// middle of `from`'s block to the middle of `to`'s, each begun a hundred-thousandth
    /// of a block along one direction, meets a block that occludes vibrations.
    fn occluded(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        let start = block_centre(containing(from));
        let end = block_centre(containing(to));
        const NUDGE: f64 = 9.999_999_747_378_752e-6;
        clustine_data::Face::ALL.into_iter().all(|face| {
            let [dx, dy, dz] = crate::physics::piston::step(face);
            let nudged = [
                start[0] + f64::from(dx) * NUDGE,
                start[1] + f64::from(dy) * NUDGE,
                start[2] + f64::from(dz) * NUDGE,
            ];
            traverse(nudged, end, |pos| {
                self.block(pos)
                    .is_some_and(|state| in_tag(state, "minecraft:occludes_vibration_signals"))
            })
        })
    }

    /// `VibrationSystem.Ticker.tick` of the sensor at `pos`, in the phase of block
    /// entities: a candidate of an earlier tick is taken up and its particle shown,
    /// a vibration held travels, and one whose travel is done is received.
    pub(super) fn sensor_entity_tick(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(mut sensor) = self.sensor(pos) else {
            return;
        };
        let now = self.clock_now();
        if sensor.current.is_none()
            && let Some((heard, tick)) = sensor.selector
            // A candidate of a time after this one is of a region whose clock was
            // further on, or of a world kept before the clock: of the past all the same.
            && tick != now
        {
            sensor.current = Some(heard);
            sensor.travel = heard.distance.0.floor() as i32;
            output.events.push(RegionEvent::Vibration {
                from: heard.position(),
                to: pos,
                ticks: sensor.travel,
            });
            sensor.selector = None;
        }
        let Some(heard) = sensor.current else {
            return;
        };
        if sensor.reload_particle {
            // `tryReloadVibrationParticle`: from the point it had reached. A sensor
            // loaded without a vibration keeps the flag until it takes one up, and then
            // shows it twice, as the game's does.
            let whole = f64::from(heard.distance.0.floor() as i32);
            let done = 1.0 - f64::from(sensor.travel) / whole;
            let (from, to) = (heard.position(), listener_position(pos));
            let at: [f64; 3] =
                std::array::from_fn(|axis| from[axis] + done * (to[axis] - from[axis]));
            if at.iter().all(|value| value.is_finite()) {
                output.events.push(RegionEvent::Vibration {
                    from: at,
                    to: pos,
                    ticks: sensor.travel,
                });
            }
            sensor.reload_particle = false;
        }
        sensor.travel = (sensor.travel - 1).max(0);
        if sensor.travel > 0 {
            self.set_sensor(pos, sensor);
            return;
        }
        // `receiveVibration`: the sensor's chunk ticks, as its ticker runs.
        sensor.current = None;
        self.set_sensor(pos, sensor);
        let between = block_distance_sqr(containing(heard.position()), pos) as f64;
        self.sensor_received(pos, &heard, between.sqrt() as f32, output);
    }

    /// Who the player `id` is as the source of a vibration, if they are the region's.
    pub(crate) fn player_source(&self, id: PlayerId) -> Option<Source> {
        let player = self.players.get(&id)?;
        Some(Source {
            entity: player.entity_id,
            careful: player.keys.sneak,
            dampens: false,
        })
    }

    /// What `Entity.move(PLAYER, delta)` and `doCheckFallDamage` post of a move of the
    /// player `id` that their client reported, `delta` from where they were, on the
    /// ground before it as `was_on_ground` says: a step each time their walk passes the
    /// next, and a landing at the end of a fall.
    pub(crate) fn player_moved(&mut self, id: PlayerId, delta: [f64; 3], was_on_ground: bool) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let feet = [
            player.pose.position.x,
            player.pose.position.y,
            player.pose.position.z,
        ];
        let flying = player.flying;
        let landed = player.pose.on_ground;
        let source = Source {
            entity: player.entity_id,
            careful: player.keys.sneak,
            dampens: false,
        };
        let mut steps = player.steps;
        // `Entity.move`: a vertical move is not on the ground unless the region's
        // collision stopped it, which it does not run; a level one keeps the ground of
        // the move before.
        let on_ground = delta[1] == 0.0 && was_on_ground;
        // `Player.getMovementEmission`: nothing while flying, or sneaking on the ground.
        let emits = !(flying || (on_ground && source.careful));
        let in_water = self.block(containing(feet)).is_some_and(has_water);
        let legacy = containing([feet[0], feet[1] - 0.2, feet[2]]);
        let legacy_state = self.block(legacy).unwrap_or(blocks::AIR);
        if emits {
            // `applyMovementEmissionAndPlaySound`.
            let on = containing([feet[0], feet[1] - 1.0e-5, feet[2]]);
            let on_state = self.block(on).unwrap_or(blocks::AIR);
            let length = distance(delta, [0.0; 3]);
            let horizontal = (delta[0] * delta[0] + delta[2] * delta[2]).sqrt();
            let walked = if climbable(on_state) {
                length
            } else {
                horizontal
            };
            steps.moved += (walked * 0.600_000_023_841_857_9) as f32;
            if steps.moved > steps.next && !on_state.props().is_air() {
                let same = on == legacy;
                let crouching = source.careful && !flying;
                let step = Stepping {
                    on_ground,
                    crouching,
                    level: delta[1] == 0.0,
                    feet,
                    source,
                };
                let mut stepped = self.step_from_block(legacy_state, same, &step);
                if !same {
                    stepped |= self.step_from_block(on_state, true, &step);
                }
                if stepped || in_water {
                    steps.next = (steps.moved as i32 + 1) as f32;
                }
                if !stepped && in_water {
                    self.vibrate(Vibration::SWIM, feet, Context::of(Some(source), None));
                }
            }
        }
        // `doCheckFallDamage` with the client's ground, then `checkFallDamage`.
        if in_water {
            steps.fall = 0.0;
        } else if delta[1] < 0.0 {
            steps.fall -= f64::from(delta[1] as f32);
        }
        if landed {
            if steps.fall > 0.0 {
                let context = Context::of(Some(source), Some(legacy_state));
                self.vibrate(Vibration::HIT_GROUND, feet, context);
            }
            steps.fall = 0.0;
        }
        // Rising, or flying (`Player.aiStep`, in the player's tick after the move), ends
        // a fall.
        if delta[1] > 0.0 || flying {
            steps.fall = 0.0;
        }
        if let Some(player) = self.players.get_mut(&id) {
            player.steps = steps;
        }
    }

    /// `Entity.vibrationAndSoundEffectsFromBlock`: a step on `state` where the entity
    /// is on the ground, climbing, crouching without moving up or down, or on rails, and
    /// not swimming; posted where `posts`.
    fn step_from_block(&mut self, state: BlockState, posts: bool, step: &Stepping) -> bool {
        if state.props().is_air() {
            return false;
        }
        if !(step.on_ground || climbable(state) || (step.crouching && step.level)) {
            return false;
        }
        if posts {
            let context = Context::of(Some(step.source), Some(state));
            self.vibrate(Vibration::STEP, step.feet, context);
        }
        true
    }

    /// `Entity.checkFallDamage`'s post of an entity other than a player that landed at
    /// the end of a fall (`fell_on`, by the block under it, `on`, or the block it stood
    /// on, `supporting`), where its feet are.
    pub(crate) fn entity_hit_ground(
        &mut self,
        source: Source,
        feet: [f64; 3],
        on: BlockPos,
        supporting: Option<BlockPos>,
    ) {
        let state = supporting.or(Some(on)).and_then(|pos| self.block(pos));
        self.vibrate(
            Vibration::HIT_GROUND,
            feet,
            Context::of(Some(source), state),
        );
    }

    /// `Block.stepOn` of the block at `pos` that the entity `source` stood on in its
    /// tick, its feet at `feet`: redstone ore lights unless it steps carefully
    /// (`RedStoneOreBlock.stepOn`), and a sculk sensor hears it either way.
    pub(crate) fn stood_on(
        &mut self,
        pos: BlockPos,
        source: Source,
        feet: [f64; 3],
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::RedStoneOreBlock) {
            if !source.careful {
                self.light_redstone_ore(pos, state, output);
            }
        } else if sculk::is_sensor(state) {
            self.sensor_stepped_on(pos, Some(source.entity), feet);
        }
    }
}

/// What a step of a move asks of the entity.
struct Stepping {
    on_ground: bool,
    crouching: bool,
    /// The move went neither up nor down.
    level: bool,
    feet: [f64; 3],
    source: Source,
}

/// Whether `item` is wool of any kind, which an item entity of it dampens (for the tests'
/// sake, beside [`Source::item`]).
#[cfg(test)]
pub(crate) fn dampens(item: i32) -> bool {
    Source::item(EntityId(0), item).dampens
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::items;

    fn tag_ids(name: &str) -> Vec<i32> {
        let mut ids: Vec<i32> = TAGS
            .iter()
            .find(|tags| tags.registry == "minecraft:game_event")
            .and_then(|tags| tags.tags.iter().find(|tag| tag.name == name))
            .map(|tag| tag.entries.to_vec())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// The ids of the names agree with the game's tags: what sneaking hides is a
    /// landing, a step, a swim, a shot and an item's use, and the events sensors hear
    /// (`#vibrations`) are the ones with a frequency.
    #[test]
    fn the_events_named_are_the_ids_the_games_tags_list() {
        let named = |name: &str| i32::from(Vibration::named(name).expect("an event").0);
        let mut hidden: Vec<i32> = [
            "minecraft:hit_ground",
            "minecraft:projectile_shoot",
            "minecraft:step",
            "minecraft:swim",
            "minecraft:item_interact_start",
            "minecraft:item_interact_finish",
        ]
        .map(named)
        .to_vec();
        hidden.sort_unstable();
        assert_eq!(tag_ids("minecraft:ignore_vibrations_sneaking"), hidden);
        let heard: Vec<i32> = (0..61u8)
            .filter(|id| Vibration(*id).frequency() > 0)
            .map(i32::from)
            .collect();
        assert_eq!(tag_ids("minecraft:vibrations"), heard);
        // The brief counts 57; the game's tag has 56.
        assert_eq!(heard.len(), 56);
        assert_eq!(
            Vibration::resonance(15).map(Vibration::name),
            Some("minecraft:resonate_15")
        );
        assert_eq!(Vibration::STEP.frequency(), 1);
        assert_eq!(Vibration::EXPLODE.frequency(), 15);
        assert_eq!(Vibration::SHRIEK.radius(), 32);
    }

    /// An item of wool dampens what it does; one of stone does not.
    #[test]
    fn a_wool_item_dampens_its_vibrations() {
        assert!(dampens(items::WHITE_WOOL));
        assert!(dampens(items::RED_CARPET));
        assert!(!dampens(items::STONE));
    }
}
