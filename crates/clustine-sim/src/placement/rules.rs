// Adapted from SteelMC's `getStateForPlacement` and `setPlacedBy` of the blocks under
// `steel-core/src/behavior/blocks/`, and its `face_attached_horizontal_directional_block.rs`.

//! The state a block takes as it is placed, by its class (section 4.4), and the other
//! blocks that are set with it: the upper half of a door or a tall plant, the head of
//! a bed.

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, blocks};
use clustine_world::BlockPos;

use super::context::{Context, View};
use super::data;
use super::geometry::{self, DirExt, HORIZONTAL};
use super::replace::{self, can_be_replaced};
use super::survive::{self, can_survive, facing};
use clustine_blocks::connect;

/// How a block is placed, by its class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    /// `axis` along the clicked face: logs, pillars, chains.
    Axis,
    /// `facing` the opposite of the player's facing: furnaces and the like.
    FacingAway,
    /// As [`Rule::FacingAway`], or turned to pair with a chest beside it as half of a
    /// double chest.
    Chest,
    /// As [`Rule::FacingAway`], and a repeater locked by a powered diode beside it.
    Diode,
    Campfire,
    Anvil,
    /// `facing` the opposite of the nearest looking direction: barrels, dispensers,
    /// droppers, pistons.
    AwayFromLook,
    /// `facing` the nearest looking direction: observers.
    TowardsLook,
    /// `facing` into the clicked block from the side, down from a top or a bottom:
    /// hoppers (`HopperBlock.getStateForPlacement`).
    Hopper,
    EndRod,
    LightningRod,
    Stairs,
    Slab,
    Door,
    TrapDoor,
    FenceGate,
    Fence,
    Pane,
    Wall,
    RedstoneWire,
    Rail,
    /// A redstone lamp, lit where power is beside it.
    Lamp,
    Bed,
    FaceAttached,
    Ladder,
    Lantern,
    DoublePlant,
    SmallDripleaf,
    BigDripleaf,
    SnowLayer,
    /// One more candle, pickle or egg where the block holds fewer than four.
    Additive,
    /// One more flower or leaf where a flower bed or leaf litter holds fewer than four.
    Segmented,
    Multiface,
    Scaffolding,
    Leaves,
    /// The wall form of a torch, sign, banner or coral fan: against the first wall
    /// along the horizontal looking directions where it can stay.
    OnWall,
    /// A standing sign or banner, turned towards the player.
    StandingRotation,
    Skull,
    WallSkull,
    CeilingHangingSign,
    WallHangingSign,
    MangrovePropagule,
    /// Seagrass, which goes only into water.
    Seagrass,
    /// Bamboo: a shoot on the ground, a stalk on bamboo or under it, nothing in a fluid.
    Bamboo,
    /// A note block, with the instrument of the blocks above and below it.
    NoteBlock,
    /// Tripwire, joined to the tripwire and the hooks beside it.
    TripWire,
    /// A bell, on the floor, under a ceiling, or on one wall or between two.
    Bell,
    /// The block's default state.
    Default,
}

/// The rule of the nearest class that has one.
pub(crate) fn rule_for(state: BlockState) -> Rule {
    data::nearest(data::info(state), class_rule).unwrap_or(Rule::Default)
}

