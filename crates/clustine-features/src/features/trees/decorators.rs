// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/decorators.rs`.

//! The tree decorators (`TreeDecorator` and its kinds): what a tree or a fallen tree
//! adds once its logs and leaves are placed, from vines and cocoa to beehives with
//! their bees and the pale oak's creaking heart.

use std::collections::BTreeSet;

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;
use clustine_worldgen_data::feature as data;

use super::block_entities;
use super::common::{
    HORIZONTAL, LOGS, clockwise, is_air, is_block, random_horizontal, shuffle, vine,
};
use crate::blocks::{face_name, read_face, relative, relative_by};
use crate::context::{Context, Heightmap};
use crate::feature::FeatureRef;
use crate::java_set::JavaSet;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::random_between_inclusive;

/// The beehive's side that faces out, and the sides of a log it may hang on.
const HIVE_FACING: Face = Face::South;
const HIVE_SIDES: [Face; 3] = [Face::East, Face::South, Face::West];

#[derive(Debug)]
pub enum Decorator {
    AlterGround(Provider),
    Beehive(f32),
    Cocoa(f32),
    CreakingHeart(f32),
    LeaveVine(f32),
    TrunkVine,
    AttachedToLeaves {
        probability: f32,
        exclusion_radius_xz: i32,
        exclusion_radius_y: i32,
        required_empty_blocks: i32,
        provider: Provider,
        directions: Vec<Face>,
    },
    AttachedToLogs {
        probability: f32,
        provider: Provider,
        directions: Vec<Face>,
    },
    PlaceOnGround {
        tries: i32,
        radius: i32,
        height: i32,
        provider: Provider,
    },
    PaleMoss {
        leaves_probability: f32,
        trunk_probability: f32,
        ground_probability: f32,
        /// `minecraft:pale_moss_patch`, which the decorator places on the ground.
        patch: FeatureRef,
    },
    ShelfMushroom(f32),
}

fn read_directions(reader: &Reader) -> Read<Vec<Face>> {
    let directions: Vec<Face> = reader.list()?.iter().map(read_face).collect::<Read<_>>()?;
    if directions.is_empty() {
        return Err(reader.error("no directions"));
    }
    Ok(directions)
}

impl Decorator {
    pub fn read(reader: &Reader) -> Read<Self> {
        let float = |key: &str| reader.field(key)?.float();
        let int_or = |key: &str, default: i32| -> Read<i32> {
            match reader.optional(key) {
                Some(value) => value.int(),
                None => Ok(default),
            }
        };
        let decorator = match reader.kind()? {
            "minecraft:alter_ground" => {
                Self::AlterGround(Provider::read(&reader.field("provider")?)?)
            }
            "minecraft:beehive" => Self::Beehive(float("probability")?),
            "minecraft:cocoa" => Self::Cocoa(float("probability")?),
            "minecraft:creaking_heart" => Self::CreakingHeart(float("probability")?),
            "minecraft:leave_vine" => Self::LeaveVine(float("probability")?),
            "minecraft:trunk_vine" => Self::TrunkVine,
            "minecraft:attached_to_leaves" => Self::AttachedToLeaves {
                probability: float("probability")?,
                exclusion_radius_xz: reader.field("exclusion_radius_xz")?.int()?,
                exclusion_radius_y: reader.field("exclusion_radius_y")?.int()?,
                required_empty_blocks: reader.field("required_empty_blocks")?.int()?,
                provider: Provider::read(&reader.field("block_provider")?)?,
                directions: read_directions(&reader.field("directions")?)?,
            },
            "minecraft:attached_to_logs" => Self::AttachedToLogs {
                probability: float("probability")?,
                provider: Provider::read(&reader.field("block_provider")?)?,
                directions: read_directions(&reader.field("directions")?)?,
            },
            "minecraft:place_on_ground" => Self::PlaceOnGround {
                tries: int_or("tries", 128)?,
                radius: int_or("radius", 2)?,
                height: int_or("height", 1)?,
                provider: Provider::read(&reader.field("block_state_provider")?)?,
            },
            "minecraft:pale_moss" => {
                let name = "minecraft:pale_moss_patch";
                let (index, _) = data::configured(name)
                    .ok_or_else(|| reader.error(format!("{name} is not a configured feature")))?;
                Self::PaleMoss {
                    leaves_probability: float("leaves_probability")?,
                    trunk_probability: float("trunk_probability")?,
                    ground_probability: float("ground_probability")?,
                    patch: FeatureRef::Named(index),
                }
            }
            "minecraft:shelf_mushroom" => Self::ShelfMushroom(float("probability")?),
            other => return Err(reader.error(format!("{other} is not a tree decorator"))),
        };
        reader.finish()?;
        Ok(decorator)
    }
}

