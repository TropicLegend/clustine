//! The shulker box's lid (`docs/groundwork/n6-brief.md`, section 3.5; findings 8 and
//! 9), which moves on the server as on the client: its block event of the openers'
//! count starts it opening or closing (`ShulkerBoxBlockEntity.triggerEvent`), and its
//! ticker moves it a tenth a tick, tells its neighbours as it leaves and reaches either
//! end, and while it opens pushes what is in its way out of it
//! (`updateAnimation`, `moveCollidedEntities`).
//!
//! The lid lives in memory, as the game's does: a box loaded is closed in the game,
//! which has had every menu on it closed; in Clustine a menu outlives a restore, so a
//! box whose saved openers' count is not 0 is loaded open, and a box loaded in a tick
//! has its count corrected from its players' menus, as it has no recheck (the plan's
//! section 7 item 5).
//!
//! At a line (section 10 item 4): the room for the lid is read in the copy where the box
//! faces across, and what the lid pushes is what the region ticks, never the other
//! region's entities. Named differences: the lid's push moves items and falling blocks
//! (and pops what hangs on a block), as a piston's does, and leaves the other entities
//! where they are; what the blocks along the push do to what it moves is not done; the
//! box's collision and support shapes stay a full block's while the lid is open.

use clustine_blocks::data::{facing, is_class, relative};
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityId};

use super::Region;
use super::containers::SET_OPEN_COUNT;
use super::items::View;
use super::updates::{UPDATE_ALL, UPDATE_LIMIT, UPDATE_SHAPE_ORDER};
use crate::api::{MenuKind, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};
use crate::physics::entities::Mover;
use crate::state::MenuOf;

/// How far the lid moves in a tick, of the way from shut to open.
const STEP: f32 = 0.1;

/// `ShulkerBoxBlockEntity.AnimationStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Closed,
    Opening,
    Opened,
    Closing,
}

/// The lid of a shulker box: how it moves, and how far open it is now and was a tick
/// before (`progress`, `progressOld`), from 0 to 1, in `float` as the game keeps them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Lid {
    pub(crate) status: Status,
    pub(crate) progress: f32,
    pub(crate) progress_old: f32,
}

// The progresses are never NaN: they start at 0 or 1 and move by a tenth.
impl Eq for Lid {}

impl Lid {
    /// The lid of a box loaded, or placed: open where players have it open, else shut.
    pub(crate) fn loaded(open: bool) -> Lid {
        let (status, progress) = if open {
            (Status::Opened, 1.0)
        } else {
            (Status::Closed, 0.0)
        };
        Lid {
            status,
            progress,
            progress_old: progress,
        }
    }
}

/// `Shulker.getProgressDeltaAabb(1, facing, from, to, Vec3.atBottomCenterOf(pos))`: the
/// slab beyond the face of the box at `pos` towards `facing`, from `from` to `to` of a
/// block out from it.
pub(crate) fn progress_delta_box(facing: Face, from: f32, to: f32, pos: BlockPos) -> Aabb {
    let step = crate::physics::piston::step(facing).map(f64::from);
    let out = f64::from(from.max(to));
    let within = f64::from(from.min(to));
    let mut min = [-0.5, 0.0, -0.5];
    let mut max = [0.5, 1.0, 0.5];
    for axis in 0..3 {
        // `expandTowards(step * out)`.
        let grow = step[axis] * out;
        if grow < 0.0 {
            min[axis] += grow;
        } else if grow > 0.0 {
            max[axis] += grow;
        }
        // `contract(-step * (1 + within))`.
        let shrink = -step[axis] * (1.0 + within);
        if shrink < 0.0 {
            min[axis] -= shrink;
        } else if shrink > 0.0 {
            max[axis] -= shrink;
        }
    }
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    Aabb::new(min, max).moved(at)
}