fn class_rule(class: BlockClass) -> Option<Rule> {
    use BlockClass as C;
    Some(match class {
        C::RotatedPillarBlock | C::InfestedRotatedPillarBlock => Rule::Axis,
        C::AbstractFurnaceBlock
        | C::CarvedPumpkinBlock
        | C::BeehiveBlock
        | C::LoomBlock
        | C::StonecutterBlock
        | C::LecternBlock
        | C::GlazedTerracottaBlock
        | C::EnderChestBlock
        | C::ChiseledBookShelfBlock
        | C::EndPortalFrameBlock => Rule::FacingAway,
        C::ChestBlock => Rule::Chest,
        C::DiodeBlock => Rule::Diode,
        C::CampfireBlock => Rule::Campfire,
        C::AnvilBlock => Rule::Anvil,
        C::BarrelBlock | C::DispenserBlock | C::PistonBaseBlock => Rule::AwayFromLook,
        C::ObserverBlock => Rule::TowardsLook,
        C::HopperBlock => Rule::Hopper,
        C::EndRodBlock => Rule::EndRod,
        C::LightningRodBlock => Rule::LightningRod,
        C::StairBlock => Rule::Stairs,
        C::SlabBlock => Rule::Slab,
        C::DoorBlock => Rule::Door,
        C::TrapDoorBlock => Rule::TrapDoor,
        C::FenceGateBlock => Rule::FenceGate,
        C::FenceBlock => Rule::Fence,
        C::IronBarsBlock => Rule::Pane,
        C::WallBlock => Rule::Wall,
        C::RedstoneWireBlock => Rule::RedstoneWire,
        C::BaseRailBlock => Rule::Rail,
        C::RedstoneLampBlock => Rule::Lamp,
        C::AbstractBedBlock => Rule::Bed,
        C::FaceAttachedHorizontalDirectionalBlock => Rule::FaceAttached,
        C::LadderBlock => Rule::Ladder,
        C::LanternBlock => Rule::Lantern,
        // A pitcher crop is one block when placed, though made from a tall plant.
        C::PitcherCropBlock => Rule::Default,
        C::SmallDripleafBlock => Rule::SmallDripleaf,
        C::DoublePlantBlock => Rule::DoublePlant,
        C::BigDripleafBlock => Rule::BigDripleaf,
        C::SnowLayerBlock => Rule::SnowLayer,
        C::CandleBlock | C::SeaPickleBlock | C::TurtleEggBlock => Rule::Additive,
        C::FlowerBedBlock | C::LeafLitterBlock => Rule::Segmented,
        C::MultifaceBlock => Rule::Multiface,
        C::ScaffoldingBlock => Rule::Scaffolding,
        C::LeavesBlock => Rule::Leaves,
        C::WallTorchBlock
        | C::RedstoneWallTorchBlock
        | C::WallSignBlock
        | C::WallBannerBlock
        | C::BaseCoralWallFanBlock
        | C::TripWireHookBlock => Rule::OnWall,
        C::NoteBlock => Rule::NoteBlock,
        C::TripWireBlock => Rule::TripWire,
        C::BellBlock => Rule::Bell,
        C::StandingSignBlock | C::BannerBlock => Rule::StandingRotation,
        C::SkullBlock => Rule::Skull,
        C::WallSkullBlock => Rule::WallSkull,
        C::CeilingHangingSignBlock => Rule::CeilingHangingSign,
        C::WallHangingSignBlock => Rule::WallHangingSign,
        C::MangrovePropaguleBlock => Rule::MangrovePropagule,
        C::SeagrassBlock => Rule::Seagrass,
        C::BambooStalkBlock => Rule::Bamboo,
        _ => return None,
    })
}

/// The state the block `block` (its default state) takes at the context's target, or
/// `None` where its rule refuses. Survival and players in the way are judged after.
pub(crate) fn state_for_placement(block: BlockState, context: &Context) -> Option<BlockState> {
    // Concrete powder placed into or beside water is concrete at once
    // (`ConcretePowderBlock.getStateForPlacement`).
    if let Some(concrete) = clustine_blocks::fluid::concrete_of(block) {
        let replaced = context.world.get(context.target);
        let view = &mut super::context::View(&context.world);
        if clustine_blocks::fluid::powder_hardens(view, context.target, replaced) {
            return Some(concrete);
        }
    }
    let rule = rule_for(block);
    let state = rule_state(rule, block, context)?;
    // Every rule keeps the water of a source it is placed into (section 4.4), except a
    // slab that becomes double.
    let double_slab = rule == Rule::Slab && state.property("type") == Some("double");
    if double_slab || state.property("waterlogged").is_none() {
        return Some(state);
    }
    let waterlogged = data::set_flag(state, "waterlogged", context.water_source_at_target());
    if rule == Rule::Campfire {
        return Some(data::set_flag(
            waterlogged,
            "lit",
            !context.water_source_at_target(),
        ));
    }
    Some(waterlogged)
}

