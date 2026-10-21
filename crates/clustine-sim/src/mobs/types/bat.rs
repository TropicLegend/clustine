//! Bats (`Bat` and `AmbientCreature`; `docs/groundwork/n7-brief.md`, section 6.11 and
//! finding W15): no goals and no path; a resting bat hangs under a solid block, its feet
//! put a tenth of a block above the block below every tick, and wakes when a player
//! comes within four blocks or the block above goes; a flying one pushes its own motion
//! towards a block it draws near it, and now and then hangs up again under a solid one.
//! It is not an animal: it does not breed, and it despawns far from players.
//!
//! What it flies to (`targetPosition`) is kept with it, where the game keeps it only in
//! memory, so that a bat crossing a line or a merge flies on as it was.
//!
//! Named differences: it is never silent (no mob is); it treads on plates and tripwire
//! as other mobs do, where the game's bat ignores them (`isIgnoringBlockTriggers`).

use clustine_data::sounds;
use clustine_world::{BlockPos, TypeState};
use serde::{Deserialize, Serialize};

use super::{Dimensions, MobType};
use crate::mobs::Mob;
use crate::mobs::ai::maths::{self, wrap_degrees};
use crate::mobs::ai::sensing::{self, Conditions};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::make_sound;
use crate::mobs::tick::{Local, MobTick};
use crate::physics;
use crate::region::Random;
use crate::shown::DataShown;

/// The version of a bat's type state.
const VERSION: u8 = 1;

/// The synced data's index of its flags (`DATA_ID_FLAGS`), of which bit 1 is resting.
const FLAGS_INDEX: u8 = 16;
const RESTING: u8 = 1;

/// `LevelEvent.SOUND_BAT_LIFTOFF`.
const LIFTOFF: i32 = 1025;

/// What a bat keeps: whether it rests, and the block it flies towards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BatState {
    pub(crate) resting: bool,
    pub(crate) target: Option<BlockPos>,
}

/// A bat's state from its type state; a record of nothing, or one this build cannot
/// read, as a bat that is awake and goes nowhere.
pub(crate) fn state_of(mob: &Mob) -> BatState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return BatState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

