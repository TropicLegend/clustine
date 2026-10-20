//! The region's side of the farm animals (`crate::mobs::types`; the brief's section 4):
//! a player's use of a mob and what it does beyond the mob (a stack given, items
//! dropped, a mooshroom turned into a cow), a spawn egg used on a mob of its own type,
//! the shears that take a saddle off, mobs made whole (a child, a converted mob), and the
//! biome at a block that a variant asks.
//!
//! Named difference: the biome at a block is the one its chunk keeps for the block's
//! cell, not the game's fiddled lookup of the nearest of eight cells (the region does
//! not know the world's seed it hashes with); only near a biome's edge can a variant
//! differ.

use clustine_data::{EntityTypeInfo, attributes as ids, items};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack};

use super::Region;
use super::entities::insecure_uuid;
use super::items::tracked;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput};
use crate::living::{MAX_AIR, slots};
use crate::mobs::Mob;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::{Head, attribute};
use crate::mobs::types::{self, Breeding, Did, Interaction, Used, animal};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

impl Region {
    /// The name of the biome at `pos`, where the region reads its chunk.
    pub(crate) fn biome_at(&self, pos: BlockPos) -> Option<&'static str> {
        let chunk = self.readable_chunk(pos.chunk())?;
        let index = usize::try_from((pos.y - chunk.min_y()).div_euclid(16)).ok()?;
        let section = chunk.sections().get(index)?;
        let (x, z) = pos.in_chunk();
        let biome = section.biome_at(x, pos.y.rem_euclid(16) as usize, z);
        clustine_data::synced_registry("minecraft:worldgen/biome")
            .and_then(|biomes| biomes.entries.get(usize::from(biome.0)))
            .copied()
    }

    /// What a player's use of a mob that did something ends with: `ENTITY_INTERACT`,
    /// heard by sculk, and the arm's swing of a player the region holds.
    pub(crate) fn interacted(
        &mut self,
        entity: &Entity,
        player: EntityId,
        result: Interaction,
        output: &mut TickOutput,
    ) {
        if let Some(vibration) = Vibration::named("minecraft:entity_interact") {
            let context = Context::of(Some(Source::of(player)), None);
            self.vibrate(vibration, entity.position, context);
        }
        if let Some(id) = self.player_with_entity(player)
            && matches!(result, Interaction::Success | Interaction::SuccessServer)
        {
            let own = result == Interaction::SuccessServer;
            self.swing_arm(id, false, clustine_data::Swing::DEFAULT, own, output);
        }
    }

    /// The mob `before` of the chunk `chunk` made `mob` between its ticks: committed
    /// where it changed, its data, attributes and equipment shown.
    pub(crate) fn commit_mob(
        &mut self,
        before: &Entity,
        chunk: ChunkPos,
        mob: &Mob,
        output: &mut TickOutput,
    ) {
        let fields = mob.changes_from(&before.data);
        if fields.is_empty() {
            return;
        }
        self.commit_entity(chunk, before.key, fields, output);
        if let Some((_, after)) = self.find_entity(before.key) {
            let after = after.clone();
            let mut report = tracked(&after, 0, true);
            report.chunk = chunk;
            output.events.push(RegionEvent::EntityTracked(report));
            self.mob_shown_changes(before, &after, chunk, output);
        }
    }

    /// Changes what every animal shares of the region's animal `key` between its ticks,
    /// at its `lived`, and commits it.
    pub(crate) fn update_animal(
        &mut self,
        key: EntityId,
        change: impl FnOnce(&mut animal::Animal, u32),
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let now = record.lived;
        animal::update(&mut mob, |shared| change(shared, now));
        self.commit_mob(&record, chunk, &mob, output);
    }

    /// `mobInteract`, then `ItemStack.interactLivingEntity` where that passed, of the
    /// player `player`'s use of the mob `entity`: drawn from the mob's stream aside;
    /// the mob committed, then what the use did beyond it.
    #[expect(
        clippy::too_many_arguments,
        reason = "the use's parts, as the effect has them"
    )]
    pub(crate) fn mob_used_by(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        player: EntityId,
        held: Option<ItemStack>,
        creative: bool,
        sneaking: bool,
        from: Vec3,
        output: &mut TickOutput,
    ) {
        let Ok(mut mob) = Mob::decode(&entity.data) else {
            return;
        };
        let mob_type = types::of(entity.kind);
        let mut stream = self.entity_stream(entity.key).random;
        let start = stream.clone();
        let mut used = Used {
            key: entity.key,
            kind: entity.kind,
            mob: &mut mob,
            player,
            held,
            creative,
            sneaking,
            from,
            position: entity.position,
            moment: entity.lived,
            draws: &mut stream,
            did: Vec::new(),
        };
        let mut result = mob_type.mob_interact(&mut used);
        if result == Interaction::Pass {
            result = mob_type.item_interact(&mut used);
        }
        let did = std::mem::take(&mut used.did);
        self.entity_drew(entity.key, stream.draws_since(&start));
        if result == Interaction::Pass {
            return;
        }
        self.commit_mob(entity, chunk, &mob, output);
        for done in did {
            self.carry_out(entity, chunk, player, from, done, output);
        }
        self.interacted(entity, player, result, output);
    }

    /// One thing a use of the mob `entity` did beyond it.
    fn carry_out(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        player: EntityId,
        from: Vec3,
        done: Did,
        output: &mut TickOutput,
    ) {
        match done {
            Did::Sound {
                sound,
                source,
                at,
                volume,
                pitch,
                but_the_player,
            } => {
                let except = if but_the_player {
                    self.player_with_entity(player)
                } else {
                    None
                };
                output.events.push(RegionEvent::Sound {
                    sound,
                    source,
                    position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                    volume,
                    pitch,
                    except,
                });
            }
            Did::Event(event) => output.events.push(RegionEvent::EntityEvent {
                entity: entity.key,
                chunk,
                event,
            }),
            Did::GameEvent(name) => {
                if let Some(vibration) = Vibration::named(name) {
                    let context = Context::of(Some(Source::of(player)), None);
                    self.vibrate(vibration, entity.position, context);
                }
            }
            Did::Give {
                stack,
                no_duplicates,
            } => self.give_to_player(player, stack, no_duplicates, from, output),
            Did::Drop {
                stack,
                at,
                pick_up_delay,
                push,
            } => {
                self.spawn_item_pushed(at, stack, pick_up_delay, push, output);
            }
            Did::Convert { kind, drops } => self.convert_mob(entity.key, kind, &drops, output),
        }
    }

    /// `Mob.convertTo(kind, single)` of the region's mob `key`: a mob of `kind` made
    /// (`EntityType.create`, no `finalizeSpawn`) where it stands, as it faces, keeping
    /// its flags; `drops` laid at its top, each one by one; the new mob added; itself
    /// gone. Its new key is the region's, from the mob's chunk.
    pub(crate) fn convert_mob(
        &mut self,
        key: EntityId,
        kind: u16,
        drops: &[ItemStack],
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        let Ok(old) = Mob::decode(&record.data) else {
            return;
        };
        let mut made = Mob::fresh();
        // `ConversionType.SINGLE`'s copy of what the old mob was beside its body.
        made.body.flags = old.body.flags.clone();
        made.body.head = old.body.head.clone();
        let Some(mut converted) =
            self.new_mob(kind, made, record.position, record.yaw, record.pitch)
        else {
            return;
        };
        converted.motion = record.motion;
        let height = types::own_size(&record)
            .map_or(EntityTypeInfo::of(i32::from(record.kind)).height, |size| {
                size.height
            });
        let top = [
            record.position[0],
            record.position[1] + f64::from(height),
            record.position[2],
        ];
        for stack in drops {
            for _ in 0..stack.count {
                let one = ItemStack {
                    item: stack.item,
                    count: 1,
                };
                self.spawn_item_as(top, one, 0, None, output);
            }
        }
        self.add_made_mob(converted, output);
        let hops = record.hops;
        self.unride_removed(key, true, output);
        self.remove_entity(key, hops, chunk, output);
    }

    /// A new mob of the type `kind` from `mob` (its type's state and what its maker
    /// set), at `position` facing `yaw` and `pitch`, as `EntityType.create` makes it:
    /// its key (the region's for a chunk it ticks), its UUID and its constructor's
    /// draws from its own stream, its health at its most and its air full. Not added
    /// yet ([`Region::add_made_mob`]).
    pub(crate) fn new_mob(
        &mut self,
        kind: u16,
        mut mob: Mob,
        position: Vec3,
        yaw: f32,
        pitch: f32,
    ) -> Option<Entity> {
        let chunk = ChunkPos::containing(position[0], position[2]);
        if !self.readable(chunk) {
            return None;
        }
        let key = if self.ticks_chunk(chunk) {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _constructor_yaw = own.next_float();
        types::of(kind).construct(&mut mob, &mut own);
        if self.ticks_chunk(chunk) {
            self.entity_drew(key, own.draws_since(&start));
        }
        mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap_or(20.0) as f32;
        mob.body.vitals.air = MAX_AIR;
        if mob.body.head == Head::default() {
            mob.body.head = Head {
                head_yaw: yaw,
                body_yaw: yaw,
                still_since: 0,
                ..Head::default()
            };
        }
        Some(Entity {
            key,
            kind,
            uuid,
            position,
            motion: [0.0; 3],
            yaw,
            pitch,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: mob.encode(),
        })
    }

    /// Adds a mob [`Region::new_mob`] made: into its chunk where the region ticks it,
    /// else sent to whoever holds it.
    pub(crate) fn add_made_mob(&mut self, entity: Entity, output: &mut TickOutput) {
        if self.ticks_chunk(entity.chunk()) {
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// `SpawnEggItem.spawnOffspringFromSpawnEgg`: an egg of the type of the farm animal
    /// `entity` used on it makes a baby of it at its feet, facing south, as its own
    /// breeding with itself would (no `finalizeSpawn`); the creative player keeps the
    /// egg. Whether it made one.
    pub(crate) fn offspring_from_egg(
        &mut self,
        entity: &Entity,
        held: Option<ItemStack>,
        output: &mut TickOutput,
    ) -> bool {
        let Some(stack) = held.filter(|stack| stack.count > 0) else {
            return false;
        };
        let egg = clustine_data::ItemComponents::of(stack.item).entity;
        if egg != Some(entity.kind) || !types::is_built(entity.kind) {
            return false;
        }
        let Ok(parent) = Mob::decode(&entity.data) else {
            return false;
        };
        let mut stream = self.entity_stream(entity.key).random;
        let start = stream.clone();
        let biome = self.biome_at(crate::physics::containing(entity.position));
        let child = types::of(entity.kind).breed_offspring(&mut Breeding {
            this: &parent,
            partner: &parent,
            draws: &mut stream,
            biome,
        });
        self.entity_drew(entity.key, stream.draws_since(&start));
        let Some(mut child) = child else {
            return false;
        };
        animal::made_baby(&mut child);
        let Some(made) = self.new_mob(entity.kind, child, entity.position, 0.0, 0.0) else {
            return false;
        };
        self.add_made_mob(made, output);
        true
    }

    /// `Mob.attemptToShearEquipment`: shears take off the first thing the mob `entity`
    /// wears that may be shorn (a saddle), dropped at the middle of its passengers'
    /// seats with its shearing sound and `GameEvent.SHEAR`. Whether there was one.
    pub(crate) fn shear_equipment(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        player: EntityId,
        output: &mut TickOutput,
    ) -> bool {
        let Ok(mut mob) = Mob::decode(&entity.data) else {
            return false;
        };
        // Of the slots in `EquipmentSlot.VALUES` order, a saddle's alone may be shorn.
        let saddle = usize::from(slots::SADDLE);
        let Some(stack) =
            mob.body.equipment.slots[saddle].filter(|stack| stack.item == items::SADDLE)
        else {
            return false;
        };
        mob.body.equipment.slots[saddle] = None;
        self.commit_mob(entity, chunk, &mob, output);
        if let Some(vibration) = Vibration::named("minecraft:shear") {
            let context = Context::of(Some(Source::of(player)), None);
            self.vibrate(vibration, entity.position, context);
        }
        // `getAttachments().getAverage(PASSENGER)`.
        let points = EntityTypeInfo::of(i32::from(entity.kind)).passenger_points;
        let mut offset = [0.0; 3];
        if !points.is_empty() {
            for point in points {
                for axis in 0..3 {
                    offset[axis] += point[axis];
                }
            }
            for value in &mut offset {
                *value /= points.len() as f64;
            }
        }
        let at = crate::physics::add(entity.position, offset);
        self.spawn_item(at, stack, output);
        output.events.push(RegionEvent::Sound {
            sound: clustine_data::sounds::ITEM_SADDLE_UNEQUIP,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(
                entity.position[0],
                entity.position[1],
                entity.position[2],
            ),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        self.interacted(entity, player, Interaction::Success, output);
        true
    }
}