fn rule_state(rule: Rule, block: BlockState, context: &Context) -> Option<BlockState> {
    let world = &context.world;
    let target = context.target;
    let at_target = world.get(target);
    let horizontal = context.horizontal();
    let face = context.face;
    let set = data::set;
    Some(match rule {
        Rule::Default => block,
        Rule::Axis => set(block, "axis", face.axis().name()),
        Rule::FacingAway => set(block, "facing", horizontal.opposite().name()),
        Rule::Chest => chest(block, context),
        Rule::Diode => {
            let state = set(block, "facing", horizontal.opposite().name());
            if state.property("locked").is_some() {
                data::set_flag(state, "locked", repeater_locked(state, context))
            } else {
                state
            }
        }
        Rule::Campfire => {
            let below = world.get(Dir::Down.relative(target));
            let state = set(block, "facing", horizontal.name());
            data::set_flag(
                state,
                "signal_fire",
                data::same_block(below, blocks::HAY_BLOCK),
            )
        }
        Rule::Anvil => set(block, "facing", horizontal.clockwise().name()),
        Rule::AwayFromLook => set(block, "facing", context.nearest_looking().opposite().name()),
        Rule::TowardsLook => set(block, "facing", context.nearest_looking().name()),
        Rule::Hopper => {
            let into = face.opposite();
            let facing = if matches!(into, Dir::Up | Dir::Down) {
                Dir::Down
            } else {
                into
            };
            data::set_flag(set(block, "facing", facing.name()), "enabled", true)
        }
        Rule::EndRod => {
            let against = world.get(face.opposite().relative(target));
            let flipped = data::same_block(against, block) && facing(against) == Some(face);
            let facing = if flipped { face.opposite() } else { face };
            set(block, "facing", facing.name())
        }
        Rule::LightningRod => set(block, "facing", face.name()),
        Rule::Stairs => {
            let top = face == Dir::Down || face != Dir::Up && context.cursor_height() > 0.5;
            let state = set(block, "facing", horizontal.name());
            let state = set(state, "half", if top { "top" } else { "bottom" });
            connect::stairs(state, target, &mut View(world))
        }
        Rule::Slab => {
            if data::same_block(at_target, block) {
                return Some(data::set_flag(
                    set(at_target, "type", "double"),
                    "waterlogged",
                    false,
                ));
            }
            let top = face == Dir::Down || face != Dir::Up && context.cursor_height() > 0.5;
            set(block, "type", if top { "top" } else { "bottom" })
        }
        Rule::Door => {
            let above = world.known(Dir::Up.relative(target))?;
            if !can_be_replaced(above, context) {
                return None;
            }
            let state = set(block, "facing", horizontal.name());
            let state = set(state, "hinge", door_hinge(context));
            let state = set(state, "half", "lower");
            // Powered beside either half, it is placed open.
            let powered = has_neighbour_signal(context, target)
                || has_neighbour_signal(context, Dir::Up.relative(target));
            let state = data::set_flag(state, "open", powered);
            data::set_flag(state, "powered", powered)
        }
        Rule::TrapDoor => {
            let state = if !context.replace_clicked && face.is_horizontal() {
                let top = context.cursor_height() > 0.5;
                set(
                    set(block, "facing", face.name()),
                    "half",
                    if top { "top" } else { "bottom" },
                )
            } else {
                set(
                    set(block, "facing", horizontal.opposite().name()),
                    "half",
                    if face == Dir::Up { "bottom" } else { "top" },
                )
            };
            let powered = has_neighbour_signal(context, target);
            data::set_flag(data::set_flag(state, "open", powered), "powered", powered)
        }
        Rule::FenceGate => {
            let in_wall = connect::gate_in_wall(target, horizontal, &mut View(world));
            let powered = has_neighbour_signal(context, target);
            let state = set(block, "facing", horizontal.name());
            let state = data::set_flag(state, "open", powered);
            let state = data::set_flag(state, "powered", powered);
            data::set_flag(state, "in_wall", in_wall)
        }
        Rule::Fence => connect::fence(block, target, &mut View(world)),
        Rule::Pane => connect::pane(block, target, &mut View(world)),
        Rule::Wall => connect::wall(block, target, &mut View(world)),
        Rule::RedstoneWire => connect::redstone_wire(block, target, &mut View(world)),
        Rule::Rail => {
            let shape = if horizontal.axis() == geometry::Axis::X {
                "east_west"
            } else {
                "north_south"
            };
            set(block, "shape", shape)
        }
        Rule::Lamp => data::set_flag(block, "lit", has_neighbour_signal(context, target)),
        Rule::Bed => {
            let head = world.known(horizontal.relative(target))?;
            if !can_be_replaced(head, context) {
                return None;
            }
            set(set(block, "facing", horizontal.name()), "part", "foot")
        }
        Rule::FaceAttached => {
            return context
                .nearest_looking_directions()
                .into_iter()
                .find_map(|dir| {
                    let state = match dir {
                        Dir::Up => set(set(block, "face", "ceiling"), "facing", horizontal.name()),
                        Dir::Down => set(set(block, "face", "floor"), "facing", horizontal.name()),
                        side => set(set(block, "face", "wall"), "facing", side.opposite().name()),
                    };
                    can_survive(state, target, world).then_some(state)
                });
        }
        Rule::Ladder => {
            if !context.replace_clicked {
                let against = world.get(face.opposite().relative(target));
                if data::same_block(against, block) && facing(against) == Some(face) {
                    return None;
                }
            }
            return on_wall(block, context);
        }
        Rule::OnWall => return on_wall(block, context),
        Rule::NoteBlock => clustine_blocks::note::with_instrument(&mut View(world), target, block),
        Rule::TripWire => HORIZONTAL_ORDER.into_iter().fold(block, |state, dir| {
            let beside = world.get(dir.relative(target));
            let joins = clustine_blocks::shape::tripwire_connects_to(beside, dir);
            data::set_flag(state, dir.name(), joins)
        }),
        Rule::Bell => return bell(block, context),
        Rule::Lantern => {
            return context
                .nearest_looking_directions()
                .into_iter()
                .filter(|dir| !dir.is_horizontal())
                .map(|dir| data::set_flag(block, "hanging", dir == Dir::Up))
                .find(|&state| can_survive(state, target, world));
        }
        Rule::DoublePlant => {
            let above = world.known(Dir::Up.relative(target))?;
            if !can_be_replaced(above, context) {
                return None;
            }
            set(block, "half", "lower")
        }
        Rule::SmallDripleaf => {
            let above = world.known(Dir::Up.relative(target))?;
            if !can_be_replaced(above, context) {
                return None;
            }
            set(
                set(block, "half", "lower"),
                "facing",
                horizontal.opposite().name(),
            )
        }
        Rule::BigDripleaf => {
            let below = world.get(Dir::Down.relative(target));
            let below_is_dripleaf = data::same_block(below, blocks::BIG_DRIPLEAF)
                || data::same_block(below, blocks::BIG_DRIPLEAF_STEM);
            let facing = match facing(below) {
                Some(theirs) if below_is_dripleaf => theirs,
                _ => horizontal.opposite(),
            };
            set(block, "facing", facing.name())
        }
        Rule::SnowLayer => {
            if data::same_block(at_target, block) {
                let layers = data::number(at_target, "layers").unwrap_or(1);
                data::set_number(at_target, "layers", (layers + 1).min(8))
            } else {
                block
            }
        }
        Rule::Additive | Rule::Segmented => {
            if data::same_block(at_target, block) {
                let Some((name, most)) = replace::additive_count(at_target) else {
                    return Some(at_target);
                };
                let count = data::number(at_target, name).unwrap_or(most);
                data::set_number(at_target, name, (count + 1).min(most))
            } else if rule == Rule::Segmented {
                set(block, "facing", horizontal.opposite().name())
            } else {
                block
            }
        }
        Rule::Multiface => {
            let own = data::same_block(at_target, block);
            return context
                .nearest_looking_directions()
                .into_iter()
                .find_map(|dir| {
                    if own && data::flag(at_target, dir.name()) {
                        return None;
                    }
                    if !survive::multiface_can_attach(world.get(dir.relative(target)), dir) {
                        return None;
                    }
                    let base = if own { at_target } else { block };
                    Some(data::set_flag(base, dir.name(), true))
                });
        }
        Rule::Scaffolding => {
            let distance = survive::scaffolding_distance(target, world);
            let below = world.get(Dir::Down.relative(target));
            let bottom = distance > 0 && !data::same_block(below, block);
            data::set_flag(
                data::set_number(block, "distance", distance),
                "bottom",
                bottom,
            )
        }
        Rule::Leaves => {
            let distance = HORIZONTAL
                .into_iter()
                .chain([Dir::Up, Dir::Down])
                .map(|dir| leaf_distance(world.get(dir.relative(target))).saturating_add(1))
                .min()
                .unwrap_or(7)
                .min(7);
            data::set_number(
                data::set_flag(block, "persistent", true),
                "distance",
                distance,
            )
        }
        Rule::StandingRotation => data::set_number(
            block,
            "rotation",
            geometry::rotation_segment(context.yaw + 180.0),
        ),
        Rule::Skull => data::set_number(block, "rotation", geometry::rotation_segment(context.yaw)),
        Rule::WallSkull => {
            return context
                .nearest_looking_directions()
                .into_iter()
                .filter(|dir| dir.is_horizontal())
                .find(|dir| !can_be_replaced(world.get(dir.relative(target)), context))
                .map(|dir| set(block, "facing", dir.opposite().name()));
        }
        Rule::CeilingHangingSign => ceiling_hanging_sign(block, context),
        Rule::WallHangingSign => {
            return context
                .nearest_looking_directions()
                .into_iter()
                .filter(|dir| dir.is_horizontal() && dir.axis() != face.axis())
                .map(|dir| set(block, "facing", dir.opposite().name()))
                .find(|&state| {
                    can_survive(state, target, world) && wall_hanging_sign_holds(state, context)
                });
        }
        Rule::MangrovePropagule => data::set_number(block, "age", 4),
        Rule::Seagrass => {
            let full_water = data::has_water(at_target)
                && at_target
                    .props()
                    .fluid()
                    .is_some_and(|fluid| fluid.amount == 8);
            if !full_water {
                return None;
            }
            block
        }
        Rule::Bamboo => {
            if at_target.props().fluid().is_some() {
                return None;
            }
            let below = world.get(Dir::Down.relative(target));
            if !data::in_tag(below, "minecraft:supports_bamboo") {
                return None;
            }
            let above = world.get(Dir::Up.relative(target));
            let age = |of: BlockState| data::number(of, "age").unwrap_or(0);
            if data::same_block(below, blocks::BAMBOO_SAPLING) {
                data::set_number(block, "age", 0)
            } else if data::same_block(below, blocks::BAMBOO) {
                data::set_number(block, "age", age(below))
            } else if data::same_block(above, blocks::BAMBOO) {
                data::set_number(block, "age", age(above))
            } else {
                blocks::BAMBOO_SAPLING
            }
        }
    })
}