/// What decorators are given (`TreeDecorator.Context`): the logs, leaves and roots
/// sorted by height once, and the set the decorations are added to.
pub struct Decorating<'d> {
    pub logs: Vec<BlockPos>,
    pub leaves: Vec<BlockPos>,
    pub roots: Vec<BlockPos>,
    /// A tree's set of decorations; a fallen tree keeps none.
    pub decorations: Option<&'d mut JavaSet>,
}

impl Decorating<'_> {
    /// The decoration setter: the block is set as a tree sets its blocks.
    fn set(&mut self, context: &mut Context<'_, '_>, pos: BlockPos, state: BlockState) -> bool {
        if let Some(decorations) = self.decorations.as_deref_mut() {
            decorations.insert(pos);
        }
        context.level.set_block_known_shape(pos, state)
    }

    /// `placeVine`.
    fn vine(&mut self, context: &mut Context<'_, '_>, pos: BlockPos, face: Face) {
        self.set(context, pos, vine(face));
    }
}

/// Places each decorator in turn.
pub fn decorate(
    context: &mut Context<'_, '_>,
    decorators: &[Decorator],
    with: &mut Decorating<'_>,
) {
    for decorator in decorators {
        match decorator {
            Decorator::AlterGround(provider) => alter_ground(context, provider, with),
            Decorator::Beehive(probability) => beehive(context, *probability, with),
            Decorator::Cocoa(probability) => cocoa(context, *probability, with),
            Decorator::CreakingHeart(probability) => creaking_heart(context, *probability, with),
            Decorator::LeaveVine(probability) => leave_vine(context, *probability, with),
            Decorator::TrunkVine => trunk_vine(context, with),
            Decorator::AttachedToLeaves {
                probability,
                exclusion_radius_xz,
                exclusion_radius_y,
                required_empty_blocks,
                provider,
                directions,
            } => attached_to_leaves(
                context,
                with,
                AttachedToLeaves {
                    probability: *probability,
                    exclusion_radius_xz: *exclusion_radius_xz,
                    exclusion_radius_y: *exclusion_radius_y,
                    required_empty_blocks: *required_empty_blocks,
                    provider,
                    directions,
                },
            ),
            Decorator::AttachedToLogs {
                probability,
                provider,
                directions,
            } => attached_to_logs(context, *probability, provider, directions, with),
            Decorator::PlaceOnGround {
                tries,
                radius,
                height,
                provider,
            } => place_on_ground(context, *tries, *radius, *height, provider, with),
            Decorator::PaleMoss {
                leaves_probability,
                trunk_probability,
                ground_probability,
                patch,
            } => pale_moss(
                context,
                [*leaves_probability, *trunk_probability, *ground_probability],
                patch,
                with,
            ),
            Decorator::ShelfMushroom(probability) => shelf_mushroom(context, *probability, with),
        }
    }
}

