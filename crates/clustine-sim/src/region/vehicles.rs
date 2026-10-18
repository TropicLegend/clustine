//! What minecarts and boats share (`docs/groundwork/n5-brief.md`, sections 3.14, 3.18
//! and 4.10; `docs/groundwork/n5-plan.md`, section 7 item 10): `VehicleEntity`'s hits,
//! its wobble and damage falling by one a tick, its breaking by a hit of more than 40 or
//! by any creative hit, the item it drops, and `ContainerEntity`'s spill of what a chest
//! or hopper cart or a chest boat holds whenever it is removed as destroyed.
//!
//! The hurt data are counters, as items' ages are: their fall is kept in the record in
//! place and saved at the worker's checkpoints, and a hit commits them
//! (`docs/groundwork/n5-plan.md`, section 7 item 7). The game's `entityDrops` rule is
//! true here: Clustine has no game rules.
//!
//! The level's random that a spill draws from is the region's stream of the block the
//! vehicle is in, as a block's drops draw (`drops.rs`): a spill and a drop at one block
//! in one tick draw on from each other, as from the game's one random.

use clustine_data::ITEMS;
use clustine_data::entity_types;
use clustine_world::{Contents, EntityData, EntityField, EntityId, Float, Hurt, ItemStack};

use super::Region;
use super::random_ticks::Random;
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::physics::entities::is_boat;

/// Mixed into a block's stream for the spill of a container entity.
pub(crate) const SPILLS: u64 = 0x7370_696c_6c73;

/// `ITEM.getWidth()`, the float 0.25 widened.
const ITEM_WIDTH: f64 = 0.25;

/// The spread of a spilled item's motion (`Containers.dropItemStack`).
const SPILL_SPREAD: f64 = 0.114_850_001_711_398_36;

/// The wobble a hit starts (`setHurtTime(10)`).
const HURT_TIME: i32 = 10;

/// The damage above which a hit breaks a vehicle.
const BREAKING_DAMAGE: f32 = 40.0;

/// The hurt data of a vehicle's record.
pub(crate) fn hurt_of(data: &EntityData) -> Option<Hurt> {
    match data {
        EntityData::Minecart { hurt, .. } | EntityData::Boat { hurt, .. } => Some(*hurt),
        _ => None,
    }
}

/// The record's data with the hurt data `hurt`.
fn with_hurt(data: &EntityData, new: Hurt) -> EntityData {
    let mut data = data.clone();
    if let EntityData::Minecart { hurt, .. } | EntityData::Boat { hurt, .. } = &mut data {
        *hurt = new;
    }
    data
}

/// The stacks of a container entity's record.
pub(crate) fn contents_of(data: &EntityData) -> Option<&Contents> {
    match data {
        EntityData::Minecart { contents, .. } | EntityData::Boat { contents, .. } => Some(contents),
        _ => None,
    }
}

/// The slots of a container entity of the type `kind` (`getContainerSize`): 27 for a
/// chest minecart and a chest boat or raft, 5 for a hopper minecart, none for the rest.
pub(crate) fn container_size(kind: u16) -> usize {
    match i32::from(kind) {
        entity_types::CHEST_MINECART => 27,
        entity_types::HOPPER_MINECART => 5,
        _ if crate::physics::entities::is_chest_boat(kind) => 27,
        _ => 0,
    }
}

/// `getDropItem()` of a vehicle of the type `kind`: the plain minecart for the plain,
/// spawner and command block carts, each other cart its own item, and a boat the item
/// its type was made with. **Inferred** for boats: the type's supplier is set where the
/// types are registered, which the dump does not have; the item of the type's own name
/// is taken.
pub(crate) fn vehicle_drop_item(kind: u16) -> Option<i32> {
    use clustine_data::items;
    match i32::from(kind) {
        entity_types::MINECART
        | entity_types::SPAWNER_MINECART
        | entity_types::COMMAND_BLOCK_MINECART => Some(items::MINECART),
        entity_types::CHEST_MINECART => Some(items::CHEST_MINECART),
        entity_types::FURNACE_MINECART => Some(items::FURNACE_MINECART),
        entity_types::TNT_MINECART => Some(items::TNT_MINECART),
        entity_types::HOPPER_MINECART => Some(items::HOPPER_MINECART),
        _ if is_boat(kind) => {
            let name = clustine_data::ENTITY_TYPES.get(usize::from(kind))?;
            let index = ITEMS.iter().position(|item| item.name == *name)?;
            i32::try_from(index).ok()
        }
        _ => None,
    }
}

/// `Containers.dropItemStack`'s place for one slot, drawn whether or not it holds
/// anything: within the block of `position`, an item's half width in from its sides.
fn spill_place(position: [f64; 3], random: &mut Random) -> [f64; 3] {
    let x = position[0].floor() + random.next_double() * (1.0 - ITEM_WIDTH) + ITEM_WIDTH / 2.0;
    let y = position[1].floor() + random.next_double() * (1.0 - ITEM_WIDTH);
    let z = position[2].floor() + random.next_double() * (1.0 - ITEM_WIDTH) + ITEM_WIDTH / 2.0;
    [x, y, z]
}

/// `RandomSource.triangle(centre, spread)`.
fn triangle(random: &mut Random, centre: f64, spread: f64) -> f64 {
    let first = random.next_double();
    let second = random.next_double();
    centre + spread * (first - second)
}