/// `Level.hasNeighborSignal` of the blocks a placement reads: whether a neighbour of
/// `pos` gives it power, read before the block is placed. A placement that reads it
/// is made only where its chunk ticks, where all it reads is there
/// (`docs/adr/0026-redstone.md`, section 1).
fn has_neighbour_signal(context: &Context, pos: BlockPos) -> bool {
    clustine_blocks::signal::has_neighbour_signal(&mut View(&context.world), pos)
}

/// The wall form of a block: `facing` away from the first horizontal looking direction
/// where it can stay.
/// The sides tripwire looks to as it is placed, in the game's order.
const HORIZONTAL_ORDER: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

/// `BellBlock.getStateForPlacement`: on the floor or under the ceiling clicked, facing
/// the player's way; on a clicked wall, facing it, between two walls where the side
/// across holds it too, and else on the floor or under the ceiling.
fn bell(block: BlockState, context: &Context) -> Option<BlockState> {
    let world = &context.world;
    let target = context.target;
    let face = context.face;
    let set = data::set;
    let sturdy = |dir: Dir, towards: Dir| {
        clustine_blocks::sturdy(
            world.get(dir.relative(target)),
            towards,
            clustine_data::SupportType::Full,
        )
    };
    if !face.is_horizontal() {
        let attachment = if face == Dir::Down {
            "ceiling"
        } else {
            "floor"
        };
        let state = set(
            set(block, "attachment", attachment),
            "facing",
            context.horizontal().name(),
        );
        return can_survive(state, target, world).then_some(state);
    }
    let double = match face.axis() {
        geometry::Axis::X => sturdy(Dir::West, Dir::East) && sturdy(Dir::East, Dir::West),
        _ => sturdy(Dir::North, Dir::South) && sturdy(Dir::South, Dir::North),
    };
    let attachment = if double { "double_wall" } else { "single_wall" };
    let state = set(
        set(block, "facing", face.opposite().name()),
        "attachment",
        attachment,
    );
    if can_survive(state, target, world) {
        return Some(state);
    }
    let on_floor = sturdy(Dir::Down, Dir::Up);
    let state = set(
        state,
        "attachment",
        if on_floor { "floor" } else { "ceiling" },
    );
    can_survive(state, target, world).then_some(state)
}