impl Region {
    /// `ShulkerBoxBlock.canOpen`: the box at `pos`, of a chunk loaded or copied, is
    /// already moving its lid, or is shut with room for its lid: the half block beyond
    /// its face meets no block's shape and no solid entity (`Level.noCollision`).
    pub(crate) fn shulker_box_may_open(&self, pos: BlockPos) -> bool {
        let Some(state) = self.block(pos) else {
            return false;
        };
        if !is_class(state, BlockClass::ShulkerBoxBlock) {
            return false;
        }
        if self
            .shulker_lid(pos)
            .is_some_and(|lid| lid.status != Status::Closed)
        {
            return true;
        }
        let lid_facing = facing(state).unwrap_or(Face::Up);
        let room = progress_delta_box(lid_facing, 0.0, 0.5, pos).deflate(1.0e-6);
        let view = View {
            region: self,
            home: pos.chunk(),
        };
        // `CollisionContext.empty()`, and no entity of its own.
        let context = Context {
            stands_on_lava: false,
            bottom: -f64::MAX,
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        let mover = Mover {
            key: EntityId(u64::MAX),
            root: EntityId(u64::MAX),
            like_a_vehicle: false,
            happy_ghast: false,
        };
        no_collision(&view, context, &mover, &room)
    }

    /// `ShulkerBoxBlockEntity.startOpen`: the count goes up and is said by a block
    /// event every time; the first opener's sound plays at the box's centre.
    pub(super) fn shulker_box_start_open(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let now = self.openers(pos).saturating_add(1);
        self.set_openers(pos, now, output);
        self.block_event(
            pos,
            state,
            SET_OPEN_COUNT,
            i32::try_from(now).unwrap_or(i32::MAX),
        );
        if now == 1 {
            self.shulker_box_sound(pos, true, output);
        }
    }

    /// `ShulkerBoxBlockEntity.stopOpen`: the count goes down and is said; the last
    /// opener's going plays the closing sound. A count at 0 stays there.
    pub(super) fn shulker_box_stop_open(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let before = self.openers(pos);
        if before == 0 {
            return;
        }
        let now = before - 1;
        self.set_openers(pos, now, output);
        self.block_event(
            pos,
            state,
            SET_OPEN_COUNT,
            i32::try_from(now).unwrap_or(i32::MAX),
        );
        if now == 0 {
            self.shulker_box_sound(pos, false, output);
        }
    }

    fn shulker_box_sound(&mut self, pos: BlockPos, open: bool, output: &mut TickOutput) {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let sound = if open {
            sounds::BLOCK_SHULKER_BOX_OPEN
        } else {
            sounds::BLOCK_SHULKER_BOX_CLOSE
        };
        // With `container_open` or `container_close` of the first opener or the last
        // (`vibrations.rs`); the user is not passed down, and is none.
        self.container_vibrates(pos, open);
        self.container_sound(centre, sound, output);
    }

    /// `ShulkerBoxBlockEntity.triggerEvent` of the box at `pos`: the count's event
    /// starts the lid closing at 0 and opening at 1, and leaves it as it is at more.
    /// Whether the event ran.
    pub(super) fn shulker_box_event(&mut self, pos: BlockPos, kind: i32, count: i32) -> bool {
        if kind != SET_OPEN_COUNT {
            return false;
        }
        let Some(lid) = self.shulker_lid_mut(pos) else {
            return false;
        };
        if count == 0 {
            lid.status = Status::Closing;
        }
        if count == 1 {
            lid.status = Status::Opening;
        }
        true
    }

    /// `ShulkerBoxBlockEntity.tick` of the box at `pos` (`updateAnimation`).
    pub(super) fn shulker_box_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(mut lid) = self.shulker_lid(pos) else {
            return;
        };
        lid.progress_old = lid.progress;
        match lid.status {
            Status::Closed => {
                lid.progress = 0.0;
                self.put_lid(pos, lid);
            }
            Status::Opened => {
                lid.progress = 1.0;
                self.put_lid(pos, lid);
            }
            Status::Opening => {
                lid.progress += STEP;
                self.put_lid(pos, lid);
                if lid.progress_old == 0.0 {
                    self.shulker_box_neighbour_updates(pos, state, output);
                }
                if lid.progress >= 1.0 {
                    lid.status = Status::Opened;
                    lid.progress = 1.0;
                    self.put_lid(pos, lid);
                    self.shulker_box_neighbour_updates(pos, state, output);
                }
                // The updates may have taken the box away.
                if self.shulker_lid(pos).is_some() {
                    self.shulker_box_push(pos, state, lid, output);
                }
            }
            Status::Closing => {
                lid.progress -= STEP;
                self.put_lid(pos, lid);
                if lid.progress_old == 1.0 {
                    self.shulker_box_neighbour_updates(pos, state, output);
                }
                if lid.progress <= 0.0 {
                    lid.status = Status::Closed;
                    lid.progress = 0.0;
                    self.put_lid(pos, lid);
                    self.shulker_box_neighbour_updates(pos, state, output);
                }
            }
        }
    }