impl Region {
    /// The fall of a vehicle's wobble and damage by one in its tick (`AbstractMinecart.tick`
    /// and `AbstractBoat.tick` begin so), kept in place as counters are; the edges are
    /// told, as the game sends the data each tick it changes.
    pub(crate) fn decay_hurt(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let Some(hurt) = hurt_of(&record.data) else {
            return;
        };
        let mut after = hurt;
        if after.time > 0 {
            after.time -= 1;
        }
        if after.damage.0 > 0.0 {
            after.damage = Float(after.damage.0 - 1.0);
        }
        if after == hurt {
            return;
        }
        let data = with_hurt(&record.data, after);
        let hops = record.hops;
        self.age_entity(chunk, key, vec![EntityField::Data(data.clone())], output);
        output.events.push(RegionEvent::DataChanged {
            entity: key,
            chunk,
            hops,
            data,
        });
    }

    /// `VehicleEntity.hurtServer(source, amount)` of the vehicle `key`, hit by an
    /// attacker that is a creative player or not: the wobble turns the other way and
    /// starts again, the damage grows by ten times the amount (in floats), and the
    /// vehicle breaks where it is above 40 and the attacker is not creative, or is
    /// discarded, dropping nothing of its own, where the attacker is. False where there
    /// is no such vehicle. The `Invulnerable` tag is not kept by Clustine, so nothing
    /// is invulnerable to a hit.
    pub(crate) fn hurt_vehicle(
        &mut self,
        key: EntityId,
        amount: f32,
        creative: bool,
        output: &mut TickOutput,
    ) -> bool {
        let Some((chunk, record)) = self.find_entity(key) else {
            return false;
        };
        let Some(hurt) = hurt_of(&record.data) else {
            return false;
        };
        let after = Hurt {
            time: HURT_TIME,
            direction: -hurt.direction,
            damage: Float(hurt.damage.0 + amount * 10.0),
        };
        let data = with_hurt(&record.data, after);
        let record = record.clone();
        // The new data are sent before any removal, and `markHurt` sends the motion.
        self.commit_entity(chunk, key, vec![EntityField::Data(data.clone())], output);
        output.events.push(RegionEvent::DataChanged {
            entity: key,
            chunk,
            hops: record.hops,
            data,
        });
        let mut report = super::items::tracked(&record, Tracked::SYNC_VELOCITY, false);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
        if !creative && after.damage.0 > BREAKING_DAMAGE {
            self.destroy_vehicle(key, output);
        } else if creative {
            self.remove_destroyed(key, output);
        }
        true
    }

    /// `VehicleEntity.destroy(level, source)`: the vehicle is killed, spilling what it
    /// holds, then drops its own item where it is, with the default pick-up delay; a
    /// container vehicle then spills again (`chestVehicleDestroyed`), which finds its
    /// slots empty and only draws.
    pub(crate) fn destroy_vehicle(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        self.remove_destroyed(key, output);
        if let Some(item) = vehicle_drop_item(record.kind) {
            // The vehicle's custom name would go onto the item; Clustine keeps none.
            self.spawn_item(record.position, ItemStack { item, count: 1 }, output);
        }
        let size = container_size(record.kind);
        if size > 0 {
            self.drop_contents(record.position, &Vec::new(), size, output);
        }
    }

    /// `remove(reason)` of an entity removed as destroyed, killed or discarded: a
    /// container spills what it holds first (`AbstractMinecartContainer.remove`,
    /// `AbstractChestBoat.remove`), then `setRemoved` lets its passengers off and, if
    /// killed or discarded, takes it off what it rides, and the record goes.
    pub(crate) fn remove_destroyed(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let size = container_size(record.kind);
        if size > 0 {
            let contents = contents_of(&record.data).cloned().unwrap_or_default();
            self.drop_contents(record.position, &contents, size, output);
        }
        // Both reasons destroy (`RemovalReason.shouldDestroy`).
        self.unride_removed(key, true, output);
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let hops = record.hops;
        self.remove_entity(key, hops, chunk, output);
    }

    /// `Containers.dropContents(level, x, y, z, container)` at `position` for a
    /// container of `size` slots holding `contents`: every slot in order draws its
    /// place from the level's random, empty or not, and what it holds comes out in
    /// pieces of 10 to 30, each drawn before the item is made, the item's own draws
    /// made as it is, and its motion drawn after from the level's random. The items keep
    /// the constructor's pick-up delay, none.
    pub(crate) fn drop_contents(
        &mut self,
        position: [f64; 3],
        contents: &Contents,
        size: usize,
        output: &mut TickOutput,
    ) {
        let block = crate::physics::containing(position);
        let mut random = self
            .block_streams
            .remove(&block)
            .unwrap_or_else(|| Random::of_block(self.config.seed, block, self.tick, SPILLS));
        for slot in 0..size {
            let place = spill_place(position, &mut random);
            let held = contents
                .iter()
                .find(|(at, _)| usize::from(*at) == slot)
                .map(|(_, stack)| *stack);
            let Some(mut stack) = held else {
                continue;
            };
            while stack.count > 0 && stack.item != clustine_data::items::AIR {
                let piece = i32::try_from(random.next_int(21)).expect("below 21") + 10;
                let taken = piece.min(stack.count);
                stack.count -= taken;
                let motion = [
                    triangle(&mut random, 0.0, SPILL_SPREAD),
                    triangle(&mut random, 0.2, SPILL_SPREAD),
                    triangle(&mut random, 0.0, SPILL_SPREAD),
                ];
                // The motion is drawn after the item's own draws, which come from its
                // own stream: the order between the two streams does not matter.
                self.spawn_item_as(
                    place,
                    ItemStack {
                        item: stack.item,
                        count: taken,
                    },
                    0,
                    Some(motion),
                    output,
                );
            }
        }
        self.block_streams.insert(block, random);
    }
}