fn on_wall(block: BlockState, context: &Context) -> Option<BlockState> {
    context
        .nearest_looking_directions()
        .into_iter()
        .filter(|dir| dir.is_horizontal())
        .map(|dir| data::set(block, "facing", dir.opposite().name()))
        .find(|&state| can_survive(state, context.target, &context.world))
}

/// Vanilla's `DoorBlock.getHinge`: the hinge goes against the more solid side, beside a
/// door to form a pair, and otherwise to the side of the door the click was on.
fn door_hinge(context: &Context) -> &'static str {
    let world = &context.world;
    let pos = context.target;
    let above = Dir::Up.relative(pos);
    let facing = context.horizontal();
    let left = facing.counter_clockwise();
    let right = facing.clockwise();
    let full = |at: BlockPos| i32::from(world.get(at).props().collision_is_full_block());
    let balance = full(right.relative(pos)) + full(right.relative(above))
        - full(left.relative(pos))
        - full(left.relative(above));
    let lower_door = |at: BlockPos| {
        let state = world.get(at);
        data::is(state, BlockClass::DoorBlock) && state.property("half") == Some("lower")
    };
    let door_left = lower_door(left.relative(pos));
    let door_right = lower_door(right.relative(pos));
    if (door_left && !door_right) || balance > 0 {
        return "right";
    }
    if (door_right && !door_left) || balance < 0 {
        return "left";
    }
    let [step_x, _, step_z] = facing.step();
    let (click_x, click_z) = context.cursor_across();
    let left_side = (step_x >= 0 || click_z >= 0.5)
        && (step_x <= 0 || click_z <= 0.5)
        && (step_z >= 0 || click_x <= 0.5)
        && (step_z <= 0 || click_x >= 0.5);
    if left_side { "left" } else { "right" }
}