/// The lowest logs or roots, which the ground decorators work around: the roots
/// where they are lower than the logs, both where they start level.
fn lowest_trunks_or_roots(with: &Decorating<'_>) -> Vec<BlockPos> {
    if with.roots.is_empty() {
        return with.logs.clone();
    }
    if with
        .logs
        .first()
        .is_some_and(|log| with.roots[0].y == log.y)
    {
        let mut positions = with.logs.clone();
        positions.extend(with.roots.iter().copied());
        positions
    } else {
        with.roots.clone()
    }
}

fn alter_ground(context: &mut Context<'_, '_>, provider: &Provider, with: &mut Decorating<'_>) {
    let positions = lowest_trunks_or_roots(with);
    let Some(first) = positions.first() else {
        return;
    };
    let min_y = first.y;
    for pos in positions.iter().filter(|pos| pos.y == min_y) {
        alter_circle(context, provider, pos.offset(-1, 0, -1), with);
        alter_circle(context, provider, pos.offset(2, 0, -1), with);
        alter_circle(context, provider, pos.offset(-1, 0, 2), with);
        alter_circle(context, provider, pos.offset(2, 0, 2), with);
        for _ in 0..5 {
            let placement = context.random.next_i32_bounded(64);
            let x = placement % 8;
            let z = placement / 8;
            if x == 0 || x == 7 || z == 0 || z == 7 {
                alter_circle(context, provider, pos.offset(-3 + x, 0, -3 + z), with);
            }
        }
    }
}

fn alter_circle(
    context: &mut Context<'_, '_>,
    provider: &Provider,
    pos: BlockPos,
    with: &mut Decorating<'_>,
) {
    for x in -2_i32..=2 {
        for z in -2_i32..=2 {
            if x.abs() != 2 || z.abs() != 2 {
                alter_block(context, provider, pos.offset(x, 0, z), with);
            }
        }
    }
}

/// From two above to three below, the first block the provider has something for.
fn alter_block(
    context: &mut Context<'_, '_>,
    provider: &Provider,
    pos: BlockPos,
    with: &mut Decorating<'_>,
) {
    for y in (-3..=2).rev() {
        let cursor = pos.offset(0, y, 0);
        if let Some(state) = provider.optional_state(context, cursor) {
            with.set(context, cursor, state);
            break;
        }
        if !is_air(context.level, cursor) && y < 0 {
            break;
        }
    }
}

fn place_on_ground(
    context: &mut Context<'_, '_>,
    tries: i32,
    radius: i32,
    height: i32,
    provider: &Provider,
    with: &mut Decorating<'_>,
) {
    let positions = lowest_trunks_or_roots(with);
    let Some(origin) = positions.first().copied() else {
        return;
    };
    let base_y = origin.y;
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (origin.x, origin.x, origin.z, origin.z);
    for pos in positions.iter().filter(|pos| pos.y == base_y) {
        min_x = min_x.min(pos.x);
        max_x = max_x.max(pos.x);
        min_z = min_z.min(pos.z);
        max_z = max_z.max(pos.z);
    }
    let (min_x, max_x, min_z, max_z) = (
        min_x - radius,
        max_x + radius,
        min_z - radius,
        max_z + radius,
    );
    let min_y = base_y - height;
    let max_y = base_y + height;
    for _ in 0..tries {
        let x = random_between_inclusive(&mut *context.random, min_x, max_x);
        let y = random_between_inclusive(&mut *context.random, min_y, max_y);
        let z = random_between_inclusive(&mut *context.random, min_z, max_z);
        let pos = BlockPos::new(x, y, z);
        let above = pos.offset(0, 1, 0);
        let above_state = context.level.block(above);
        if !above_state.props().is_air() && !is_block(above_state, blocks::VINE) {
            continue;
        }
        if !context.level.block(pos).props().is_solid_render() {
            continue;
        }
        if context
            .level
            .height_at(Heightmap::MotionBlockingNoLeaves, pos.x, pos.z)
            > above.y
        {
            continue;
        }
        let state = provider.state(context, above);
        with.set(context, above, state);
    }
}