    /// Keeps `lid` as the lid of the box at `pos`, where it still has its block entity.
    fn put_lid(&mut self, pos: BlockPos, lid: Lid) {
        if let Some(kept) = self.shulker_lid_mut(pos) {
            *kept = lid;
        }
    }

    /// `moveCollidedEntities`: what the region ticks in the slab the lid swept this
    /// tick is moved the slab's depth out, and a hundredth more.
    fn shulker_box_push(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        lid: Lid,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::ShulkerBoxBlock) {
            return;
        }
        let towards = facing(state).unwrap_or(Face::Up);
        let swept = progress_delta_box(towards, lid.progress_old, lid.progress, pos);
        let size = swept.size();
        let step = crate::physics::piston::step(towards).map(f64::from);
        let by = [
            (size[0] + 0.01) * step[0],
            (size[1] + 0.01) * step[1],
            (size[2] + 0.01) * step[2],
        ];
        self.push_by_shulker_box(&swept, by, output);
    }

    /// `doNeighborUpdates`: the box's neighbours take their shapes again
    /// (`updateNeighbourShapes` with every update) and are told of it
    /// (`updateNeighborsAt`).
    fn shulker_box_neighbour_updates(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        for direction in UPDATE_SHAPE_ORDER {
            self.shape_changed_at(
                relative(pos, direction),
                direction.opposite(),
                state,
                UPDATE_ALL,
                UPDATE_LIMIT,
                output,
            );
        }
        self.update_neighbours_at(pos, state, output);
    }

    /// The shulker boxes of the chunk at `position`, just loaded, whose saved openers'
    /// count is not what the region's players' menus show: the count corrected, and the
    /// lid moved and the sound played as the game's `startOpen` and `stopOpen` would.
    pub(crate) fn recount_shulker_boxes(&mut self, position: ChunkPos, output: &mut TickOutput) {
        for pos in self.shulker_boxes_in(position) {
            let Some(state) = self.own_block(pos) else {
                continue;
            };
            let found = self
                .players
                .values()
                .filter(|player| {
                    player.menu.as_ref().is_some_and(|menu| {
                        menu.kind == MenuKind::ShulkerBox
                            && matches!(menu.of, MenuOf::Block { at, .. } if at == pos)
                    })
                })
                .count();
            let found = u32::try_from(found).unwrap_or(u32::MAX);
            let saved = self.openers(pos);
            if found == saved {
                continue;
            }
            self.set_openers(pos, found, output);
            self.block_event(
                pos,
                state,
                SET_OPEN_COUNT,
                i32::try_from(found).unwrap_or(i32::MAX),
            );
            if saved == 0 {
                self.shulker_box_sound(pos, true, output);
            } else if found == 0 {
                self.shulker_box_sound(pos, false, output);
            }
        }
    }
}