/// `ChestBlock.getStateForPlacement`: a chest faces the player and pairs with a single
/// chest beside it that faces the same way, the one on its left first (it becomes
/// that chest's right half). Sneaking against the side of a chest pairs with that chest
/// whichever way it faces, unless it faces towards or away from the click; sneaking
/// otherwise keeps it single. A copper chest that pairs becomes the least weathered of
/// the two (`CopperChestBlock.getStateForPlacement`). The other chest joins it as the
/// placing updates its neighbours (`growth.rs`, `ChestBlock.updateShape`).
fn chest(block: BlockState, context: &Context) -> BlockState {
    let world = &context.world;
    let target = context.target;
    let face = context.face;
    let mut facing = context.horizontal().opposite();
    let mut kind = "single";
    if face.is_horizontal() && context.sneaking {
        let partner = chest_partner_facing(block, world, target, face.opposite());
        if let Some(partner) = partner
            && partner.axis() != face.axis()
        {
            facing = partner;
            kind = if partner.counter_clockwise() == face.opposite() {
                "right"
            } else {
                "left"
            };
        }
    }
    if kind == "single" && !context.sneaking {
        // `ChestBlock.getChestType`.
        if chest_partner_facing(block, world, target, facing.clockwise()) == Some(facing) {
            kind = "left";
        } else if chest_partner_facing(block, world, target, facing.counter_clockwise())
            == Some(facing)
        {
            kind = "right";
        }
    }
    let state = data::set(data::set(block, "facing", facing.name()), "type", kind);
    let partner = clustine_blocks::growth::chest_connected_direction(state)
        .map_or(blocks::AIR, |towards| world.get(towards.relative(target)));
    clustine_blocks::growth::least_weathered_chest(state, partner)
}

/// `ChestBlock.candidatePartnerFacing`: the facing of the chest towards `towards`, if
/// it is single and one `block` pairs with. A chest in a chunk the placing region does
/// not hold is none: two chests across a line stay single, as a sneaking player places
/// one (`docs/groundwork/n6-plan.md`, section 10 item 2).
fn chest_partner_facing(
    block: BlockState,
    world: &super::context::World,
    target: BlockPos,
    towards: Dir,
) -> Option<Dir> {
    let at = towards.relative(target);
    let other = world.get(at);
    if world.holds(at)
        && clustine_blocks::growth::chest_can_connect_to(block, other)
        && other.property("type") == Some("single")
    {
        facing(other)
    } else {
        None
    }
}