fn trunk_vine(context: &mut Context<'_, '_>, with: &mut Decorating<'_>) {
    let logs = with.logs.clone();
    for log in logs {
        for (side, face) in [
            (Face::West, Face::East),
            (Face::East, Face::West),
            (Face::North, Face::South),
            (Face::South, Face::North),
        ] {
            if context.random.next_i32_bounded(3) > 0 {
                let pos = relative(log, side);
                if is_air(context.level, pos) {
                    with.vine(context, pos, face);
                }
            }
        }
    }
}

fn leave_vine(context: &mut Context<'_, '_>, probability: f32, with: &mut Decorating<'_>) {
    let leaves = with.leaves.clone();
    for leaf in leaves {
        for (side, face) in [
            (Face::West, Face::East),
            (Face::East, Face::West),
            (Face::North, Face::South),
            (Face::South, Face::North),
        ] {
            if context.random.next_f32() < probability {
                let pos = relative(leaf, side);
                if is_air(context.level, pos) {
                    hanging_vine(context, pos, face, with);
                }
            }
        }
    }
}

/// `LeaveVineDecorator.addHangingVine`: a vine and up to four more below it.
fn hanging_vine(
    context: &mut Context<'_, '_>,
    pos: BlockPos,
    face: Face,
    with: &mut Decorating<'_>,
) {
    with.vine(context, pos, face);
    let mut pos = pos.offset(0, -1, 0);
    let mut left = 4;
    while is_air(context.level, pos) && left > 0 {
        with.vine(context, pos, face);
        pos = pos.offset(0, -1, 0);
        left -= 1;
    }
}

fn cocoa(context: &mut Context<'_, '_>, probability: f32, with: &mut Decorating<'_>) {
    if context.random.next_f32() >= probability {
        return;
    }
    let Some(first) = with.logs.first() else {
        return;
    };
    let tree_y = first.y;
    let logs: Vec<BlockPos> = with
        .logs
        .iter()
        .copied()
        .filter(|pos| pos.y - tree_y <= 2)
        .collect();
    for log in logs {
        for direction in HORIZONTAL {
            if context.random.next_f32() <= 0.25 {
                let pos = relative(log, direction.opposite());
                if is_air(context.level, pos) {
                    let age = context.random.next_i32_bounded(3);
                    let state = blocks::COCOA
                        .with_property("age", &age.to_string())
                        .and_then(|state| state.with_property("facing", face_name(direction)))
                        .expect("cocoa has an age to 2 and a horizontal facing");
                    with.set(context, pos, state);
                }
            }
        }
    }
}

/// A beehive's place: beside a log at the height under the lowest leaves, or a little
/// above the foot of a tree without leaves; the nest is given two or three bees.
fn beehive(context: &mut Context<'_, '_>, probability: f32, with: &mut Decorating<'_>) {
    if with.logs.is_empty() || context.random.next_f32() >= probability {
        return;
    }
    let first_log = with.logs[0];
    let hive_y = match with.leaves.first() {
        Some(leaf) => (leaf.y - 1).max(first_log.y + 1),
        None => {
            let y = first_log.y + 1 + context.random.next_i32_bounded(3);
            y.min(with.logs[with.logs.len() - 1].y)
        }
    };
    let mut places: Vec<BlockPos> = with
        .logs
        .iter()
        .filter(|pos| pos.y == hive_y)
        .flat_map(|pos| HIVE_SIDES.map(|side| relative(*pos, side)))
        .collect();
    if places.is_empty() {
        return;
    }
    shuffle(&mut places, &mut *context.random);
    let Some(hive) = places.into_iter().find(|pos| {
        is_air(context.level, *pos) && is_air(context.level, relative(*pos, HIVE_FACING))
    }) else {
        return;
    };
    let state = blocks::BEE_NEST
        .with_property("facing", face_name(HIVE_FACING))
        .expect("a bee nest faces a horizontal direction");
    // The bees are drawn only where the nest stands, and so has its block entity.
    if !with.set(context, hive, state) {
        return;
    }
    let count = 2 + context.random.next_i32_bounded(2);
    let bees: Vec<i32> = (0..count)
        .map(|_| context.random.next_i32_bounded(599))
        .collect();
    context
        .level
        .set_block_entity(hive, Some(block_entities::beehive(state, &bees)));
}

