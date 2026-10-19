//! Armour stands (`ArmorStand`, `docs/groundwork/n5-brief.md`, section 5;
//! `docs/groundwork/n5-plan.md`, decision 4 and section 8 item 7): placed by their item,
//! a reduced living body in their tick (`crate::living`), dressed and undressed by a
//! player's click by the height it lands at, broken at once by a creative player's hit
//! and by two hits within five ticks of anyone else's, and pushing the rideable carts
//! they stand in.
//!
//! What a stand wears is in its record by `EquipmentSlot` ordinal. Clients are told of
//! it by the edge, which sends `SetEquipment` at pairing and whenever a report's data
//! shows it changed (the game's `detectEquipmentUpdates`), so the region only reports
//! the data.
//!
//! A stand's counters follow N2's rule: its age, its air under water and its freezing
//! in powder snow change its record in place, its chunk ageing, and are reported to the
//! edge where a client shows them; only what moves it, what it wears and its health are
//! committed. At rest a stand's motion holds at the -0.0784 its gravity and the ground
//! leave it every tick, so from its third tick on it commits nothing (its first sets its
//! motion, its second its being on the ground).
//!
//! Its two hits within five ticks are counted by its age, which counts the ticks its
//! region ran it (plan, decision 6: the game counts the world's game time; a stand that
//! crosses a line between the two hits needs two more).
//!
//! Named differences: across a line (another region's player) only a creative player's
//! putting an item on is done, as an effect; a take or a swap gives the player an item
//! and does nothing there (plan, section 8 item 7). The game's swing of the player's own
//! arm, an off hand's swing, the first hit's wobble and sound (entity event 32), and the
//! breaking particles are not sent: the repo sends no entity events or particles yet.
//! A name tag names nothing (Clustine keeps no names), and a stand's `Invisible` field
//! is its flag, set only from its record. `noPhysics` is taken from being a marker at
//! each tick, where the game sets it only at a load. Whether it was in powder snow the
//! tick before is not kept, so a stand in leather boots does not climb out of it.

use clustine_blocks::data::item_in_tag;
use clustine_data::{EntityTypeInfo, ItemComponents, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, EntityUse, Float,
    ItemStack, OFF_HAND, PlayerId,
};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{Memory, View, tracked};
use super::random_ticks::Random;
use super::riding::{Relocated, wrap_degrees};
use crate::api::{Face, PlayerEvent, RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput, Tracked};
use crate::living::{End, Kind, Living, MAX_AIR, slots};
use crate::physics;
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Movement};
use crate::physics::collide::{self, Context};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::placement::Click;

/// `DATA_CLIENT_FLAGS`' bits, and the invisibility the record keeps beside them.
pub(crate) const SMALL: u8 = 1;
pub(crate) const SHOW_ARMS: u8 = 4;
pub(crate) const MARKER: u8 = 16;
pub(crate) const INVISIBLE: u8 = 32;

/// The parts' rotations of a stand that has not been posed (head, body, left arm, right
/// arm, left leg, right leg; the static block of `ArmorStand`).
pub(crate) const DEFAULT_POSE: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
    [-10.0, 0.0, -10.0],
    [-15.0, 0.0, 10.0],
    [-1.0, 0.0, -1.0],
    [1.0, 0.0, 1.0],
];

/// The health of a new stand: the `max_health` attribute's default. **Inferred**:
/// `Attributes` is not in the dump; 20 is the game's.
pub(super) const MAX_HEALTH: f32 = 20.0;

/// The ticks within which a second hit breaks a stand (`now - lastHit > 5` is a first
/// hit).
const DOUBLE_HIT_WINDOW: u32 = 5;

/// `ArmorStand.pushEntities`' reach: a cart nearer than this, squared, is pushed.
const PUSH_REACH: f64 = 0.2;

/// The size of a stand of `flags`, its width, height and eyes: a marker none, a small
/// one half the type's size with its own eyes, any other the type's
/// (`getDimensionsMarker`).
pub(crate) fn stand_size(kind: u16, flags: u8) -> (f32, f32, f32) {
    let info = EntityTypeInfo::of(i32::from(kind));
    if flags & MARKER != 0 {
        (0.0, 0.0, 0.0)
    } else if flags & SMALL != 0 {
        (info.width * 0.5, info.height * 0.5, 0.987_5)
    } else {
        (info.width, info.height, info.eye_height)
    }
}

