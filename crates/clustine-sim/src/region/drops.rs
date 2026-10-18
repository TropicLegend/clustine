//! What a block broken by a rule drops (`docs/groundwork/n4-plan.md`, section 7,
//! "Drops"; the brief's sections 8.1 and 8.4): the game's `Block.dropResources`, its
//! loot table evaluated by `clustine_blocks::loot`, and `Block.popResource` for each
//! stack, which lays a new item where the block was.
//!
//! The game draws the loot and the items' places from the level's one random, and each
//! new item's UUID, bob, yaw and motion from the item's own. Clustine draws the first
//! two from a stream of the block (the world, the block's position, the tick and a mark
//! of what it is for), kept for the rest of the tick so that two drops at one block in
//! one tick draw on from each other, and the item's own from its key's stream at its
//! birth, as a thrown item's are: the distributions are the game's, the values not.
//!
//! A drop needs its chunk loaded, not ticking: an item laid in a chunk that does not
//! tick lies there until it does. Into a full chunk ([`MAX_CHUNK_ENTITIES`]), or where
//! the region has no key left, a drop is lost, as a throw would not happen. What the
//! game does beside the loot is left out and named: the experience of ores and the
//! silverfish of infested blocks (`spawnAfterBreak`), the block entity a table would
//! copy from (`clustine_blocks::loot` names what it gives instead), and what a
//! container broken by a rule held, which the game spills beside its loot
//! (`Containers.dropContents`).

use clustine_blocks::loot::{LootDraws, block_drops};
use clustine_data::{BlockState, EntityTypeInfo, ITEMS, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, ItemStack};

use super::Region;
use super::entities::insecure_uuid;
use super::random_ticks::Random;
use crate::api::TickOutput;

/// Mixed into a block's stream for its drops.
const DROPS: u64 = 0x0064_726f_7073;

/// The pick-up delay of an item a block or an entity drops (`setDefaultPickUpDelay`).
pub(crate) const DEFAULT_PICK_UP_DELAY: u16 = 10;

/// A stream as the loot evaluator draws from it.
struct Loot<'a>(&'a mut Random);

impl LootDraws for Loot<'_> {
    fn next_float(&mut self) -> f32 {
        self.0.next_float()
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        let bound = u32::try_from(bound.max(1)).expect("a bound of at least 1");
        i32::try_from(self.0.next_int(bound)).expect("below a bound that fits an i32")
    }
}

/// `Mth.nextDouble(random, min, max)`.
fn next_double(random: &mut Random, min: f64, max: f64) -> f64 {
    if min >= max {
        min
    } else {
        random.next_double() * (max - min) + min
    }
}

/// The item of the block of `state` (`Block.asItem`), where it has one: the item that
/// places it, or places it on a wall.
pub(crate) fn item_of_block(state: BlockState) -> Option<i32> {
    let block = u16::try_from(state.block_id()?).ok()?;
    let index = ITEMS.iter().position(|item| {
        item.places == Some(block) || item.wall.is_some_and(|(wall, _)| wall == block)
    })?;
    i32::try_from(index).ok()
}

impl Region {
    /// `Block.dropResources(state, level, pos)`: what the block `state`, broken by a
    /// rule at `pos`, drops by its loot table, each stack popped out where it was.
    pub(crate) fn drop_resources(
        &mut self,
        state: BlockState,
        pos: BlockPos,
        output: &mut TickOutput,
    ) {
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.config.seed, pos, self.tick, DROPS));
        let stacks = block_drops(state, pos, &mut Loot(&mut random), &mut |at| {
            self.block(at).unwrap_or(blocks::AIR)
        });
        for stack in stacks {
            self.pop_resource(pos, stack, &mut random, output);
        }
        self.block_streams.insert(pos, random);
    }

    /// `Block.popResource(level, pos, stack)`: three draws put the item within a
    /// quarter of a block of the block's centre each way, its feet half its height
    /// below, and drawn for an empty stack too; then the item, if there is one.
    fn pop_resource(
        &mut self,
        pos: BlockPos,
        stack: ItemStack,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let info = EntityTypeInfo::of(entity_types::ITEM);
        let half = f64::from(info.height) / 2.0;
        let x = f64::from(pos.x) + 0.5 + next_double(random, -0.25, 0.25);
        let y = f64::from(pos.y) + 0.5 + next_double(random, -0.25, 0.25) - half;
        let z = f64::from(pos.z) + 0.5 + next_double(random, -0.25, 0.25);
        if stack.count <= 0 || stack.item == clustine_data::items::AIR {
            return;
        }
        self.spawn_item([x, y, z], stack, output);
    }

    /// `new ItemEntity(level, x, y, z, stack)` with `setDefaultPickUpDelay` and
    /// `addFreshEntity`: an item at `position` that the game's constructor sets off a
    /// little sideways and a fifth of a block a tick up. Its own stream gives what the
    /// constructor draws: its UUID, the bob of its model, its yaw, and its motion along
    /// x and then z. False, and nothing made, where its chunk is not loaded or full, or
    /// no key is left.
    pub(crate) fn spawn_item(
        &mut self,
        position: [f64; 3],
        stack: ItemStack,
        output: &mut TickOutput,
    ) -> bool {
        let chunk = ChunkPos::containing(position[0], position[2]);
        let Some(key) = self.take_entity_key(chunk) else {
            return false;
        };
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _bob = own.next_float();
        let yaw = own.next_float() * 360.0;
        let x = own.next_double() * 0.2 - 0.1;
        let z = own.next_double() * 0.2 - 0.1;
        self.entity_draws.insert(key, 6);
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::ITEM).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion: [x, 0.2, z],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            data: EntityData::Item {
                stack,
                pick_up_delay: DEFAULT_PICK_UP_DELAY,
                thrower: None,
                health: 5,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
        };
        self.add_fresh_entity(entity, output);
        true
    }
}
