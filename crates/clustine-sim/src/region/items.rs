//! The items' part of the entities' phase (`docs/groundwork/n4-plan.md`, sections 2
//! and 7; the brief's sections 3 and 5): each item's tick through the physics
//! (`crate::physics::item`), its merging with the items around it, what the tick
//! commits and reports, and the players' pick-up after the block entities.
//!
//! A tick commits an item's motion only together with a change of its position, its
//! being on the ground, its stack or health, or its burning; its counters (age, pick-up
//! delay, the fire's countdown) never make a commit, and its chunk is named among those
//! whose entities age (`TickOutput::entities_ageing`), which the worker saves at its
//! checkpoints. An item that moves into another chunk of the region is `Gone` in the
//! one and `Whole` in the other. The physics keep it in the chunks the region ticks.
//!
//! What the game keeps of an item in memory only and not in its record (the block it
//! stands on, the moves of its last tick, a cobweb's hold) the region keeps by key,
//! no part of the state; where it has none (after a take-in, a restore, a merge or a
//! split) it works them out from the record: the block under a resting item as a move
//! would find it, the last tick's moves as the one a resting item makes, and no hold.
//! Named differences: a cobweb's hold on its next move is lost at a restore (the game
//! loses it at a load too, when the hold is mid-tick), and the game's `firstTick`,
//! which keeps lava from counting for one tick after a load, is the item's very first
//! tick here, so that a region and its restore go on alike.

use clustine_data::EntityTypeInfo;
use clustine_world::{
    BlockPos, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, ItemStack,
    MAX_CHUNK_ENTITIES, PlayerId,
};

use super::Region;
use super::random_ticks::Random;
use crate::api::{PlayerEvent, RegionEvent, TickOutput, Tracked};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Movement};
use crate::physics::item::{self, Draws, Item, NEVER_PICKED_UP, SOUND_SOURCE_NEUTRAL};
use crate::physics::{self, World};

/// What the region keeps of an item between ticks beyond its record.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Memory {
    supporting: Option<BlockPos>,
    on_ground_no_blocks: bool,
    last_movements: Vec<Movement>,
    stuck_speed: [f64; 3],
}

/// The region as the physics of one entity read it: its loaded and copied chunks, and
/// the chunks it ticks.
pub(crate) struct View<'a> {
    pub(crate) region: &'a Region,
    /// The chunk the entity is in.
    pub(crate) home: ChunkPos,
}

impl World for View<'_> {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        self.region.block(pos)
    }

    fn has_chunk(&self, chunk: ChunkPos) -> bool {
        self.region.readable(chunk)
    }
}

impl item::Region for View<'_> {
    fn may_hold(&self, chunk: ChunkPos) -> bool {
        self.region.ticks_chunk(chunk)
            && (chunk == self.home
                || self
                    .region
                    .chunks
                    .get(&chunk)
                    .is_some_and(|loaded| loaded.entities().len() < MAX_CHUNK_ENTITIES))
    }

    fn min_y(&self) -> i32 {
        self.region
            .chunks
            .get(&self.home)
            .map_or(-64, |chunk| chunk.min_y())
    }
}

/// The entity's own stream in this tick, on from what it has drawn already.
struct Stream {
    random: Random,
    used: u64,
}

impl Draws for Stream {
    fn next_float(&mut self) -> f32 {
        self.used += 1;
        self.random.next_float()
    }
}

/// Whether an entity burns, as its record says.
fn burning(entity: &Entity) -> bool {
    entity.fire_ticks > 0
}

/// The stack and health of an item's record, which a change of commits.
fn item_parts(data: &EntityData) -> Option<(ItemStack, i16)> {
    match data {
        EntityData::Item { stack, health, .. } => Some((*stack, *health)),
        EntityData::FallingBlock { .. } => None,
    }
}

impl Region {
    /// The record of the entity `key` in the loaded chunk `position`.
    fn entity_record(&self, key: EntityId, position: ChunkPos) -> Option<&Entity> {
        let entities = self.chunks.get(&position)?.entities();
        let at = entities
            .binary_search_by_key(&key, |entity| entity.key)
            .ok()?;
        Some(&entities[at])
    }