/// The size of `entity`'s box where its kind's flags make it other than its type's.
pub(crate) fn own_size(entity: &Entity) -> Option<(f32, f32)> {
    match entity.data {
        EntityData::ArmorStand { flags, .. } => {
            let (width, height, _) = stand_size(entity.kind, flags);
            Some((width, height))
        }
        _ => None,
    }
}

/// What a stand wears in `slot`, if anything.
pub(super) fn worn(equipment: &[(u8, ItemStack)], slot: u8) -> Option<ItemStack> {
    equipment
        .iter()
        .find(|(at, _)| *at == slot)
        .map(|(_, stack)| *stack)
}

/// `equipment` with `stack` in `slot`, nothing there where `None`, slots ascending.
fn with_worn(
    equipment: &[(u8, ItemStack)],
    slot: u8,
    stack: Option<ItemStack>,
) -> Vec<(u8, ItemStack)> {
    let mut changed: Vec<(u8, ItemStack)> = equipment
        .iter()
        .filter(|(at, _)| *at != slot)
        .copied()
        .collect();
    if let Some(stack) = stack.filter(|stack| stack.count > 0) {
        changed.push((slot, stack));
        changed.sort_by_key(|(at, _)| *at);
    }
    changed
}

/// The `minecraft:equippable` of the stack's item, where it has one.
fn equippable(stack: ItemStack) -> Option<clustine_data::Equippable> {
    usize::try_from(stack.item)
        .ok()
        .filter(|item| *item < clustine_data::ITEMS.len())
        .and_then(|_| ItemComponents::of(stack.item).equippable)
}

/// `isDisabled(slot)`: its bit of `disabledSlots`, or a hand of a stand without arms.
fn is_disabled(disabled_slots: i32, flags: u8, slot: u8) -> bool {
    disabled_slots & (1 << slots::filter_bit(slot)) != 0
        || (slots::is_hand(slot) && flags & SHOW_ARMS == 0)
}

/// `canUseSlot(slot)`: anything but an animal's body, a saddle and a disabled slot.
pub(super) fn can_use_slot(disabled_slots: i32, flags: u8, slot: u8) -> bool {
    slot != slots::BODY && slot != slots::SADDLE && !is_disabled(disabled_slots, flags, slot)
}

/// `getEquipmentSlotForItem(stack)`: the slot of its `equippable` where the stand can
/// use it, else the main hand.
fn slot_for_item(disabled_slots: i32, flags: u8, held: Option<ItemStack>) -> u8 {
    held.and_then(equippable)
        .map(|equippable| equippable.slot)
        .filter(|slot| can_use_slot(disabled_slots, flags, *slot))
        .unwrap_or(slots::MAIN_HAND)
}

/// `getClickedSlot(location)`: by the click's height on the stand, measured on a small
/// stand doubled, the first slot whose band it is in and that holds something; else
/// the off hand where only it holds something, else the main hand.
pub(crate) fn clicked_slot(location_y: f64, flags: u8, equipment: &[(u8, ItemStack)]) -> u8 {
    let small = flags & SMALL != 0;
    // `getScale() * getAgeScale()`, a float: the scale attribute's 1, halved for a
    // small stand, which is a baby.
    let scale = 1.0_f32 * if small { 0.5 } else { 1.0 };
    let y = location_y / f64::from(scale);
    let has = |slot: u8| worn(equipment, slot).is_some();
    let within = |low: f64, high: f64| y >= low && y < high;
    if within(0.1, 0.1 + if small { 0.8 } else { 0.45 }) && has(slots::FEET) {
        slots::FEET
    } else if within(
        0.9 + if small { 0.3 } else { 0.0 },
        0.9 + if small { 1.0 } else { 0.7 },
    ) && has(slots::CHEST)
    {
        slots::CHEST
    } else if within(0.4, 0.4 + if small { 1.0 } else { 0.8 }) && has(slots::LEGS) {
        slots::LEGS
    } else if y >= 1.6 && has(slots::HEAD) {
        slots::HEAD
    } else if !has(slots::MAIN_HAND) && has(slots::OFF_HAND) {
        slots::OFF_HAND
    } else {
        slots::MAIN_HAND
    }
}