/// Whether a repeater with this state is locked by a powered repeater or comparator
/// beside it that points into its side.
fn repeater_locked(state: BlockState, context: &Context) -> bool {
    let Some(own) = facing(state) else {
        return false;
    };
    [own.clockwise(), own.counter_clockwise()]
        .into_iter()
        .any(|side| {
            let beside = context.world.get(side.relative(context.target));
            data::is(beside, BlockClass::DiodeBlock)
                && facing(beside) == Some(side)
                && data::flag(beside, "powered")
        })
}

/// How far from a log a neighbour of new leaves is: 0 for a log, a leaf's distance,
/// and the most for anything else.
fn leaf_distance(state: BlockState) -> u8 {
    if data::in_tag(state, "minecraft:logs") {
        0
    } else if data::is(state, BlockClass::LeavesBlock) {
        data::number(state, "distance").unwrap_or(7)
    } else {
        7
    }
}

/// Vanilla's `CeilingHangingSignBlock.getStateForPlacement`: hung by its middle under
/// a block without a full bottom or while sneaking, turned towards the player; else by
/// its chains, square to the player, and in line under another hanging sign.
fn ceiling_hanging_sign(block: BlockState, context: &Context) -> BlockState {
    let above = context.world.get(Dir::Up.relative(context.target));
    let facing_dir = context.horizontal();
    let mut middle =
        !survive::sturdy(above, Dir::Down, clustine_data::SupportType::Full) || context.sneaking;
    if data::in_tag(above, "minecraft:all_hanging_signs") && !context.sneaking {
        let theirs = facing(above)
            .or_else(|| data::number(above, "rotation").and_then(geometry::segment_direction));
        if theirs.is_some_and(|theirs| theirs.axis() == facing_dir.axis()) {
            middle = false;
        }
    }
    let rotation = if middle {
        geometry::rotation_segment(context.yaw + 180.0)
    } else {
        geometry::rotation_segment(geometry::y_rot(facing_dir.opposite()))
    };
    data::set_flag(
        data::set_number(block, "rotation", rotation),
        "attached",
        middle,
    )
}

/// Vanilla's `WallHangingSignBlock.canPlace`: a wall hanging sign hangs from a sturdy
/// side, or from another wall hanging sign in line, on its left or its right.
pub(crate) fn wall_hanging_sign_holds(state: BlockState, context: &Context) -> bool {
    let Some(own) = facing(state) else {
        return false;
    };
    let attaches = |side: Dir| {
        let at = context.world.get(side.relative(context.target));
        if data::in_tag(at, "minecraft:wall_hanging_signs") {
            facing(at).is_some_and(|theirs| theirs.axis() == own.axis())
        } else {
            survive::sturdy(at, side.opposite(), clustine_data::SupportType::Full)
        }
    };
    attaches(own.clockwise()) || attaches(own.counter_clockwise())
}