    /// The item of `entity` as its tick starts.
    fn item_of(&self, entity: &Entity, position: ChunkPos) -> Option<Item> {
        let EntityData::Item {
            stack,
            pick_up_delay,
            health,
            ..
        } = entity.data
        else {
            return None;
        };
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let memory = self.entity_memory.get(&entity.key);
        let mut body = Body {
            position: entity.position,
            motion: entity.motion,
            on_ground: entity.on_ground,
            fall_distance: entity.fall_distance,
            width: info.width,
            height: info.height,
            old_position: entity.position,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            supporting: memory.and_then(|memory| memory.supporting),
            on_ground_no_blocks: memory.is_some_and(|memory| memory.on_ground_no_blocks),
            stuck_speed: memory.map_or([0.0; 3], |memory| memory.stuck_speed),
            movements: Vec::new(),
            in_water: false,
            sync_position: false,
            falling_block: false,
            item: true,
        };
        if memory.is_none() && body.on_ground {
            // As a move would have found it.
            body.check_supporting_block(
                &View {
                    region: self,
                    home: position,
                },
                None,
            );
        }
        let last_movements = memory.map_or_else(
            || {
                vec![Movement {
                    from: entity.position,
                    to: entity.position,
                    asked: None,
                }]
            },
            |memory| memory.last_movements.clone(),
        );
        Some(Item {
            body,
            stack,
            health,
            pick_up_delay,
            age: entity.age,
            fire_ticks: entity.fire_ticks,
            fire_immune: item::resists_fire(stack),
            first_tick: memory.is_none() && entity.age == 0,
            tick_count: self.tick as i32,
            game_id: entity.key.game_id(),
            last_movements,
            no_physics: false,
            removed: false,
            needs_sync: false,
            sync_velocity: false,
            on_fire_flag: burning(entity),
            fluids: Default::default(),
            start_motion: entity.motion,
            ore: Vec::new(),
            sounds: Vec::new(),
        })
    }

    /// The tick of the item `entity`, in the loaded chunk `position`.
    pub(crate) fn tick_item(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some(mut item) = self.item_of(&entity, position) else {
            return;
        };
        let key = entity.key;
        let drawn = self.entity_draws.get(&key).copied().unwrap_or(0);
        let mut random = Random::of_entity(self.config.seed, key, self.tick);
        for _ in 0..drawn {
            random.next_u64();
        }
        let mut stream = Stream { random, used: 0 };
        let alive = item.tick_before_merge(
            &View {
                region: self,
                home: position,
            },
            &mut stream,
        );
        if alive && item.merges_now() {
            self.merge_with_neighbours(&mut item, &entity, output);
        }
        item.tick_after_merge(&View {
            region: self,
            home: position,
        });
        if stream.used > 0 {
            *self.entity_draws.entry(key).or_default() += stream.used;
        }
        for ore in std::mem::take(&mut item.ore) {
            if let Some(state) = self.own_block(ore) {
                self.light_redstone_ore(ore, state, output);
            }
        }
        for sound in std::mem::take(&mut item.sounds) {
            output.events.push(RegionEvent::Sound {
                sound: sound.sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(
                    sound.position[0],
                    sound.position[1],
                    sound.position[2],
                ),
                volume: sound.volume,
                pitch: sound.pitch,
                except: None,
            });
        }
        self.write_item(&entity, item, position, output);
    }

    /// Removes the entity `key` from the chunk `position`, as the commit has it, and
    /// tells the edges it is gone.
    fn remove_entity(
        &mut self,
        key: EntityId,
        hops: u32,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let change = EntityChange::Gone(key);
        if let Some(chunk) = self.chunks.get_mut(&position) {
            chunk.apply_entity(&change);
        }
        output.entity_changes.push((position, change));
        self.entity_chunks.remove(&key);
        self.entity_memory.remove(&key);
        output.events.push(RegionEvent::EntityGone {
            entity: key,
            chunk: position,
            hops,
        });
    }

