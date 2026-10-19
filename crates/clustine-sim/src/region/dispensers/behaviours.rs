//! What a dispenser does with each item (`DispenseItemBehavior.bootStrap` and
//! `DispenserBlock.getDefaultDispenseMethod`; `docs/groundwork/n6-brief.md`, sections 4.3
//! to 4.16): the items the game registers by name, then by their components (anything
//! `minecraft:equippable` goes on whoever stands in front), and the rest dropped.
//!
//! A behaviour changes the dispensed stack in place, as the game's changes the slot's own
//! object, and gives back what the slot is to hold; the click (1000, or 1001 for a
//! behaviour that may fail and did) and the smoke (2000) follow it, and a behaviour that
//! falls back to a whole inner dispense (a bucket that cannot be emptied, a boat or cart
//! with nowhere to go, a potion other than water) clicks and smokes twice, as the game's.
//!
//! Named differences, each where the step that brings what it acts on will port it:
//! - projectiles (arrows, eggs, snowballs, bottles of experience, splash and lingering
//!   potions, fireworks, fire and wind charges) are dropped as items until N9
//!   (decision 7);
//! - what acts on mobs waits for N7: shears shear no sheep, a brush brushes no
//!   armadillo, a chest goes on no horse (it is dropped), a spawn egg spawns nothing (it
//!   stays, with the click and smoke of a spawn that made nothing), a mob bucket
//!   releases no fish, axolotl or tadpole (the water is placed), a sulfur cube bucket,
//!   whose content is no fluid, is dropped, no sulfur cube swallows or is lit, a glass
//!   bottle or shears release no bees from a hive, a carved pumpkin or a wither skeleton
//!   skull makes no golem or wither (it goes on a head or stays);
//! - bone meal grows nothing, as Clustine has no growth by it: it stays with the click
//!   of a failure, as the game's does where nothing can grow (brief 4.12's
//!   recommendation);
//! - stacks have no components (decision 3): a flint and steel and shears never wear
//!   out, a glass bottle filled from water gives a potion with no contents, and every
//!   potion dispensed is taken for water (a water bottle makes mud);
//! - a command block minecart, which Clustine does not have, is dropped;
//! - water does not evaporate where it is dispensed, as Clustine has no dimension where
//!   it does;
//! - a water bottle's five splashes over the dispenser are drawn and not sent, as the
//!   repo sends no particles;
//! - a shulker box is not placed into a player's box, and is into any other entity's,
//!   as a player's placing is;
//! - among the living entities in front, an armour stand comes before a player.
//!
//! Read from what the brief names as unread (section 12) and taken as the game's
//! source has them, **Inferred**: a powder snow bucket places powder snow into air with
//! its own sound (`SolidBucketItem.emptyContents`); an empty bucket takes a source, the
//! water of a waterlogged block (which goes if it can no longer stay), a bubble column
//! or powder snow (`BucketPickup.pickupBlock`); glowstone charges an anchor with its
//! sound (`RespawnAnchorBlock.charge`); honeycomb waxes the copper that weathers into
//! its waxed block of the same name (`HoneycombItem.getWaxed`); shears take three
//! honeycombs from a full hive (`BeehiveBlock.dropHoneycomb`).

use super::super::vibrations::Vibration;
use clustine_blocks::data::{
    face_name, flag, has_lava, has_water, in_tag, is_block, is_class, number, relative, set,
    set_flag,
};
use clustine_blocks::fluid::{FluidState, Kind};
use clustine_blocks::survive::can_survive;
use clustine_data::{
    BLOCKS, BlockClass, BlockState, EntityTypeInfo, Face, ITEMS, ItemClass, ItemComponents, blocks,
    entity_types, items, sounds,
};
use clustine_world::{
    ARMOUR_START, BlockPos, ChunkPos, Double, Entity, EntityData, EntityId, Float, Hurt, OFF_HAND,
};

use super::super::Region;
use super::super::entities::insecure_uuid;
use super::super::items::Memory;
use super::super::random_ticks::Random;
use super::super::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT, fluid_block};
use super::{
    DISPENSED, Dispense, FAILED, WAX_ON, empty_sound, is_shulker_box_item, level_event, y_rot,
};
use crate::api::TickOutput;
use crate::api::{ItemStack, PlayerEvent, RegionEvent, SOUND_SOURCE_BLOCKS, SOUND_SOURCE_PLAYERS};
use crate::living::{MAX_AIR, slots};
use crate::physics::aabb::Aabb;

