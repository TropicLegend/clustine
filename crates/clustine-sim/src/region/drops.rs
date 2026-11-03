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
//! game does beside the loot is left out and named: the block entity a table would
//! copy from (`clustine_blocks::loot` names what it gives instead), and what a
//! container broken by a rule held, which the game spills beside its loot
//! (`Containers.dropContents`).

use clustine_blocks::loot::{Broken, LootDraws, block_drops, block_drops_as};
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

/// The experience the block of `state` gives when a player breaks it with the right
/// tool, the bounds of its `IntProvider` (inclusive), where it gives any: the ores'
/// `DropExperienceBlock`s as `Blocks`' static initialiser makes them (n6 Blocks,
/// `lambda$static$21` to `$27`, `$42`, `$43`, `$78`, `$79`, `$193`, `$194`, `$252`,
/// `$376`, `$377`: gold, iron and copper 0, coal 0 to 2, nether gold 0 to 1, lapis
/// and quartz 2 to 5, diamond and emerald 3 to 7), redstone ore 1 to 5
/// (`RedStoneOreBlock.spawnAfterBreak` @21-23) and the sculk sensors 5
/// (`SculkSensorBlock.spawnAfterBreak` @21). The spawner (15 to 43), sculk (1), the
/// sculk catalyst and the sculk shrieker (5 each) are the game's source's: their
/// classes are not among the dumps (named).
pub(crate) fn experience_range(state: BlockState) -> Option<(i32, i32)> {
    use clustine_blocks::data::is_block;
    let range = [
        (blocks::COAL_ORE, (0, 2)),
        (blocks::DEEPSLATE_COAL_ORE, (0, 2)),
        (blocks::NETHER_GOLD_ORE, (0, 1)),
        (blocks::LAPIS_ORE, (2, 5)),
        (blocks::DEEPSLATE_LAPIS_ORE, (2, 5)),
        (blocks::DIAMOND_ORE, (3, 7)),
        (blocks::DEEPSLATE_DIAMOND_ORE, (3, 7)),
        (blocks::EMERALD_ORE, (3, 7)),
        (blocks::DEEPSLATE_EMERALD_ORE, (3, 7)),
        (blocks::NETHER_QUARTZ_ORE, (2, 5)),
        (blocks::REDSTONE_ORE, (1, 5)),
        (blocks::DEEPSLATE_REDSTONE_ORE, (1, 5)),
        (blocks::SCULK_SENSOR, (5, 5)),
        (blocks::CALIBRATED_SCULK_SENSOR, (5, 5)),
        (blocks::SPAWNER, (15, 43)),
        (blocks::SCULK, (1, 1)),
        (blocks::SCULK_CATALYST, (5, 5)),
        (blocks::SCULK_SHRIEKER, (5, 5)),
    ];
    range
        .into_iter()
        .find(|(block, _)| is_block(state, *block))
        .map(|(_, range)| range)
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
            .unwrap_or_else(|| Random::of_block(self.seed(), pos, self.tick, DROPS));
        let stacks = block_drops(state, pos, &mut Loot(&mut random), &mut |at| {
            self.block(at).unwrap_or(blocks::AIR)
        });
        for stack in stacks {
            self.pop_resource(pos, stack, &mut random, output);
        }
        self.block_streams.insert(pos, random);
        // `state.spawnAfterBreak(level, pos, EMPTY, true)`, after the drops.
        self.infested_broken(pos, state, output);
    }

    /// `Block.dropResources(state, level, pos, blockEntity, player, tool)` of a block a
    /// survival player broke with `tool` in their main hand (`None` for a bare hand):
    /// its table evaluated with the player as `this_entity` and the tool
    /// (`Broken`), each stack popped out where the block was, then
    /// [`Region::spawn_after_break`] with the tool. The block entity a table would copy
    /// from is not given (named in `clustine_blocks::loot`).
    pub(crate) fn drop_resources_by_player(
        &mut self,
        state: BlockState,
        pos: BlockPos,
        tool: Option<&ItemStack>,
        output: &mut TickOutput,
    ) {
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.seed(), pos, self.tick, DROPS));
        let broken = Broken {
            this_entity: true,
            explosion_radius: None,
            tool,
        };
        let stacks = block_drops_as(state, pos, broken, &mut Loot(&mut random), &mut |at| {
            self.block(at).unwrap_or(blocks::AIR)
        });
        for stack in stacks {
            self.pop_resource(pos, stack, &mut random, output);
        }
        self.block_streams.insert(pos, random);
        self.spawn_after_break(pos, state, tool, output);
    }

    /// `state.spawnAfterBreak(level, pos, tool, true)` of a block a player broke with
    /// `tool`: an infested block's silverfish, unless the tool's enchantments are in
    /// `prevents_infested_spawns` (`InfestedBlock.spawnAfterBreak`, n9 @0-38; Silk Touch
    /// alone is), and the experience of an ore and of the blocks that give it
    /// ([`experience_range`]; `Block.tryDropExperience` @0-31), none with Silk Touch
    /// (`EnchantmentHelper.processBlockExperience`: its `block_experience` effect sets it
    /// to 0, the only enchantment of the game with one). The experience is drawn from
    /// the block's stream, as its drops are, where the game draws the level's random.
    pub(crate) fn spawn_after_break(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        tool: Option<&ItemStack>,
        output: &mut TickOutput,
    ) {
        let silk_touch = tool.is_some_and(|tool| {
            crate::items::enchantments::level(tool, "minecraft:silk_touch") > 0
        });
        if !silk_touch {
            self.infested_broken(pos, state, output);
        }
        let Some((low, high)) = experience_range(state) else {
            return;
        };
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.seed(), pos, self.tick, DROPS));
        // `IntProvider.sample`: `ConstantInt` draws nothing, `UniformInt` its
        // `randomBetweenInclusive`.
        let sampled = if low == high {
            low
        } else {
            random.next_int_between(low, high)
        };
        let amount = if silk_touch { 0 } else { sampled };
        // `popExperience`: `ExperienceOrb.award` at the block's centre, in orbs of the
        // game's values, each merged by a draw of 40 (`tryMergeToExisting`) or new.
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let mut left = amount;
        while left > 0 {
            let value = crate::mobs::body::hurt::experience_value(left);
            left -= value;
            let draw = random.next_int(40);
            self.award_orb(centre, value, draw, output);
        }
        self.block_streams.insert(pos, random);
    }

    /// `Block.popResource(level, pos, stack)` by an entity's rule (an armour stand's
    /// break): the level's draws from the stream of the block's drops, as a block's own
    /// drops at `pos` in this tick draw.
    pub(crate) fn pop_resource_at(
        &mut self,
        pos: BlockPos,
        stack: ItemStack,
        output: &mut TickOutput,
    ) {
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.seed(), pos, self.tick, DROPS));
        self.pop_resource(pos, stack, &mut random, output);
        self.block_streams.insert(pos, random);
    }

    /// `Block.popResource(level, pos, stack)`: three draws put the item within a
    /// quarter of a block of the block's centre each way, its feet half its height
    /// below, and drawn for an empty stack too; then the item, if there is one.
    pub(super) fn pop_resource(
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
        self.spawn_item_as(position, stack, DEFAULT_PICK_UP_DELAY, None, output)
    }

    /// [`Region::spawn_item`] with the pick-up delay `pick_up_delay` and, where given,
    /// the motion `motion` set after the constructor's (which draws its own all the
    /// same): `Containers.dropItemStack` sets no delay, the constructor's 0, and a
    /// motion of its own.
    pub(crate) fn spawn_item_as(
        &mut self,
        position: [f64; 3],
        stack: ItemStack,
        pick_up_delay: u16,
        motion: Option<[f64; 3]>,
        output: &mut TickOutput,
    ) -> bool {
        self.spawn_item_moved(position, stack, pick_up_delay, motion, None, output)
    }

    /// [`Region::spawn_item`] with the pick-up delay `pick_up_delay` and, where given,
    /// `push` added to the motion its constructor drew (a sheep's wool).
    pub(crate) fn spawn_item_pushed(
        &mut self,
        position: [f64; 3],
        stack: ItemStack,
        pick_up_delay: u16,
        push: Option<[f64; 3]>,
        output: &mut TickOutput,
    ) -> bool {
        self.spawn_item_moved(position, stack, pick_up_delay, None, push, output)
    }

    fn spawn_item_moved(
        &mut self,
        position: [f64; 3],
        stack: ItemStack,
        pick_up_delay: u16,
        motion: Option<[f64; 3]>,
        push: Option<[f64; 3]>,
        output: &mut TickOutput,
    ) -> bool {
        let chunk = ChunkPos::containing(position[0], position[2]);
        let Some(key) = self.take_entity_key(chunk) else {
            return false;
        };
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _bob = own.next_float();
        let yaw = own.next_float() * 360.0;
        let x = own.next_double() * 0.2 - 0.1;
        let z = own.next_double() * 0.2 - 0.1;
        self.entity_drew(key, 6);
        let entity = Entity {
            portal: Default::default(),
            key,
            kind: u16::try_from(entity_types::ITEM).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion: match push {
                Some(push) => crate::physics::add(motion.unwrap_or([x, 0.2, z]), push),
                None => motion.unwrap_or([x, 0.2, z]),
            },
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            data: EntityData::Item {
                stack,
                pick_up_delay,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
        };
        self.add_fresh_entity(entity, output);
        true
    }
}