    /// What the item's tick leaves: its record, its commit, the edges' report.
    fn write_item(
        &mut self,
        before: &Entity,
        item: Item,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        if item.removed {
            self.remove_entity(key, before.hops, position, output);
            return;
        }
        // A merge may have changed the record since the tick began.
        let current = self.entity_record(key, position).cloned();
        let Some(current) = current else {
            return;
        };
        let thrower = match &current.data {
            EntityData::Item { thrower, .. } => *thrower,
            EntityData::FallingBlock { .. } => None,
        };
        let after = Entity {
            position: item.body.position,
            motion: item.body.motion,
            on_ground: item.body.on_ground,
            fall_distance: item.body.fall_distance,
            fire_ticks: item.fire_ticks,
            age: item.age,
            data: EntityData::Item {
                stack: item.stack,
                pick_up_delay: item.pick_up_delay,
                thrower,
                health: item.health,
            },
            ..current.clone()
        };
        self.entity_memory.insert(
            key,
            Memory {
                supporting: item.body.supporting,
                on_ground_no_blocks: item.body.on_ground_no_blocks,
                last_movements: item.last_movements.clone(),
                stuck_speed: item.body.stuck_speed,
            },
        );
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let moved = bits(after.position) != bits(before.position);
        let stack_changed = item_parts(&after.data).map(|(stack, _)| stack)
            != item_parts(&before.data).map(|(stack, _)| stack);
        let committed = moved
            || after.on_ground != before.on_ground
            || item_parts(&after.data) != item_parts(&before.data)
            || burning(&after) != burning(before);
        let to = after.chunk();
        if to != position {
            let gone = EntityChange::Gone(key);
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&gone);
            }
            output.entity_changes.push((position, gone));
            let whole = EntityChange::Whole(after.clone());
            if let Some(chunk) = self.chunks.get_mut(&to) {
                chunk.apply_entity(&whole);
            }
            output.entity_changes.push((to, whole));
            self.entity_chunks.insert(key, to);
        } else {
            if committed {
                let mut fields = Vec::new();
                if moved {
                    fields.push(EntityField::Position(after.position));
                }
                if bits(after.motion) != bits(current.motion) {
                    fields.push(EntityField::Motion(after.motion));
                }
                if after.on_ground != current.on_ground {
                    fields.push(EntityField::OnGround(after.on_ground));
                }
                if after.fall_distance.to_bits() != current.fall_distance.to_bits() {
                    fields.push(EntityField::FallDistance(after.fall_distance));
                }
                if burning(&after) != burning(before) {
                    fields.push(EntityField::FireTicks(after.fire_ticks));
                }
                if item_parts(&after.data) != item_parts(&before.data) {
                    fields.push(EntityField::Data(after.data.clone()));
                }
                // A merge's smaller age goes with its stack.
                if stack_changed {
                    fields.push(EntityField::Age(after.age));
                }
                output
                    .entity_changes
                    .push((position, EntityChange::Set(key, fields)));
            }
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let mut flags = 0;
        if item.body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if item.body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        if item.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        if item.body.sync_position {
            flags |= Tracked::SYNC_POSITION;
        }
        if item.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        if item.on_fire_flag {
            flags |= Tracked::ON_FIRE;
        }
        let synced = Tracked::NEEDS_SYNC | Tracked::SYNC_POSITION | Tracked::SYNC_VELOCITY;
        if committed || to != position || flags & synced != 0 {
            output.events.push(RegionEvent::EntityTracked(tracked(
                &after,
                flags,
                stack_changed,
            )));
        }
    }

    /// `ItemEntity.mergeWithNeighbours`: the items of the region's own chunks around
    /// it that can take it or that it can take, in the order of lookups (chunk, section,
    /// key); the larger stack takes the smaller.
    fn merge_with_neighbours(&mut self, item: &mut Item, entity: &Entity, output: &mut TickOutput) {
        let reach = item.body.bounds().inflate([0.5, 0.0, 0.5]);
        let candidates: Vec<(EntityId, ChunkPos)> = self
            .entities_in(&reach)
            .into_iter()
            .filter(|(other, _)| *other != entity.key)
            .filter(|(other, chunk)| {
                self.entity_record(*other, *chunk)
                    .is_some_and(record_is_mergable)
            })
            .collect();
        for (other, chunk) in candidates {
            let Some(record) = self.entity_record(other, chunk).cloned() else {
                continue;
            };
            if !record_is_mergable(&record) || !item.is_mergable() {
                continue;
            }
            self.try_to_merge(item, &record, chunk, output);
            if item.removed {
                break;
            }
        }
    }

    /// `ItemEntity.tryToMerge(other)`.
    fn try_to_merge(
        &mut self,
        item: &mut Item,
        other: &Entity,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Item {
            stack,
            pick_up_delay,
            thrower,
            health,
        } = other.data
        else {
            return;
        };
        // The game's owners (`target`) are never set yet: both are none.
        let same = stack.item == item.stack.item;
        if !same || stack.count + item.stack.count > item::max_stack_size(stack) {
            return;
        }
        let max = item::max_stack_size(stack).min(64);
        if stack.count < item.stack.count {
            // This one takes the other.
            let moved = (max - item.stack.count).min(stack.count);
            item.stack.count += moved;
            item.pick_up_delay = item.pick_up_delay.max(pick_up_delay);
            item.age = item.age.min(other.age);
            let left = stack.count - moved;
            if left <= 0 {
                self.remove_entity(other.key, other.hops, chunk, output);
            }
        } else {
            // The other takes this one.
            let moved = (max - stack.count).min(item.stack.count);
            let taken = Entity {
                age: other.age.min(item.age),
                data: EntityData::Item {
                    stack: ItemStack {
                        count: stack.count + moved,
                        ..stack
                    },
                    pick_up_delay: pick_up_delay.max(item.pick_up_delay),
                    thrower,
                    health,
                },
                ..other.clone()
            };
            let fields = vec![
                EntityField::Age(taken.age),
                EntityField::Data(taken.data.clone()),
            ];
            let change = EntityChange::Set(other.key, fields);
            if let Some(loaded) = self.chunks.get_mut(&chunk) {
                loaded.apply_entity(&change);
            }
            output.entity_changes.push((chunk, change));
            output
                .events
                .push(RegionEvent::EntityTracked(tracked(&taken, 0, true)));
            item.stack.count -= moved;
            if item.stack.count <= 0 {
                item.removed = true;
            }
        }
    }

    /// The entities of the region's own chunks whose boxes meet `bounds`, in the order
    /// of the game's lookups: by chunk, then by section, then by key.
    fn entities_in(&self, bounds: &Aabb) -> Vec<(EntityId, ChunkPos)> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let chunk = ChunkPos::new(x, z);
                let Some(loaded) = self.chunks.get(&chunk) else {
                    continue;
                };
                let mut here: Vec<(i32, EntityId)> = loaded
                    .entities()
                    .iter()
                    .filter(|entity| {
                        let info = EntityTypeInfo::of(i32::from(entity.kind));
                        Aabb::of_entity(entity.position, info.width, info.height)
                            .intersects(*bounds)
                    })
                    .map(|entity| (physics::floor(entity.position[1]) >> 4, entity.key))
                    .collect();
                here.sort();
                found.extend(here.into_iter().map(|(_, key)| (key, chunk)));
            }
        }
        found.sort_by_key(|(_, chunk)| *chunk);
        found
    }

    /// The players' pick-up (`Player.aiStep` → `ItemEntity.playerTouch`), after the
    /// block entities as the game's connections tick after its levels, in the order of
    /// the players: each takes the items of the region's chunks in their box grown by a
    /// block sideways and half a block up and down, whose delay is over. A creative
    /// player takes all of a stack, what does not fit being destroyed
    /// (`Inventory.add`); the client is told of each slot that changed, and everyone
    /// who sees the item of its being taken.
    pub(crate) fn pick_up_items(&mut self, output: &mut TickOutput) {
        let players: Vec<(PlayerId, EntityId, [f64; 3])> = self
            .players
            .iter()
            .map(|(id, player)| {
                let at = player.pose.position;
                (*id, player.entity_id, [at.x, at.y, at.z])
            })
            .collect();
        for (id, collector, feet) in players {
            let reach = Aabb::of_entity(feet, 0.6, 1.8).inflate([1.0, 0.5, 1.0]);
            for (key, chunk) in self.entities_in(&reach) {
                let Some(record) = self.entity_record(key, chunk).cloned() else {
                    continue;
                };
                let EntityData::Item {
                    stack,
                    pick_up_delay,
                    ..
                } = record.data
                else {
                    continue;
                };
                if pick_up_delay != 0 {
                    continue;
                }
                let Some(player) = self.players.get_mut(&id) else {
                    break;
                };
                let changed = add_to_inventory(
                    &mut player.inventory,
                    usize::from(player.selected_slot),
                    stack,
                );
                self.journal.players.insert(id);
                for slot in changed {
                    let slot_stack = player.inventory.0[slot];
                    let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
                    output.player_events.push((
                        id,
                        PlayerEvent::InventorySlot {
                            slot,
                            stack: slot_stack,
                        },
                    ));
                }
                output.events.push(RegionEvent::ItemTaken {
                    item: key,
                    collector,
                    count: stack.count,
                    chunk,
                });
                // A creative player takes the whole stack.
                self.remove_entity(key, record.hops, chunk, output);
            }
        }
    }
}