/// The blocks of `AttachedToLeavesDecorator`.
struct AttachedToLeaves<'d> {
    probability: f32,
    exclusion_radius_xz: i32,
    exclusion_radius_y: i32,
    required_empty_blocks: i32,
    provider: &'d Provider,
    directions: &'d [Face],
}

fn attached_to_leaves(
    context: &mut Context<'_, '_>,
    with: &mut Decorating<'_>,
    attached: AttachedToLeaves<'_>,
) {
    // Only asked whether it holds a position, so its order does not matter.
    let mut blacklist = BTreeSet::new();
    let mut leaves = with.leaves.clone();
    shuffle(&mut leaves, &mut *context.random);
    for leaf in leaves {
        let direction = attached.directions[context
            .random
            .next_i32_bounded(attached.directions.len() as i32)
            as usize];
        let pos = relative(leaf, direction);
        if blacklist.contains(&pos)
            || context.random.next_f32() >= attached.probability
            || !(1..=attached.required_empty_blocks)
                .all(|distance| is_air(context.level, relative_by(leaf, direction, distance)))
        {
            continue;
        }
        let (xz, y) = (attached.exclusion_radius_xz, attached.exclusion_radius_y);
        for dx in -xz..=xz {
            for dy in -y..=y {
                for dz in -xz..=xz {
                    blacklist.insert(pos.offset(dx, dy, dz));
                }
            }
        }
        let state = attached.provider.state(context, pos);
        with.set(context, pos, state);
    }
}

fn attached_to_logs(
    context: &mut Context<'_, '_>,
    probability: f32,
    provider: &Provider,
    directions: &[Face],
    with: &mut Decorating<'_>,
) {
    let mut logs = with.logs.clone();
    shuffle(&mut logs, &mut *context.random);
    for log in logs {
        let direction =
            directions[context.random.next_i32_bounded(directions.len() as i32) as usize];
        let pos = relative(log, direction);
        if context.random.next_f32() > probability || !is_air(context.level, pos) {
            continue;
        }
        let state = provider.state(context, pos);
        with.set(context, pos, state);
    }
}

fn pale_moss(
    context: &mut Context<'_, '_>,
    [leaves_probability, trunk_probability, ground_probability]: [f32; 3],
    patch: &FeatureRef,
    with: &mut Decorating<'_>,
) {
    let mut shuffled = with.logs.clone();
    shuffle(&mut shuffled, &mut *context.random);
    // The first of the lowest, as Java's `Stream.min` keeps.
    let Some(origin) = shuffled.into_iter().min_by_key(|pos| pos.y) else {
        return;
    };
    if context.random.next_f32() < ground_probability {
        patch.place(context, origin.offset(0, 1, 0));
    }
    let logs = with.logs.clone();
    for log in logs {
        if context.random.next_f32() < trunk_probability {
            let down = log.offset(0, -1, 0);
            if is_air(context.level, down) {
                moss_hanger(context, down, with);
            }
        }
    }
    let leaves = with.leaves.clone();
    for leaf in leaves {
        if context.random.next_f32() < leaves_probability {
            let down = leaf.offset(0, -1, 0);
            if is_air(context.level, down) {
                moss_hanger(context, down, with);
            }
        }
    }
}

/// `PaleMossDecorator.addMossHanger`: hanging moss down to a tip, each block below
/// the first with an even chance of being the last.
fn moss_hanger(context: &mut Context<'_, '_>, mut pos: BlockPos, with: &mut Decorating<'_>) {
    let moss = |tip: bool| {
        blocks::PALE_HANGING_MOSS
            .with_property("tip", if tip { "true" } else { "false" })
            .expect("pale hanging moss has a tip")
    };
    while is_air(context.level, pos.offset(0, -1, 0)) && context.random.next_f32() >= 0.5 {
        with.set(context, pos, moss(false));
        pos = pos.offset(0, -1, 0);
    }
    with.set(context, pos, moss(true));
}