/// What a dispenser does with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behaviour {
    /// `DefaultDispenseItemBehavior`: one dropped. Also the projectiles' until N9, a
    /// chest's, and a block a sulfur cube would swallow.
    Drop,
    /// `$1`: an armour stand placed in front.
    ArmourStand,
    /// `BoatDispenseItemBehavior` of the boat (or raft) type `kind`.
    Boat(u16),
    /// `$3`: a full bucket of the item emptied in front.
    FullBucket(i32),
    /// `$4`: an empty bucket filled in front.
    EmptyBucket,
    /// `FlintAndSteelDispenseItemBehavior`.
    FlintAndSteel,
    /// `$5`.
    BoneMeal,
    /// `$6`: primed TNT in front.
    Tnt,
    /// `$7` and `$8`: a wither skeleton skull or a carved pumpkin, which make no wither
    /// or golem before N7 and go on a head or stay.
    Head,
    /// `ShulkerBoxDispenseBehavior`.
    ShulkerBox,
    /// `$9`.
    GlassBottle,
    /// `$10`.
    Glowstone,
    /// `ShearsDispenseItemBehavior`.
    Shears,
    /// `$11`.
    Brush,
    /// `$12`.
    Honeycomb,
    /// `$13`.
    Potion,
    /// `MinecartDispenseItemBehavior` of a cart of the entity type, where Clustine has it.
    Minecart(Option<i32>),
    /// `EquipmentDispenseItemBehavior`: the item has `minecraft:equippable`.
    Equipment,
    /// `SpawnEggItemBehavior`.
    SpawnEgg,
}

impl Behaviour {
    /// Whether the behaviour reads or changes the block in front or what is in it, so
    /// that it fails where that block is another region's. Dropping, priming TNT and
    /// placing an armour stand only make an entity, which goes there as an arrival.
    fn reads_front(self) -> bool {
        !matches!(
            self,
            Behaviour::Drop
                | Behaviour::ArmourStand
                | Behaviour::Tnt
                | Behaviour::SpawnEgg
                | Behaviour::Brush
                | Behaviour::BoneMeal
        )
    }
}

/// `DispenserBlock.getDispenseMethod` of `item`: what `bootStrap` registered for it, else
/// what its components make it.
pub(crate) fn behaviour_of(item: i32) -> Behaviour {
    if let Some(kind) = super::super::boats::boat_of_item(item) {
        return Behaviour::Boat(kind);
    }
    match item {
        items::ARMOR_STAND => Behaviour::ArmourStand,
        items::LAVA_BUCKET
        | items::WATER_BUCKET
        | items::POWDER_SNOW_BUCKET
        | items::SALMON_BUCKET
        | items::COD_BUCKET
        | items::PUFFERFISH_BUCKET
        | items::TROPICAL_FISH_BUCKET
        | items::AXOLOTL_BUCKET
        | items::SULFUR_CUBE_BUCKET
        | items::TADPOLE_BUCKET => Behaviour::FullBucket(item),
        items::BUCKET => Behaviour::EmptyBucket,
        items::FLINT_AND_STEEL => Behaviour::FlintAndSteel,
        items::BONE_MEAL => Behaviour::BoneMeal,
        items::TNT => Behaviour::Tnt,
        items::WITHER_SKELETON_SKULL | items::CARVED_PUMPKIN => Behaviour::Head,
        items::GLASS_BOTTLE => Behaviour::GlassBottle,
        items::GLOWSTONE => Behaviour::Glowstone,
        items::SHEARS => Behaviour::Shears,
        items::BRUSH => Behaviour::Brush,
        items::HONEYCOMB => Behaviour::Honeycomb,
        items::POTION => Behaviour::Potion,
        items::MINECART
        | items::CHEST_MINECART
        | items::FURNACE_MINECART
        | items::TNT_MINECART
        | items::HOPPER_MINECART
        | items::COMMAND_BLOCK_MINECART => {
            Behaviour::Minecart(super::super::minecarts::kind_of_item(item))
        }
        _ if is_shulker_box_item(item) => Behaviour::ShulkerBox,
        _ if equippable(item).is_some() => Behaviour::Equipment,
        _ if usize::try_from(item)
            .ok()
            .and_then(|index| ITEMS.get(index))
            .is_some_and(|info| info.class == ItemClass::SpawnEggItem) =>
        {
            Behaviour::SpawnEgg
        }
        _ => Behaviour::Drop,
    }
}

/// The `minecraft:equippable` of `item`, where it has one.
fn equippable(item: i32) -> Option<clustine_data::Equippable> {
    usize::try_from(item)
        .ok()
        .filter(|index| *index < ITEMS.len())
        .and_then(|_| ItemComponents::of(item).equippable)
}

/// The slot of a player's inventory that the equipment slot `slot` is: the hand held,
/// the off hand, or the armour from the feet up. An animal's body and a saddle are none:
/// every item that goes there is for mobs alone (its `allowed_entities`).
fn player_slot(slot: u8, selected: u8) -> Option<usize> {
    match slot {
        slots::MAIN_HAND => Some(usize::from(selected)),
        slots::OFF_HAND => Some(OFF_HAND),
        slots::FEET | slots::LEGS | slots::CHEST | slots::HEAD => {
            Some(ARMOUR_START + usize::from(slot - slots::FEET))
        }
        _ => None,
    }
}