/// `Mth.floor` of a float.
fn floor_float(value: f32) -> i32 {
    let truncated = value as i32;
    if value < truncated as f32 {
        truncated - 1
    } else {
        truncated
    }
}

/// `ArmorStandItem.useOn`'s yaw: the player's turned to face them, to the nearest 45
/// degrees, in floats.
pub(crate) fn placed_yaw(player_yaw: f32) -> f32 {
    floor_float((wrap_degrees(player_yaw - 180.0) + 22.5) / 45.0) as f32 * 45.0
}

/// The block one step from `pos` across `face`.
fn beside(pos: BlockPos, face: Face) -> BlockPos {
    match face {
        Face::Bottom => pos.offset(0, -1, 0),
        Face::Top => pos.offset(0, 1, 0),
        Face::North => pos.offset(0, 0, -1),
        Face::South => pos.offset(0, 0, 1),
        Face::West => pos.offset(-1, 0, 0),
        Face::East => pos.offset(1, 0, 0),
    }
}

/// `CollisionContext.empty()`: what a block's shape is to no entity at all.
const NO_ENTITY: Context = Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// The hand of a local player that a use of a stand was made with.
#[derive(Debug, Clone, Copy)]
struct Hand {
    player: PlayerId,
    slot: usize,
    main: bool,
}