/// The creaking heart takes the place of a log with logs on every side.
fn creaking_heart(context: &mut Context<'_, '_>, probability: f32, with: &mut Decorating<'_>) {
    if with.logs.is_empty() || context.random.next_f32() >= probability {
        return;
    }
    let mut logs = with.logs.clone();
    shuffle(&mut logs, &mut *context.random);
    let Some(heart) = logs.into_iter().find(|pos| {
        Face::ALL
            .iter()
            .all(|face| LOGS.contains(context.level.block(relative(*pos, *face))))
    }) else {
        return;
    };
    let state = blocks::CREAKING_HEART
        .with_property("creaking_heart_state", "dormant")
        .and_then(|state| state.with_property("natural", "true"))
        .expect("a creaking heart has a state and is natural or not");
    if with.set(context, heart, state) {
        context
            .level
            .set_block_entity(heart, Some(block_entities::empty(state)));
    }
}

fn shelf_mushroom(context: &mut Context<'_, '_>, probability: f32, with: &mut Decorating<'_>) {
    if context.random.next_f32() >= probability {
        return;
    }
    let logs = with.logs.clone();
    let (Some(first), Some(last)) = (logs.first().copied(), logs.last().copied()) else {
        return;
    };
    if first.y == last.y {
        // A fallen log: on its two long sides.
        let directions = if first.x == last.x {
            [Face::East, Face::West]
        } else {
            [Face::North, Face::South]
        };
        for log in &logs {
            for facing in directions {
                if context.random.next_f32() <= 0.25 {
                    let pos = relative(*log, facing);
                    if shelf_replaceable(context, pos)
                        && !beside_shelf_mushroom(context, pos)
                        && !beside_shelf_mushroom(context, *log)
                    {
                        place_shelf_mushroom(context, pos, facing, with);
                    }
                }
            }
        }
    } else {
        let first_direction = random_horizontal(&mut *context.random);
        let directions = [first_direction, clockwise(first_direction)];
        for log in &logs {
            let height = log.y - first.y;
            if !(1..=4).contains(&height) {
                continue;
            }
            for facing in directions {
                if context.random.next_f32() <= 0.25 {
                    let pos = relative(*log, facing);
                    if shelf_replaceable(context, pos)
                        && !is_block(
                            context.level.block(pos.offset(0, -1, 0)),
                            blocks::SHELF_MUSHROOM,
                        )
                    {
                        place_shelf_mushroom(context, pos, facing, with);
                        break;
                    }
                }
            }
        }
    }
}

fn place_shelf_mushroom(
    context: &mut Context<'_, '_>,
    pos: BlockPos,
    facing: Face,
    with: &mut Decorating<'_>,
) {
    let age = context.random.next_i32_bounded(2);
    let state = blocks::SHELF_MUSHROOM
        .with_property("age", &age.to_string())
        .and_then(|state| state.with_property("facing", face_name(facing)))
        .expect("a shelf mushroom has an age to 1 and a horizontal facing");
    with.set(context, pos, state);
}

/// Replaceable, and neither water nor beside water.
fn shelf_replaceable(context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
    let here = context.level.block(pos);
    here.props().can_be_replaced()
        && !HORIZONTAL
            .iter()
            .any(|face| is_block(context.level.block(relative(pos, *face)), blocks::WATER))
        && !is_block(here, blocks::WATER)
}

fn beside_shelf_mushroom(context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
    HORIZONTAL.iter().any(|face| {
        is_block(
            context.level.block(relative(pos, *face)),
            blocks::SHELF_MUSHROOM,
        )
    })
}