/// `isMergable` of an item's record.
fn record_is_mergable(record: &Entity) -> bool {
    match record.data {
        EntityData::Item {
            stack,
            pick_up_delay,
            ..
        } => {
            pick_up_delay != NEVER_PICKED_UP
                && record.age < item::LIFETIME
                && stack.count < item::max_stack_size(stack)
        }
        EntityData::FallingBlock { .. } => false,
    }
}

/// The report of `entity` to the edges.
fn tracked(entity: &Entity, flags: u8, with_data: bool) -> Tracked {
    Tracked {
        entity: entity.key,
        chunk: entity.chunk(),
        hops: entity.hops,
        position: entity.position,
        motion: entity.motion,
        yaw: entity.yaw,
        pitch: entity.pitch,
        on_ground: entity.on_ground,
        flags,
        data: with_data.then(|| entity.data.clone()),
    }
}

/// `Inventory.add(stack)` for a player with infinite materials: into the selected slot,
/// the off hand or the first of the main slots that has the same item and room, then
/// into the first empty main slot, as long as a pass puts any in; what does not fit is
/// destroyed. The slots changed, in the order changed.
pub(crate) fn add_to_inventory(
    inventory: &mut clustine_world::Inventory,
    selected: usize,
    stack: ItemStack,
) -> Vec<usize> {
    let max = item::max_stack_size(stack).min(99);
    let mut left = stack.count;
    let mut changed = Vec::new();
    loop {
        let before = left;
        // `getSlotWithRemainingSpace`, then `getFreeSlot`.
        let has_room = |slot: Option<ItemStack>| {
            slot.is_some_and(|held| held.item == stack.item && max > 1 && held.count < max)
        };
        let order = std::iter::once(selected)
            .chain(std::iter::once(clustine_world::OFF_HAND))
            .chain(0..36);
        let target = order
            .clone()
            .find(|slot| has_room(inventory.0[*slot]))
            .or_else(|| (0..36).find(|slot| inventory.0[*slot].is_none()));
        let Some(slot) = target else {
            break;
        };
        let held = inventory.0[slot].map_or(0, |held| held.count);
        let put = left.min(max - held);
        if put == 0 {
            break;
        }
        inventory.0[slot] = Some(ItemStack {
            item: stack.item,
            count: held + put,
        });
        if !changed.contains(&slot) {
            changed.push(slot);
        }
        left -= put;
        if left <= 0 || left >= before {
            break;
        }
    }
    changed
}