pub(crate) fn put_state(mob: &mut Mob, own: &BatState) {
    mob.state = if *own == BatState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// The bat.
pub(crate) struct Bat;

/// `setResting(false)` with its take-off's sound for those near.
fn wake(tick: &mut MobTick<'_>, own: &mut BatState, at: BlockPos) {
    own.resting = false;
    tick.out.local.push(Local::LevelEvent {
        kind: LIFTOFF,
        pos: at,
        data: 0,
    });
}

/// `signum(d)` as `Math.signum`: nothing for nothing.
fn signum(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

impl MobType for Bat {
    fn register_goals(&self, _: &mut Setup) {}

    /// The constructor: a new bat rests.
    fn construct(&self, mob: &mut Mob, _: &mut Random) {
        let mut own = state_of(mob);
        own.resting = true;
        put_state(mob, &own);
    }

    /// `Bat.customServerAiStep`: a resting bat turns its head now and then and wakes at a
    /// player within four blocks, or at once without a solid block above; a flying one
    /// draws a new place to go when it has none, it is reached, or one time in 30, and
    /// steers its motion towards it; one time in a hundred it hangs up under a solid
    /// block.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let mut own = state_of(&tick.mob);
        let position = tick.living.body.position;
        let at = physics::containing(position);
        let above = BlockPos::new(at.x, at.y + 1, at.z);
        if own.resting {
            // `isRedstoneConductor(level, pos)`, read of the block above.
            if tick.world.state(above).props().is_redstone_conductor() {
                if tick.draws.next_int(200) == 0 {
                    tick.mob.body.head.head_yaw = tick.draws.next_int(360) as f32;
                }
                if near_player(tick) {
                    wake(tick, &mut own, at);
                }
            } else {
                wake(tick, &mut own, at);
            }
        } else {
            if let Some(target) = own.target
                && (!tick.world.state(target).props().is_air() || target.y <= tick.world.min_y())
            {
                own.target = None;
            }
            let renew = match own.target {
                None => true,
                Some(target) => {
                    tick.draws.next_int(30) == 0 || {
                        let centre = [
                            f64::from(target.x) + 0.5,
                            f64::from(target.y) + 0.5,
                            f64::from(target.z) + 0.5,
                        ];
                        physics::distance_sqr(centre, position) < 4.0
                    }
                }
            };
            if renew {
                let dx = tick.draws.next_int(7) as i32 - tick.draws.next_int(7) as i32;
                let dy = f64::from(tick.draws.next_int(6)) - 2.0;
                let dz = tick.draws.next_int(7) as i32 - tick.draws.next_int(7) as i32;
                own.target = Some(physics::containing([
                    position[0] + f64::from(dx),
                    position[1] + dy,
                    position[2] + f64::from(dz),
                ]));
            }
            let target = own.target.unwrap_or(at);
            let dx = f64::from(target.x) + 0.5 - position[0];
            let dy = f64::from(target.y) + 0.1 - position[1];
            let dz = f64::from(target.z) + 0.5 - position[2];
            let v = tick.living.body.motion;
            let step = 0.100_000_001_490_116_12;
            let w = [
                v[0] + (signum(dx) * 0.5 - v[0]) * step,
                v[1] + (signum(dy) * 0.699_999_988_079_071 - v[1]) * step,
                v[2] + (signum(dz) * 0.5 - v[2]) * step,
            ];
            tick.living.body.motion = w;
            let yaw = (maths::atan2(w[2], w[0]) * 57.295_776_367_187_5) as f32 - 90.0;
            // `zza = 0.5f`, which the move control's wait sets back to nothing.
            tick.mob.body.inputs.zza = 0.5;
            tick.yaw += wrap_degrees(yaw - tick.yaw);
            if tick.draws.next_int(100) == 0
                && tick.world.state(above).props().is_redstone_conductor()
            {
                own.resting = true;
            }
        }
        put_state(&mut tick.mob, &own);
    }

    /// `Bat.tick` after the living tick: a resting bat is still, its feet a tenth of a
    /// block over the block it is in; a flying one's fall is slowed to six tenths.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let own = state_of(&tick.mob);
        let body = &mut tick.living.body;
        if own.resting {
            body.motion = [0.0; 3];
            let y = f64::from(physics::floor(body.position[1])) + 1.0 - f64::from(body.height);
            body.position[1] = y;
        } else {
            body.motion[1] *= 0.6;
        }
    }

    /// `Bat.hurtServer`: a hurt it is not spared wakes it.
    fn before_hurt(&self, mob: &mut Mob) {
        let mut own = state_of(mob);
        if own.resting {
            own.resting = false;
            put_state(mob, &own);
        }
    }

    fn pushable(&self) -> bool {
        false
    }

    fn dimensions(&self, _: &Mob) -> Option<Dimensions> {
        None
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BAT_AMBIENT)
    }

    /// `getAmbientSound()`: a resting bat is silent three times in four (a draw of its
    /// own after the ambient's chance).
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        let sound = if state_of(&tick.mob).resting && tick.draws.next_int(4) != 0 {
            None
        } else {
            Some(sounds::ENTITY_BAT_AMBIENT)
        };
        make_sound(tick, sound, self.sound_volume());
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_BAT_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_BAT_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.1
    }

    fn voice_pitch_factor(&self) -> f32 {
        0.95
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let flags = if state_of(mob).resting { RESTING } else { 0 };
        vec![(FLAGS_INDEX, DataShown::Byte(flags), flags == 0)]
    }
}

/// `level.getNearestPlayer(BAT_RESTING_TARGETING, bat)`: whether a player within four
/// blocks of its feet is seen (non-combat: a creative player too).
fn near_player(tick: &mut MobTick<'_>) -> bool {
    let config = crate::mobs::ai::Config::default();
    let mut ai = Ai {
        tick,
        config: &config,
        sensing: sensing::Sensing::default(),
        navigation_ticked: true,
    };
    let conditions = Conditions::non_combat().range(4.0);
    let feet = ai.position();
    sensing::nearest_player(&mut ai, &conditions, &|_| true, feet).is_some()
}
