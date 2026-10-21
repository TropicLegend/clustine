//! Dried ghasts (`DriedGhastBlock`; `docs/groundwork/n7-brief.md`, section 7.11.7): a
//! block that, under water, takes up water a stage at a time and at its fourth wakes as a
//! ghastling facing the way the block faced, and out of water dries a stage at a time.
//! Each stage waits 5000 ticks of a tick its random tick schedules where none waits
//! (`random_ticks/minerals.rs`).
//!
//! Named difference: placing it plays the block's own placing sound and not the one it
//! plays in water (`setPlacedBy`'s `place_in_water`).

use clustine_blocks::data::{flag, has_water, is_class, number, set};
use clustine_data::{BlockClass, BlockState, blocks, entity_types, sounds};
use clustine_world::BlockPos;

use super::Region;
use super::updates::{UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::types::happy_ghast;

/// `MAX_HYDRATION_LEVEL`: at this stage under water it wakes.
pub(crate) const MAX_HYDRATION: u8 = 3;

/// `Direction.toYRot()` of a horizontal facing: south 0, west 90, north 180, east 270.
pub(crate) fn facing_yaw(state: BlockState) -> f32 {
    match state.property("facing") {
        Some("west") => 90.0,
        Some("north") => 180.0,
        Some("east") => 270.0,
        _ => 0.0,
    }
}

impl Region {
    /// `DriedGhastBlock.tick`: under water a stage more (`tickWaterlogged`), and at the
    /// last a ghastling; out of it a stage less, with `BLOCK_CHANGE`. Whether the block
    /// was a dried ghast.
    pub(crate) fn dried_ghast_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::DriedGhastBlock) {
            return false;
        }
        let hydration = number(state, "hydration").unwrap_or(0);
        if flag(state, "waterlogged") {
            if hydration < MAX_HYDRATION {
                output.events.push(RegionEvent::Sound {
                    sound: sounds::BLOCK_DRIED_GHAST_TRANSITION,
                    source: SOUND_SOURCE_BLOCKS,
                    position: clustine_world::Vec3::new(
                        f64::from(pos.x) + 0.5,
                        f64::from(pos.y) + 0.5,
                        f64::from(pos.z) + 0.5,
                    ),
                    volume: 1.0,
                    pitch: 1.0,
                    except: None,
                });
                self.set_hydration(pos, state, hydration + 1, output);
            } else {
                self.spawn_ghastling(pos, state, output);
            }
        } else if hydration > 0 {
            self.set_hydration(pos, state, hydration - 1, output);
        }
        true
    }

    /// The stage `hydration` set (flags 2), with `BLOCK_CHANGE`.
    fn set_hydration(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        hydration: u8,
        output: &mut TickOutput,
    ) {
        let changed = set(state, "hydration", &hydration.to_string());
        self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        self.vibrate_at(
            Vibration::BLOCK_CHANGE,
            pos,
            Context::of(None, Some(changed)),
        );
    }

    /// `spawnGhastling`: the block removed (its water left), and a ghastling at its
    /// bottom's middle facing as the block faced, with its sound at it.
    fn spawn_ghastling(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let left = if has_water(state) {
            blocks::WATER
        } else {
            blocks::AIR
        };
        self.set_block_updated(pos, left, UPDATE_ALL, UPDATE_LIMIT, output);
        let yaw = facing_yaw(state);
        let mut baby = happy_ghast::ghastling();
        baby.body.head.head_yaw = yaw;
        baby.body.head.body_yaw = yaw;
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let kind = entity_types::HAPPY_GHAST as u16;
        if let Some(made) = self.new_mob(kind, baby, at, yaw, 0.0) {
            self.add_made_mob(made, output);
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_GHASTLING_SPAWN,
                source: SOUND_SOURCE_BLOCKS,
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
    }
}