/// Whether `class` is one of the blocks of copper that weather (`WeatheringCopper`'s
/// classes), which honeycomb waxes.
fn is_weathering(class: BlockClass) -> bool {
    matches!(
        class,
        BlockClass::WeatheringCopperBarsBlock
            | BlockClass::WeatheringCopperBulbBlock
            | BlockClass::WeatheringCopperChainBlock
            | BlockClass::WeatheringCopperChestBlock
            | BlockClass::WeatheringCopperDoorBlock
            | BlockClass::WeatheringCopperFullBlock
            | BlockClass::WeatheringCopperGolemStatueBlock
            | BlockClass::WeatheringCopperGrateBlock
            | BlockClass::WeatheringCopperSlabBlock
            | BlockClass::WeatheringCopperStairBlock
            | BlockClass::WeatheringCopperTrapDoorBlock
            | BlockClass::WeatheringLanternBlock
            | BlockClass::WeatheringLightningRodBlock
    )
}

/// `HoneycombItem.getWaxed(state)`: the waxed block of the copper `state` is, with its
/// properties (**Inferred**: the waxing map is not read; each waxed block is named
/// `waxed_` and the name of the block it waxes).
pub(crate) fn waxed(state: BlockState) -> Option<BlockState> {
    if !is_weathering(state.block_info()?.class) {
        return None;
    }
    let name = state.block()?.name.strip_prefix("minecraft:")?;
    let id = clustine_data::block_id_by_name(&format!("minecraft:waxed_{name}"))?;
    let mut waxed = BLOCKS[id].default_state;
    for (property, value) in state.properties() {
        waxed = waxed.with_property(property, value).unwrap_or(waxed);
    }
    Some(waxed)
}

/// Whether `state` is a hive full of honey (`#beehives`, a `BeehiveBlock` with
/// `honey_level` at least 5).
fn full_hive(state: BlockState) -> bool {
    in_tag(state, "minecraft:beehives")
        && is_class(state, BlockClass::BeehiveBlock)
        && number(state, "honey_level").is_some_and(|level| level >= 5)
}

impl Region {
    /// The behaviour of the dispensed stack's item run, with its click and smoke. What
    /// the slot is left with.
    pub(super) fn run_behaviour(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let stack = dispense.stack()?;
        let behaviour = behaviour_of(stack.item);
        if behaviour.reads_front() && !self.chunks.contains_key(&dispense.front().chunk()) {
            // At a line: the empty dispense's click, the item kept.
            level_event(FAILED, dispense.pos, 0, output);
            return dispense.stack();
        }
        match behaviour {
            Behaviour::Drop => self.default_dispense(dispense, output),
            Behaviour::ArmourStand => {
                if self.dispense_armour_stand(dispense, output) {
                    self.post(Vibration::ENTITY_PLACE, dispense.front());
                }
                self.click_and_smoke(dispense, DISPENSED, output);
                dispense.stack()
            }
            Behaviour::Boat(kind) => self.dispense_boat(dispense, kind, output),
            Behaviour::Minecart(Some(kind)) => self.dispense_minecart(dispense, kind, output),
            Behaviour::Minecart(None) => {
                let left = self.default_dispense(dispense, output);
                self.click_and_smoke(dispense, DISPENSED, output);
                left
            }
            Behaviour::FullBucket(item) => {
                let left = if self.empty_contents(item, dispense.front(), output) {
                    self.post(Vibration::FLUID_PLACE, dispense.front());
                    let bucket = ItemStack {
                        item: items::BUCKET,
                        count: 1,
                    };
                    self.consume_with_remainder(dispense, bucket, output)
                } else {
                    self.default_dispense(dispense, output)
                };
                self.click_and_smoke(dispense, DISPENSED, output);
                left
            }
            Behaviour::EmptyBucket => {
                let left = match self.pickup_block(dispense.front(), output) {
                    Some(filled) => {
                        self.post(Vibration::FLUID_PICKUP, dispense.front());
                        let filled = ItemStack {
                            item: filled,
                            count: 1,
                        };
                        self.consume_with_remainder(dispense, filled, output)
                    }
                    None => self.default_execute(dispense, output),
                };
                self.click_and_smoke(dispense, DISPENSED, output);
                left
            }
            Behaviour::FlintAndSteel => {
                let lit = self.dispense_flint_and_steel(dispense, output);
                self.optional_click(dispense, lit, output)
            }
            Behaviour::Tnt => {
                let primed =
                    self.prime_tnt_posting(dispense.front(), None, Vibration::ENTITY_PLACE, output);
                if primed {
                    dispense.shrink(1);
                }
                self.optional_click(dispense, primed, output)
            }
            Behaviour::Head => {
                let worn = self.dispense_equipment(dispense, output);
                self.optional_click(dispense, worn, output)
            }
            Behaviour::Equipment => {
                let left = if self.dispense_equipment(dispense, output) {
                    dispense.stack()
                } else {
                    self.default_execute(dispense, output)
                };
                self.click_and_smoke(dispense, DISPENSED, output);
                left
            }
            Behaviour::ShulkerBox => {
                let placed = self.dispense_shulker_box(dispense, output);
                if placed {
                    self.post(Vibration::BLOCK_PLACE, dispense.front());
                }
                self.optional_click(dispense, placed, output)
            }
            Behaviour::GlassBottle => self.dispense_glass_bottle(dispense, output),
            Behaviour::Glowstone => self.dispense_glowstone(dispense, output),
            Behaviour::Honeycomb => {
                // `$12` sets its success only to true, so it never clicks a failure.
                let front = dispense.front();
                let left = match self.own_block(front).and_then(waxed) {
                    Some(waxed) => {
                        self.set_block(front, waxed, output);
                        level_event(WAX_ON, front, 0, output);
                        Region::block_sound(front, sounds::ITEM_HONEYCOMB_WAX_ON, 1.0, 1.0, output);
                        dispense.shrink(1);
                        dispense.stack()
                    }
                    None => self.default_execute(dispense, output),
                };
                self.click_and_smoke(dispense, DISPENSED, output);
                left
            }
            Behaviour::Potion => self.dispense_potion(dispense, output),
            Behaviour::Shears => {
                let front = dispense.front();
                let sheared =
                    self.shear_beehive(front, output) || self.shear_leashes(front, output);
                self.optional_click(dispense, sheared, output)
            }
            Behaviour::Brush | Behaviour::BoneMeal => self.optional_click(dispense, false, output),
            Behaviour::SpawnEgg => {
                self.click_and_smoke(dispense, DISPENSED, output);
                dispense.stack()
            }
        }
    }