/// The other blocks set with `state` at `T`: the upper half of a door or a tall plant,
/// the head of a bed. `None` if one of them is in a chunk the caller cannot see.
pub(crate) fn more_blocks(
    state: BlockState,
    context: &Context,
) -> Option<Vec<(BlockPos, BlockState)>> {
    let target = context.target;
    Some(match rule_for(state) {
        Rule::Door => vec![(Dir::Up.relative(target), data::set(state, "half", "upper"))],
        Rule::DoublePlant | Rule::SmallDripleaf => {
            let above = Dir::Up.relative(target);
            let water = data::has_water(context.world.known(above)?);
            let upper = data::set(state, "half", "upper");
            let upper = if upper.property("waterlogged").is_some() {
                data::set_flag(upper, "waterlogged", water)
            } else {
                upper
            };
            vec![(above, upper)]
        }
        Rule::Bed => {
            let towards = facing(state)?;
            vec![(towards.relative(target), data::set(state, "part", "head"))]
        }
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::BLOCKS;

    fn rule(name: &str) -> Rule {
        rule_for(BlockState::parse(name).unwrap())
    }

    #[test]
    fn the_nearer_class_wins_where_two_rules_could_match() {
        // A pitcher crop is made from a tall plant and places one block.
        assert_eq!(rule("minecraft:pitcher_crop"), Rule::Default);
        assert_eq!(rule("minecraft:sunflower"), Rule::DoublePlant);
        assert_eq!(rule("minecraft:small_dripleaf"), Rule::SmallDripleaf);
        // A wall torch is made from a torch.
        assert_eq!(rule("minecraft:wall_torch"), Rule::OnWall);
        assert_eq!(rule("minecraft:torch"), Rule::Default);
        // A trapped chest is a chest, a dropper a dispenser, a chain a pillar.
        assert_eq!(rule("minecraft:trapped_chest"), Rule::Chest);
        assert_eq!(rule("minecraft:dropper"), Rule::AwayFromLook);
        assert_eq!(rule("minecraft:iron_chain"), Rule::Axis);
        assert_eq!(rule("minecraft:copper_door"), Rule::Door);
        assert_eq!(rule("minecraft:red_stained_glass_pane"), Rule::Pane);
        assert_eq!(
            rule("minecraft:oak_wall_hanging_sign"),
            Rule::WallHangingSign
        );
        assert_eq!(rule("minecraft:dead_tube_coral_wall_fan"), Rule::OnWall);
        assert_eq!(rule("minecraft:tripwire_hook"), Rule::OnWall);
        assert_eq!(rule("minecraft:note_block"), Rule::NoteBlock);
        assert_eq!(rule("minecraft:tripwire"), Rule::TripWire);
        assert_eq!(rule("minecraft:bell"), Rule::Bell);
    }

    #[test]
    fn every_block_with_a_rule_has_the_properties_the_rule_sets() {
        // A rule sets a property only where the block has it; this guards against a
        // class whose blocks lack what its rule would set, which would leave them in
        // their default state.
        for block in &BLOCKS {
            let state = block.default_state;
            let needs: &[&str] = match rule_for(state) {
                Rule::Axis => &["axis"],
                Rule::FacingAway
                | Rule::Chest
                | Rule::Diode
                | Rule::Campfire
                | Rule::Anvil
                | Rule::AwayFromLook
                | Rule::TowardsLook
                | Rule::EndRod
                | Rule::LightningRod
                | Rule::OnWall
                | Rule::WallSkull
                | Rule::WallHangingSign
                | Rule::Ladder
                | Rule::BigDripleaf => &["facing"],
                Rule::Stairs => &["facing", "half", "shape"],
                Rule::Slab => &["type"],
                Rule::Door => &["facing", "hinge", "half", "open"],
                Rule::TrapDoor => &["facing", "half", "open"],
                Rule::FenceGate => &["facing", "open", "in_wall"],
                Rule::Fence | Rule::Pane => &["north", "east", "south", "west"],
                Rule::Wall => &["north", "east", "south", "west", "up"],
                Rule::RedstoneWire => &["north", "east", "south", "west"],
                Rule::Rail => &["shape"],
                Rule::Lamp => &["lit"],
                Rule::Bed => &["facing", "part"],
                Rule::FaceAttached => &["face", "facing"],
                Rule::Lantern => &["hanging"],
                Rule::DoublePlant => &["half"],
                Rule::SmallDripleaf => &["half", "facing"],
                Rule::SnowLayer => &["layers"],
                Rule::Segmented => &["facing"],
                Rule::Scaffolding => &["distance", "bottom"],
                Rule::Leaves => &["distance", "persistent"],
                Rule::StandingRotation | Rule::Skull | Rule::CeilingHangingSign => &["rotation"],
                Rule::MangrovePropagule => &["age"],
                Rule::Bamboo => &["age"],
                Rule::NoteBlock => &["instrument"],
                Rule::TripWire => &["north", "east", "south", "west"],
                Rule::Bell => &["facing", "attachment"],
                Rule::Hopper => &["facing", "enabled"],
                Rule::Additive | Rule::Multiface | Rule::Seagrass | Rule::Default => &[],
            };
            for property in needs {
                assert!(
                    state.property(property).is_some(),
                    "{} has no {property}",
                    block.name
                );
            }
        }
    }
}