impl Region {
    /// `ArmorStandItem.useOn` for the click `click`, whose clicked block the region
    /// holds: a stand at the clicked block where it gives way, else beside it, standing
    /// on what is under it, turned to face the player. Refused on the clicked block's
    /// underside, and where any block's shape or any entity (a dropped item among them)
    /// is in its box. A creative player keeps the item
    /// (`ServerPlayerGameMode.useItemOn`), and every player is one.
    pub(crate) fn place_armor_stand(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let _ = player;
        if click.held != Some(items::ARMOR_STAND) || click.face == Face::Bottom {
            return false;
        }
        // `BlockPlaceContext.getClickedPos`: an item that places no block replaces what
        // gives way to an empty hand.
        let Some(clicked) = self.block(click.position) else {
            return false;
        };
        let pos = if crate::placement::can_be_replaced_empty_handed(clicked) {
            click.position
        } else {
            beside(click.position, click.face)
        };
        let chunk = pos.chunk();
        if !self.readable(chunk) || self.block(pos).is_none() {
            return false;
        }
        let kind = u16::try_from(entity_types::ARMOR_STAND).expect("entity type ids fit 16 bits");
        let (width, height, _) = stand_size(kind, 0);
        let bottom = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let bounds = Aabb::of_entity(bottom, width, height);
        if !self.free_for_a_stand(&bounds) {
            return false;
        }
        // `EntityType.create(..., true, true)`: the stand put on what collides in its
        // block or the one below.
        let raised = Aabb::of_entity([bottom[0], bottom[1] + 1.0, bottom[2]], width, height);
        let y = f64::from(pos.y) + self.y_offset(pos, &raised);
        let position = [bottom[0], y, bottom[2]];
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        // The new stand's own draws: its UUID, then `LivingEntity`'s yaw, which the
        // placing turns over. The level's draw of a yaw in `EntityType.create`, also
        // turned over, is drawn nowhere: the repo keeps no stream of the level's that
        // anything later reads (the brief, finding 39).
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _yaw = own.next_float();
        let entity = Entity {
            key,
            kind,
            uuid,
            position,
            motion: [0.0; 3],
            yaw: placed_yaw(click.yaw),
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::ArmorStand {
                flags: 0,
                disabled_slots: 0,
                pose: Box::new(DEFAULT_POSE.map(|part| part.map(Float))),
                equipment: Vec::new(),
                health: Float(MAX_HEALTH),
                last_hit: None,
                air: MAX_AIR as i16,
                frozen: 0,
            },
        };
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_ARMOR_STAND_PLACE,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            volume: 0.75,
            pitch: 0.8,
            except: None,
        });
        if here {
            self.entity_draws.insert(key, 3);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        true
    }

    /// `level.noCollision(null, box)` and `level.getEntities(null, box).isEmpty()`: no
    /// block's shape in the box, no boat touching it, no entity's box meeting it (an
    /// item's, a cart's, a stand's), and no player the region knows of in it. Players of
    /// other regions are not known here (`docs/groundwork/n5-plan.md`, section 7 item 3).
    fn free_for_a_stand(&self, bounds: &Aabb) -> bool {
        let centre = bounds.center();
        let view = View {
            region: self,
            home: ChunkPos::containing(centre[0], centre[2]),
        };
        if !collide::block_collisions(&view, NO_ENTITY, bounds).is_empty() {
            return false;
        }
        let reach = bounds.inflate([1.0e-7; 3]);
        let in_the_way = self.colliders_in(&reach).iter().any(|other| {
            other.bounds.intersects(*bounds) || (other.solid && other.bounds.intersects(reach))
        });
        if in_the_way {
            return false;
        }
        !self.players.values().any(|player| {
            let at = player.pose.position;
            Aabb::of_entity([at.x, at.y, at.z], 0.6, 1.8).intersects(*bounds)
        })
    }

    /// `ArmorStandItem`'s `getYOffset(level, pos, true, box)`: how far above `pos` the
    /// box `raised` (the stand's, a block up) comes to rest moving down at most two, on
    /// the shapes of the block and the one below, and the boats there.
    fn y_offset(&self, pos: BlockPos, raised: &Aabb) -> f64 {
        let view = View {
            region: self,
            home: pos.chunk(),
        };
        let area = Aabb::of_block(pos).expand_towards([0.0, -1.0, 0.0]);
        let reach = area.inflate([1.0e-7; 3]);
        let mut shapes: Vec<_> = self
            .colliders_in(&reach)
            .into_iter()
            .filter(|other| other.solid && other.bounds.intersects(reach))
            .map(|other| collide::box_shape(&other.bounds))
            .collect();
        shapes.extend(
            collide::block_collisions(&view, NO_ENTITY, &area)
                .into_iter()
                .map(|found| found.shape),
        );
        1.0 + collide::collide_with_shapes([0.0, -2.0, 0.0], raised, &shapes)[1]
    }

    /// The living body of the stand `entity` of the loaded chunk `position`, as its tick
    /// starts.
    fn living_of(&self, entity: &Entity, position: ChunkPos) -> Option<Living> {
        let EntityData::ArmorStand {
            flags,
            equipment,
            health,
            air,
            frozen,
            ..
        } = &entity.data
        else {
            return None;
        };
        let (width, height, eye) = stand_size(entity.kind, *flags);
        let memory = self.entity_memory.get(&entity.key);
        let mut body = Body {
            position: entity.position,
            motion: entity.motion,
            on_ground: entity.on_ground,
            fall_distance: entity.fall_distance,
            width,
            height,
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
            item: false,
            fell_on: None,
            noclip: None,
            mover: self.mover_of(entity, position),
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
        let kind = Kind::ArmorStand {
            health: health.0,
            invulnerable: flags & (INVISIBLE | MARKER) != 0,
        };
        let mut living = Living::new(body, kind, eye);
        let marker = flags & MARKER != 0;
        living.travels = !marker;
        living.no_physics = marker;
        living.fire_ticks = entity.fire_ticks;
        living.air = i32::from(*air);
        living.frozen = i32::from(*frozen);
        living.first_tick = memory.is_none() && entity.age == 0;
        // `canFreeze`: nothing of `#freeze_immune_wearables` (leather) worn as armour.
        living.can_freeze = ![slots::FEET, slots::LEGS, slots::CHEST, slots::HEAD]
            .into_iter()
            .filter_map(|slot| worn(equipment, slot))
            .any(|stack| item_in_tag(stack.item, "minecraft:freeze_immune_wearables"));
        living.walks_on_powder_snow =
            worn(equipment, slots::FEET).is_some_and(|stack| stack.item == items::LEATHER_BOOTS);
        living.last_movements = memory.map_or_else(
            || {
                vec![Movement {
                    from: entity.position,
                    to: entity.position,
                    asked: None,
                }]
            },
            |memory| memory.last_movements.clone(),
        );
        Some(living)
    }

    /// `LivingEntity.tick()` of the stand `entity`: `baseTick`'s riding, its body's
    /// tick, what that ended in, and the carts it pushes.
    pub(crate) fn tick_armor_stand(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let _ = position;
        let key = entity.key;
        self.base_tick_riding(key, output);
        // Getting off what is gone may have moved it.
        let Some((position, entity)) = self
            .find_entity(key)
            .map(|(position, entity)| (position, entity.clone()))
        else {
            return;
        };
        let Some(mut living) = self.living_of(&entity, position) else {
            return;
        };
        let mut stream = self.entity_stream(key);
        living.tick(
            &View {
                region: self,
                home: position,
            },
            &mut stream,
        );
        if stream.used > 0 {
            *self.entity_draws.entry(key).or_default() += stream.used;
        }
        // Its landing, then what it stood on (`vibrations.rs`).
        let source = super::vibrations::Source::of(key);
        if let Some(on) = living.landed_on.take() {
            let supporting = living.body.supporting;
            self.entity_hit_ground(source, living.body.position, on, supporting);
        }
        for ore in std::mem::take(&mut living.ore) {
            self.stood_on(ore, source, living.body.position, output);
        }
        // The plates, tripwire and detector rails its walk met, which count it where
        // it is, a living entity (`pressed.rs`).
        let toucher = super::pressed::Toucher {
            key,
            bounds: living.body.bounds(),
            living: true,
        };
        for pos in std::mem::take(&mut living.inside) {
            self.entity_touched(pos, &toucher, output);
        }
        for sound in std::mem::take(&mut living.sounds) {
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
        match living.end {
            End::Lives => {}
            End::Broken => {
                self.broken_by_anything(&entity, output);
                self.kill_stand(key, output);
                return;
            }
            End::Killed => {
                self.kill_stand(key, output);
                return;
            }
        }
        if let Some(after) = self.write_stand(&entity, living, position, output) {
            self.push_carts(&after, output);
        }
    }

    /// What the body's tick leaves of the stand `before` of the chunk `position`: its
    /// record, its commit where it changed what is committed, its report where it
    /// changed what is shown, and the stand as it is now; `None` where it was sent on.
    fn write_stand(
        &mut self,
        before: &Entity,
        living: Living,
        position: ChunkPos,
        output: &mut TickOutput,
    ) -> Option<Entity> {
        let key = before.key;
        let mut data = before.data.clone();
        let EntityData::ArmorStand {
            health,
            air,
            frozen,
            ..
        } = &mut data
        else {
            return None;
        };
        let Kind::ArmorStand {
            health: health_now, ..
        } = living.kind;
        *health = Float(health_now);
        *air = i16::try_from(living.air).expect("air is kept between -20 and 300");
        *frozen = u16::try_from(living.frozen).expect("freezing is kept between 0 and 140");
        let health_changed = matches!(
            (&data, &before.data),
            (EntityData::ArmorStand { health: a, .. }, EntityData::ArmorStand { health: b, .. })
                if a != b
        );
        let mut after = Entity {
            position: living.body.position,
            motion: living.body.motion,
            on_ground: living.body.on_ground,
            fall_distance: living.body.fall_distance,
            fire_ticks: living.fire_ticks,
            age: before.age.saturating_add(1),
            data,
            ..before.clone()
        };
        self.entity_memory.insert(
            key,
            Memory {
                supporting: living.body.supporting,
                on_ground_no_blocks: living.body.on_ground_no_blocks,
                last_movements: living.last_movements.clone(),
                stuck_speed: living.body.stuck_speed,
                no_physics: living.no_physics,
                ticks_since_check: 0,
                cart: Default::default(),
                boat: Default::default(),
            },
        );
        let to = match self.relocate(position, before.position, &mut after, output) {
            Relocated::Sent => return None,
            Relocated::Moved(to) => to,
            Relocated::Stays => position,
        };
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let burning = |entity: &Entity| entity.fire_ticks > 0;
        let moved = bits(after.position) != bits(before.position);
        let motion_changed = bits(after.motion) != bits(before.motion);
        let data_changed = after.data != before.data;
        let committed = moved
            || motion_changed
            || after.on_ground != before.on_ground
            || burning(&after) != burning(before)
            || health_changed;
        if to == position {
            if committed {
                let mut fields = Vec::new();
                if moved {
                    fields.push(EntityField::Position(after.position));
                }
                if motion_changed {
                    fields.push(EntityField::Motion(after.motion));
                }
                if after.on_ground != before.on_ground {
                    fields.push(EntityField::OnGround(after.on_ground));
                }
                if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
                    fields.push(EntityField::FallDistance(after.fall_distance));
                }
                if after.fire_ticks != before.fire_ticks {
                    fields.push(EntityField::FireTicks(after.fire_ticks));
                }
                if data_changed {
                    fields.push(EntityField::Data(after.data.clone()));
                }
                self.commit_entity(position, key, fields, output);
            }
            // Its counters, committed or not, as the tick leaves them.
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let mut flags = 0;
        if living.body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if living.body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        if living.body.sync_position {
            flags |= Tracked::SYNC_POSITION;
        }
        if living.on_fire_flag {
            flags |= Tracked::ON_FIRE;
        }
        let burning_shown = living.on_fire_flag != (before.fire_ticks > 0);
        if committed || data_changed || burning_shown || to != position || living.body.sync_position
        {
            let mut report = tracked(&after, flags, data_changed);
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
        Some(after)
    }

    /// `ArmorStand.pushEntities()`: each rideable cart of the region's own chunks whose
    /// box meets the stand's and whose position is near its own is pushed by it
    /// (`AbstractMinecart.push(stand)`, [`Region::minecart_pushed_by`]). Carts over a
    /// line are not reached: a push there would be an effect.
    fn push_carts(&mut self, stand: &Entity, output: &mut TickOutput) {
        let bounds = self.entity_box(stand);
        let carts: Vec<EntityId> = self
            .entities_in(&bounds)
            .into_iter()
            .filter(|(other, _)| *other != stand.key)
            .filter_map(|(other, chunk)| {
                let record = self.entity_record(other, chunk)?;
                let rideable = i32::from(record.kind) == entity_types::MINECART;
                (rideable && physics::distance_sqr(record.position, stand.position) <= PUSH_REACH)
                    .then_some(other)
            })
            .collect();
        for cart in carts {
            self.minecart_pushed_by(cart, stand.key, output);
        }
    }

    /// `AbstractMinecart.push(entity)` of the cart `cart` by the entity `by`, an armour
    /// stand: the cart is pushed away from it, and the stand is given its share the other
    /// way (`Entity.push(x, y, z)`, which asks nothing of whether it is pushable).
    pub(crate) fn minecart_pushed_by(
        &mut self,
        cart: EntityId,
        by: EntityId,
        output: &mut TickOutput,
    ) {
        let Some((_, stand)) = self.find_entity(by) else {
            return;
        };
        let at = stand.position;
        // A cart does not push what it carries (`hasPassenger`).
        if let Some(push) = self.push_minecart(cart, at, Some(by), output) {
            self.push_entity(by, push, output);
        }
    }

    /// A player's hit on, or use of, the stand `entity`: made here, or by another
    /// region's player as an effect.
    pub(crate) fn armor_stand_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        match by {
            EntityUse::Attack { creative, .. } => {
                self.hit_armor_stand(entity.key, creative, output);
            }
            EntityUse::Interact {
                player,
                creative,
                held,
                location,
                ..
            } => {
                let used = Used {
                    player,
                    creative,
                    held,
                    location_y: location[1].0,
                };
                self.interact_armor_stand(entity.key, used, output);
            }
            // An explosion's hit across a line is step N6g's (`docs/groundwork/n6-plan.md`).
            EntityUse::Blasted { .. } => {}
        }
    }

    /// `ArmorStand.hurtServer` of a player's hit: nothing to an invisible stand or a
    /// marker; a creative player's breaks it at once, dropping nothing, not even what it
    /// wears; anyone else's is a first hit unless one came within five ticks, and the
    /// second breaks it, dropping the stand and what it wears.
    pub(crate) fn hit_armor_stand(
        &mut self,
        key: EntityId,
        creative: bool,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let EntityData::ArmorStand {
            flags, last_hit, ..
        } = &record.data
        else {
            return;
        };
        if flags & (INVISIBLE | MARKER) != 0 {
            return;
        }
        if creative {
            self.play_broken_sound(&record, output);
            self.kill_stand(key, output);
            return;
        }
        let now = record.age;
        let first = last_hit.is_none_or(|then| now.saturating_sub(then) > DOUBLE_HIT_WINDOW);
        if first {
            let mut data = record.data.clone();
            if let EntityData::ArmorStand { last_hit, .. } = &mut data {
                *last_hit = Some(now);
            }
            self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
            // `broadcastEntityEvent(this, 32)`: clients wobble it and play its hit.
            output.events.push(RegionEvent::EntityEvent {
                entity: key,
                chunk,
                event: 32,
            });
            return;
        }
        // `brokenByPlayer`: the stand's item where it stood, then what it wears.
        self.pop_resource_at(
            physics::containing(record.position),
            ItemStack {
                item: items::ARMOR_STAND,
                count: 1,
            },
            output,
        );
        self.broken_by_anything(&record, output);
        self.kill_stand(key, output);
    }

    /// `brokenByAnything`: the breaking sound, the empty death loot (which draws
    /// nothing), and each worn stack in the order of `EquipmentSlot.VALUES` popped out
    /// at the block above the stand's own.
    pub(super) fn broken_by_anything(&mut self, stand: &Entity, output: &mut TickOutput) {
        self.play_broken_sound(stand, output);
        let EntityData::ArmorStand { equipment, .. } = &stand.data else {
            return;
        };
        let above = physics::containing(stand.position).offset(0, 1, 0);
        for (_, stack) in equipment.clone() {
            self.pop_resource_at(above, stack, output);
        }
    }

    /// `playBrokenSound()`.
    fn play_broken_sound(&self, stand: &Entity, output: &mut TickOutput) {
        let [x, y, z] = stand.position;
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_ARMOR_STAND_BREAK,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(x, y, z),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// `kill`: the stand is removed at once, its passengers getting off it and it
    /// getting off what it rides.
    pub(super) fn kill_stand(&mut self, key: EntityId, output: &mut TickOutput) {
        self.unride_removed(key, true, output);
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let hops = record.hops;
        self.remove_entity(key, hops, chunk, output);
    }

    /// The hand of the region's player whose entity is `player` that holds `held`: the
    /// main hand where it does, else the off hand. **Inferred**: a use of an entity
    /// carries what the hand held and not which hand it was; where both hold the same,
    /// the main hand is taken.
    fn hand_holding(&self, player: EntityId, held: Option<ItemStack>) -> Option<Hand> {
        let (id, record) = self
            .players
            .iter()
            .find(|(_, record)| record.entity_id == player)?;
        let main = usize::from(record.selected_slot);
        if record.inventory.0[main] == held {
            return Some(Hand {
                player: *id,
                slot: main,
                main: true,
            });
        }
        (record.inventory.0[OFF_HAND] == held).then_some(Hand {
            player: *id,
            slot: OFF_HAND,
            main: false,
        })
    }

    /// `ArmorStand.interact(player, hand, location)`, then `swapItem`. A marker, and a
    /// name tag in hand, pass to the entity's default, which does nothing to a stand.
    fn interact_armor_stand(&mut self, key: EntityId, used: Used, output: &mut TickOutput) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let EntityData::ArmorStand {
            flags,
            disabled_slots,
            equipment,
            ..
        } = &record.data
        else {
            return;
        };
        let (flags, disabled_slots, equipment) = (*flags, *disabled_slots, equipment.clone());
        let held = used.held.filter(|stack| stack.count > 0);
        if flags & MARKER != 0 || held.is_some_and(|stack| stack.item == items::NAME_TAG) {
            return;
        }
        let for_item = slot_for_item(disabled_slots, flags, held);
        let slot = if held.is_none() {
            let clicked = clicked_slot(used.location_y, flags, &equipment);
            let slot = if is_disabled(disabled_slots, flags, clicked) {
                for_item
            } else {
                clicked
            };
            if worn(&equipment, slot).is_none() {
                return;
            }
            slot
        } else {
            if is_disabled(disabled_slots, flags, for_item)
                || (slots::is_hand(for_item) && flags & SHOW_ARMS == 0)
            {
                return;
            }
            for_item
        };
        // `swapItem(player, slot, held, hand)`: a slot whose taking or putting is
        // disabled refuses it.
        let on = worn(&equipment, slot);
        let filter = slots::filter_bit(slot);
        if on.is_some() && disabled_slots & (1 << (filter + 8)) != 0 {
            return;
        }
        if on.is_none() && disabled_slots & (1 << (filter + 16)) != 0 {
            return;
        }
        // What is put on, and what the hand is left with where that changes.
        let (put, left_in_hand) = match held {
            // A creative player puts one on and keeps the stack.
            Some(stack) if used.creative && on.is_none() => {
                (Some(ItemStack { count: 1, ..stack }), None)
            }
            Some(stack) if stack.count > 1 => {
                if on.is_some() {
                    return;
                }
                let left = ItemStack {
                    count: stack.count - 1,
                    ..stack
                };
                (Some(ItemStack { count: 1, ..stack }), Some(Some(left)))
            }
            // A swap: the stand takes what the hand held, the hand what it wore.
            _ => (held, Some(on)),
        };
        let hand = self.hand_holding(used.player, held);
        // What gives the player anything needs an answer, of which there is none across
        // a line (`docs/groundwork/n5-plan.md`, section 8 item 7).
        if left_in_hand.is_some() && hand.is_none() {
            return;
        }
        self.set_worn(key, slot, put, output);
        if let (Some(hand), Some(left)) = (hand, left_in_hand)
            && let Some(record) = self.players.get_mut(&hand.player)
        {
            record.inventory.set(hand.slot, left);
            self.journal.players.insert(hand.player);
            let slot = u8::try_from(hand.slot).expect("an inventory slot fits a byte");
            output.player_events.push((
                hand.player,
                PlayerEvent::InventorySlot { slot, stack: left },
            ));
        }
        // `SUCCESS_SERVER`: the arm swings for those who see the player.
        if let Some(hand) = hand
            && hand.main
        {
            self.swing_arm(
                hand.player,
                false,
                clustine_data::Swing::DEFAULT,
                true,
                output,
            );
        }
    }

    /// `setItemSlot(slot, stack)` of the stand `key`, committed and shown, and
    /// `onEquipItem`'s sound: an item of a slot of its own put in its slot plays its
    /// equip sound, but not in the stand's first tick, and not where the slot held the
    /// same item already.
    pub(super) fn set_worn(
        &mut self,
        key: EntityId,
        slot: u8,
        stack: Option<ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let EntityData::ArmorStand { equipment, .. } = &record.data else {
            return;
        };
        let old = worn(equipment, slot);
        let mut data = record.data.clone();
        if let EntityData::ArmorStand { equipment, .. } = &mut data {
            *equipment = with_worn(equipment, slot, stack);
        }
        self.commit_entity(chunk, key, vec![EntityField::Data(data.clone())], output);
        let after = Entity {
            data,
            ..record.clone()
        };
        // A report's burning holds until the next: it says it burns where it does.
        let burning = if after.fire_ticks > 0 {
            Tracked::ON_FIRE
        } else {
            0
        };
        let mut report = tracked(&after, burning, true);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
        let same = old.map(|stack| stack.item) == stack.map(|stack| stack.item);
        if same || record.age == 0 {
            return;
        }
        let Some(sound) = stack
            .and_then(equippable)
            .filter(|equippable| equippable.slot == slot)
            .map(|equippable| equippable.sound)
        else {
            return;
        };
        // `playSeededSound` takes its seed from the stand's own random: one long.
        *self.entity_draws.entry(key).or_default() += 1;
        let [x, y, z] = record.position;
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(x, y, z),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }
}

/// What a player's use of a stand carries.
#[derive(Debug, Clone, Copy)]
struct Used {
    /// The player's entity.
    player: EntityId,
    creative: bool,
    /// What the hand used holds.
    held: Option<ItemStack>,
    /// How high above the stand's feet the click landed.
    location_y: f64,
}