    /// `OptionalDispenseItemBehavior.playSound` by `success`, and the smoke: what the slot
    /// is left with.
    fn optional_click(
        &mut self,
        dispense: &mut Dispense,
        success: bool,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let click = if success { DISPENSED } else { FAILED };
        self.click_and_smoke(dispense, click, output);
        dispense.stack()
    }

    /// `$1`: an armour stand at the middle of the floor of the block in front, turned the
    /// way the dispenser faces, placed into whatever is there (no check of room). The
    /// level's draw of a yaw that `EntityType.create` makes and the facing turns over is
    /// drawn from the dispenser's stream.
    fn dispense_armour_stand(&mut self, dispense: &mut Dispense, output: &mut TickOutput) -> bool {
        let front = dispense.front();
        let position = [
            f64::from(front.x) + 0.5,
            f64::from(front.y),
            f64::from(front.z) + 0.5,
        ];
        let _yaw = dispense.random.next_float();
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.chunks.contains_key(&chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        // The stand's own draws: its UUID, then `LivingEntity`'s yaw, turned over.
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _yaw = own.next_float();
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::ARMOR_STAND).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion: [0.0; 3],
            yaw: y_rot(dispense.facing),
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
                pose: Box::new(super::super::stands::DEFAULT_POSE.map(|part| part.map(Float))),
                equipment: Vec::new(),
                health: Float(super::super::stands::MAX_HEALTH),
                last_hit: None,
                air: MAX_AIR as i16,
                frozen: 0,
            },
        };
        if here {
            self.entity_draws.insert(key, 3);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        dispense.shrink(1);
        true
    }

    /// `Level.gameEvent(null, event, pos)`: what a dispenser does, heard by the sculk
    /// sensors near (`vibrations.rs`), with no entity as its source.
    fn post(&mut self, event: Vibration, pos: BlockPos) {
        self.vibrate_at(event, pos, super::super::vibrations::Context::default());
    }

    /// `BoatDispenseItemBehavior.execute`: a boat of `kind` a block and a quarter out of
    /// the middle of the dispenser (half its width and 0.5625), a block above that where
    /// the block in front holds water, level with it where that block is air over water,
    /// turned the way the dispenser faces, with no check of room; elsewhere the whole
    /// inner dispense, which drops it. Its own click is always the success's.
    fn dispense_boat(
        &mut self,
        dispense: &mut Dispense,
        kind: u16,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let [dx, dy, dz] = crate::physics::piston::step(dispense.facing);
        let width = EntityTypeInfo::of(i32::from(kind)).width;
        let out = 0.5625 + f64::from(width) / 2.0;
        let centre = block_centre(dispense.pos);
        let x = centre[0] + f64::from(dx) * out;
        let y = centre[1] + f64::from(dy as f32 * 1.125_f32);
        let z = centre[2] + f64::from(dz) * out;
        let front = dispense.front();
        let at_front = self.block(front).unwrap_or(blocks::AIR);
        let below = self
            .block(relative(front, Face::Down))
            .unwrap_or(blocks::AIR);
        let lift = if has_water(at_front) {
            1.0
        } else if at_front.props().is_air() && has_water(below) {
            0.0
        } else {
            let left = self.default_dispense(dispense, output);
            self.click_and_smoke(dispense, DISPENSED, output);
            return left;
        };
        let position = [x, y + lift, z];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.chunks.contains_key(&chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        if let Some(key) = key {
            let mut own = Random::of_entity(self.config.seed, key, self.tick);
            let uuid = insecure_uuid(own.next_u64(), own.next_u64());
            let entity = Entity {
                key,
                kind,
                uuid,
                position,
                motion: [0.0; 3],
                yaw: y_rot(dispense.facing),
                pitch: 0.0,
                on_ground: false,
                hops: 0,
                age: 0,
                fall_distance: 0.0,
                fire_ticks: 0,
                riding: Default::default(),
                data: EntityData::Boat {
                    hurt: Hurt::default(),
                    paddles: [false; 2],
                    bubble_time: 0,
                    out_of_control: Float(0.0),
                    turning: Float(0.0),
                    status: [crate::physics::boat::NO_STATUS; 2],
                    contents: Vec::new(),
                },
            };
            if here {
                self.entity_draws.insert(key, 2);
                self.entity_memory.insert(key, Memory::default());
                self.add_fresh_entity(entity, output);
            } else {
                self.send_made(entity, output);
            }
            dispense.shrink(1);
        }
        self.click_and_smoke(dispense, DISPENSED, output);
        dispense.stack()
    }

    /// `MinecartDispenseItemBehavior.execute`: a cart of `kind` an eighth past the block in
    /// front, at the dispenser's own height plus its step, a tenth up on a rail there (six
    /// tenths on a slope), or nine tenths down onto a rail below an empty block in front
    /// (four tenths onto a slope, unless the dispenser faces down); elsewhere the whole
    /// inner dispense, which drops it. Its own click is always the success's.
    fn dispense_minecart(
        &mut self,
        dispense: &mut Dispense,
        kind: i32,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        use crate::physics::minecart::{Shape, in_rails_tag};
        let shape_of = |state: BlockState| {
            if is_class(state, BlockClass::BaseRailBlock) {
                Shape::of(state).unwrap_or(Shape::NorthSouth)
            } else {
                Shape::NorthSouth
            }
        };
        let [dx, dy, dz] = crate::physics::piston::step(dispense.facing);
        let centre = block_centre(dispense.pos);
        let x = centre[0] + f64::from(dx) * 1.125;
        let y = centre[1].floor() + f64::from(dy);
        let z = centre[2] + f64::from(dz) * 1.125;
        let front = dispense.front();
        let at_front = self.block(front).unwrap_or(blocks::AIR);
        let offset = if in_rails_tag(at_front) {
            if shape_of(at_front).is_slope() {
                0.6
            } else {
                0.1
            }
        } else {
            let below = self
                .block(relative(front, Face::Down))
                .unwrap_or(blocks::AIR);
            if at_front.props().is_air() && in_rails_tag(below) {
                if dispense.facing != Face::Down && shape_of(below).is_slope() {
                    -0.4
                } else {
                    -0.9
                }
            } else {
                let left = self.default_dispense(dispense, output);
                self.click_and_smoke(dispense, DISPENSED, output);
                return left;
            }
        };
        let position = [x, y + offset, z];
        if self.spawn_dispensed_cart(kind, position, output) {
            dispense.shrink(1);
        }
        self.click_and_smoke(dispense, DISPENSED, output);
        dispense.stack()
    }

    /// `AbstractMinecart.createMinecart` and `addFreshEntity` of a cart of `kind` at
    /// `position`, as a player's placing makes one (`minecarts.rs`), facing nowhere in
    /// particular. Into a chunk the region does not have loaded it is sent as an arrival.
    /// Whether it was made.
    fn spawn_dispensed_cart(
        &mut self,
        kind: i32,
        position: [f64; 3],
        output: &mut TickOutput,
    ) -> bool {
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.chunks.contains_key(&chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let entity = Entity {
            key,
            kind: u16::try_from(kind).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Minecart {
                hurt: Hurt::default(),
                display: None,
                display_offset: None,
                flipped: false,
                contents: Vec::new(),
                fuel: 0,
                push: [Double(0.0); 2],
                fuse: -1,
                enabled: true,
            },
        };
        if here {
            self.entity_draws.insert(key, 2);
            self.entity_memory.insert(key, Memory::default());
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        true
    }

    /// `DispensibleContainerItem.emptyContents(null, level, pos, null)` of a full bucket
    /// of `item` into `pos`: water or lava where the block is air, gives way to it or (for
    /// water) takes it in; a block that gives way and is no liquid is broken with its
    /// drops first; water into a block that takes it in waterlogs it. A source of the same
    /// fluid there already takes it too. Powder snow goes only into air. Whether it went.
    fn empty_contents(&mut self, item: i32, pos: BlockPos, output: &mut TickOutput) -> bool {
        let Some(state) = self.own_block(pos) else {
            return false;
        };
        let props = state.props();
        if item == items::POWDER_SNOW_BUCKET {
            if !props.is_air() {
                return false;
            }
            self.set_block(pos, blocks::POWDER_SNOW, output);
            Region::block_sound(pos, sounds::ITEM_BUCKET_EMPTY_POWDER_SNOW, 1.0, 1.0, output);
            return true;
        }
        // The sulfur cube's bucket holds no fluid (its `content` is empty).
        if item == items::SULFUR_CUBE_BUCKET {
            return false;
        }
        let (liquid, fluid) = if item == items::LAVA_BUCKET {
            (blocks::LAVA, clustine_blocks::fluid::LAVA)
        } else {
            (blocks::WATER, clustine_blocks::fluid::WATER)
        };
        let water = fluid == clustine_blocks::fluid::WATER;
        // `canBeReplaced(state, fluid)`: what gives way or is not solid, but for the end's
        // portals, which keep any fluid out.
        let never = is_class(state, BlockClass::EndPortalBlock)
            || is_class(state, BlockClass::EndGatewayBlock);
        let replaceable = !never && (props.can_be_replaced() || !props.is_solid());
        let container = clustine_blocks::fluid::is_container(state);
        let can_place =
            replaceable || (container && clustine_blocks::fluid::can_place_liquid(state, fluid));
        if !(props.is_air() || can_place) {
            return false;
        }
        if container && water {
            self.place_liquid(pos, state, FluidState::source(Kind::Water, false), output);
            Region::block_sound(pos, empty_sound(item), 1.0, 1.0, output);
            return true;
        }
        if replaceable && !props.is_liquid() {
            self.destroy_block(pos, true, UPDATE_LIMIT, output);
        }
        let was_source = FluidState::of(state).is_source();
        if self.set_block_updated(pos, liquid, UPDATE_ALL, UPDATE_LIMIT, output) || was_source {
            Region::block_sound(pos, empty_sound(item), 1.0, 1.0, output);
            return true;
        }
        false
    }

    /// `BucketPickup.pickupBlock(null, level, pos, state)` of the block at `pos`: a source
    /// taken (air left), the water of a waterlogged block (which breaks if it can no
    /// longer stay), a bubble column, or powder snow (with the bits of its breaking). The
    /// filled bucket, if any; the dispenser plays no sound of it.
    fn pickup_block(&mut self, pos: BlockPos, output: &mut TickOutput) -> Option<i32> {
        let state = self.own_block(pos)?;
        if is_class(state, BlockClass::LiquidBlock) {
            if !FluidState::of(state).is_source() {
                return None;
            }
            let filled = if has_lava(state) {
                items::LAVA_BUCKET
            } else {
                items::WATER_BUCKET
            };
            self.set_block_updated(pos, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
            return Some(filled);
        }
        if is_class(state, BlockClass::BubbleColumnBlock) {
            self.set_block_updated(pos, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
            return Some(items::WATER_BUCKET);
        }
        if is_class(state, BlockClass::PowderSnowBlock) {
            self.set_block_updated(pos, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
            output.events.push(RegionEvent::BlockDestroyed {
                position: pos,
                state,
                breaker: None,
            });
            return Some(items::POWDER_SNOW_BUCKET);
        }
        if flag(state, "waterlogged") {
            self.set_block(pos, set_flag(state, "waterlogged", false), output);
            if !can_survive(state, pos, &mut RegionView(self)) {
                self.destroy_block(pos, true, UPDATE_LIMIT, output);
            }
            return Some(items::WATER_BUCKET);
        }
        None
    }

    /// `FlintAndSteelDispenseItemBehavior.execute`: fire in an empty block in front where
    /// it can stay; else an unlit campfire, candle or candle cake lit; else TNT primed and
    /// taken away; else nothing, which still counts as lit. Whether it succeeded: only TNT
    /// that could not be primed fails.
    fn dispense_flint_and_steel(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> bool {
        let front = dispense.front();
        let Some(state) = self.own_block(front) else {
            return false;
        };
        if state.props().is_air() {
            let fire = clustine_blocks::shape::fire_state(&mut RegionView(self), front);
            if can_survive(fire, front, &mut RegionView(self)) {
                self.set_block(front, fire, output);
                self.post(Vibration::BLOCK_PLACE, front);
                return true;
            }
        }
        if super::super::tnt::can_light(state) {
            self.set_block(front, set_flag(state, "lit", true), output);
            self.post(Vibration::BLOCK_CHANGE, front);
            return true;
        }
        if is_class(state, BlockClass::TntBlock) {
            if self.prime_tnt(front, None, output) {
                self.set_block(front, fluid_block(state), output);
                return true;
            }
            return false;
        }
        true
    }

    /// `EquipmentDispenseItemBehavior.dispenseEquipment`: one of the stack put on the
    /// first living entity in the block in front that can wear it there, its slot empty:
    /// an armour stand (not in an animal's body or saddle slot, nor one it has disabled)
    /// or a player. Whether one was.
    fn dispense_equipment(&mut self, dispense: &mut Dispense, output: &mut TickOutput) -> bool {
        let Some(stack) = dispense.stack() else {
            return false;
        };
        let Some(equippable) = equippable(stack.item) else {
            return false;
        };
        let slot = equippable.slot;
        let bounds = Aabb::of_block(dispense.front());
        let stand = self
            .entities_in(&bounds)
            .into_iter()
            .find_map(|(key, chunk)| {
                let record = self.entity_record(key, chunk)?;
                let EntityData::ArmorStand {
                    flags,
                    disabled_slots,
                    equipment,
                    ..
                } = &record.data
                else {
                    return None;
                };
                let free = super::super::stands::can_use_slot(*disabled_slots, *flags, slot)
                    && super::super::stands::worn(equipment, slot).is_none();
                free.then_some(key)
            });
        if let Some(key) = stand {
            let one = dispense.split_one();
            self.set_worn(key, slot, one, output);
            return true;
        }
        let half = super::super::PLAYER_WIDTH / 2.0;
        let player = self.players.iter().find_map(|(id, player)| {
            let feet = player.pose.position;
            let body = Aabb::new(
                [feet.x - half, feet.y, feet.z - half],
                [
                    feet.x + half,
                    feet.y + super::super::PLAYER_HEIGHT,
                    feet.z + half,
                ],
            );
            let at = player_slot(slot, player.selected_slot)?;
            let empty = player.inventory.get(at).flatten().is_none();
            (body.intersects(bounds) && empty).then_some((*id, at, [feet.x, feet.y, feet.z]))
        });
        let Some((id, at, feet)) = player else {
            return false;
        };
        let one = dispense.split_one();
        if let Some(player) = self.players.get_mut(&id) {
            player.inventory.set(at, one);
        }
        self.journal.players.insert(id);
        let at = u8::try_from(at).expect("an inventory slot fits a byte");
        output.player_events.push((
            id,
            PlayerEvent::InventorySlot {
                slot: at,
                stack: one,
            },
        ));
        // `onEquipItem`: the item's sound where it goes in its own slot, heard by all.
        if one.is_some() {
            output.events.push(RegionEvent::Sound {
                sound: equippable.sound,
                source: SOUND_SOURCE_PLAYERS,
                position: clustine_world::Vec3::new(feet[0], feet[1], feet[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        true
    }

    /// `ShulkerBoxDispenseBehavior.execute`: the box placed in the block in front, as
    /// `BlockItem.place` places it from a `DirectionalPlaceContext`, facing the way the
    /// dispenser does, or up where the block below that is not empty; where that block
    /// does not give way or a player stands in it, nothing, and the box stays. Placed, it
    /// plays its place sound to everyone and is used up. Whether it was placed.
    fn dispense_shulker_box(&mut self, dispense: &mut Dispense, output: &mut TickOutput) -> bool {
        let Some(stack) = dispense.stack() else {
            return false;
        };
        let front = dispense.front();
        let below = self
            .block(relative(front, Face::Down))
            .unwrap_or(blocks::AIR);
        let face = if below.props().is_air() {
            dispense.facing
        } else {
            Face::Up
        };
        let Some(target) = self.own_block(front) else {
            return false;
        };
        if !crate::placement::can_be_replaced_empty_handed(target) {
            return false;
        }
        let half = super::super::PLAYER_WIDTH / 2.0;
        let bounds = Aabb::of_block(front);
        let obstructed = self.players.values().any(|player| {
            let feet = player.pose.position;
            Aabb::new(
                [feet.x - half, feet.y, feet.z - half],
                [
                    feet.x + half,
                    feet.y + super::super::PLAYER_HEIGHT,
                    feet.z + half,
                ],
            )
            .intersects(bounds)
        });
        if obstructed {
            return false;
        }
        let Some(block) = usize::try_from(stack.item)
            .ok()
            .and_then(|index| ITEMS.get(index))
            .and_then(|info| info.places)
        else {
            return false;
        };
        let placed = set(
            BLOCKS[usize::from(block)].default_state,
            "facing",
            face_name(face),
        );
        if !self.set_block_updated(front, placed, UPDATE_ALL, UPDATE_LIMIT, output) {
            return false;
        }
        if let Some(sound) = placed.sound_type() {
            output.events.push(RegionEvent::Sound {
                sound: sound.place,
                source: SOUND_SOURCE_BLOCKS,
                position: clustine_world::Vec3::new(
                    f64::from(front.x) + 0.5,
                    f64::from(front.y) + 0.5,
                    f64::from(front.z) + 0.5,
                ),
                volume: (sound.volume + 1.0) / 2.0,
                pitch: sound.pitch * 0.8,
                except: None,
            });
        }
        dispense.shrink(1);
        true
    }

    /// `$9`: a bottle filled with honey from a full hive in front (its honey gone), or with
    /// water from water there (which stays); else dropped, clicking its failure.
    fn dispense_glass_bottle(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let front = dispense.front();
        let state = self.own_block(front).unwrap_or(blocks::AIR);
        let (left, success) = if full_hive(state) {
            // `releaseBeesAndResetHoneyLevel`: the bees are N7's.
            self.set_block(front, set(state, "honey_level", "0"), output);
            // `takeLiquid`: at the dispenser, not in front.
            self.post(Vibration::FLUID_PICKUP, dispense.pos);
            let honey = ItemStack {
                item: items::HONEY_BOTTLE,
                count: 1,
            };
            (self.consume_with_remainder(dispense, honey, output), true)
        } else if has_water(state) {
            self.post(Vibration::FLUID_PICKUP, dispense.pos);
            let water = ItemStack {
                item: items::POTION,
                count: 1,
            };
            (self.consume_with_remainder(dispense, water, output), true)
        } else {
            (self.default_execute(dispense, output), false)
        };
        let click = if success { DISPENSED } else { FAILED };
        self.click_and_smoke(dispense, click, output);
        left
    }

    /// `$10`: an anchor in front charged by one where it is not full, and the glowstone
    /// used; a full one fails; anything else and the glowstone is dropped.
    fn dispense_glowstone(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let front = dispense.front();
        let state = self.own_block(front).unwrap_or(blocks::AIR);
        if !is_block(state, blocks::RESPAWN_ANCHOR) {
            let left = self.default_execute(dispense, output);
            self.click_and_smoke(dispense, DISPENSED, output);
            return left;
        }
        let charge = number(state, "charges").unwrap_or(0);
        if charge == 4 {
            return self.optional_click(dispense, false, output);
        }
        // `RespawnAnchorBlock.charge`: one more, with every update, and its sound.
        self.set_block(
            front,
            set(state, "charges", &(charge + 1).to_string()),
            output,
        );
        Region::block_sound(front, sounds::BLOCK_RESPAWN_ANCHOR_CHARGE, 1.0, 1.0, output);
        dispense.shrink(1);
        self.optional_click(dispense, true, output)
    }

    /// `$13`: a water potion makes the block in front mud where it can become mud, with
    /// five splashes over the dispenser (ten level draws), the sound of a bottle emptied
    /// there, and the bottle left; else the whole inner dispense, which drops it.
    fn dispense_potion(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let front = dispense.front();
        let state = self.own_block(front).unwrap_or(blocks::AIR);
        let left = if in_tag(state, "minecraft:convertible_to_mud") {
            for _ in 0..5 {
                let _x = dispense.random.next_double();
                let _z = dispense.random.next_double();
            }
            Region::block_sound(dispense.pos, sounds::ITEM_BOTTLE_EMPTY, 1.0, 1.0, output);
            self.set_block(front, blocks::MUD, output);
            let bottle = ItemStack {
                item: items::GLASS_BOTTLE,
                count: 1,
            };
            self.consume_with_remainder(dispense, bottle, output)
        } else {
            self.default_dispense(dispense, output)
        };
        self.click_and_smoke(dispense, DISPENSED, output);
        left
    }

    /// `ShearsDispenseItemBehavior.tryShearBeehive`: a full hive in front sheared, with its
    /// sound, three honeycombs and its honey gone. Whether it was.
    fn shear_beehive(&mut self, pos: BlockPos, output: &mut TickOutput) -> bool {
        let Some(state) = self.own_block(pos) else {
            return false;
        };
        if !full_hive(state) {
            return false;
        }
        Region::block_sound(pos, sounds::BLOCK_BEEHIVE_SHEAR, 1.0, 1.0, output);
        let combs = ItemStack {
            item: items::HONEYCOMB,
            count: 3,
        };
        self.pop_resource_at(pos, combs, output);
        self.set_block(pos, set(state, "honey_level", "0"), output);
        true
    }

    /// `ShearsDispenseItemBehavior.tryShearEntity`, of what N6 has: the first entity in the
    /// block in front that is in a leash, or holds one, has every leash cut, each with its
    /// lead (`leashes.rs`). Whether any was.
    fn shear_leashes(&mut self, pos: BlockPos, output: &mut TickOutput) -> bool {
        let found: Vec<EntityId> = self
            .entities_in(&Aabb::of_block(pos))
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        found.into_iter().any(|key| self.cut_leashes(key, output))
    }
}

/// The middle of the block at `pos` (`Vec3.atCenterOf`).
fn block_centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}
