// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/sculk_patch.rs`.

//! `sculk_patch`: charges of sculk let loose at the origin, which spread veins and
//! turn the ground they cross into sculk, growing sensors and shriekers here and
//! there, as a sculk catalyst spreads them in play (`SculkPatchFeature`, with the
//! game's `SculkSpreader` made for world generation and the `SculkBehaviour` of sculk,
//! sculk veins and every other block).
//!
//! In 26.3 the catalyst and the rare shriekers of the patch are features of their own
//! in the data, after this one in a sequence.
//!
//! The game merges cursors that meet and sends the client a level event for each,
//! but not in world generation, where neither has an effect; so cursors are kept in
//! a list, in the order the game's list keeps them. The sensors and shriekers it
//! grows get block entities that the game makes empty, as for every block with one
//! that a feature places.

use clustine_data::{BlockState, Face, SupportType, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::multiface::{
    Spreader, available_faces, can_attach, has_any_face, has_face, still_water, with_face,
};
use super::{all_shuffled, shuffle};
use crate::blocks::{in_block_tag, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::survive::sturdy;

#[derive(Debug)]
pub struct SculkPatch {
    charge_count: i32,
    amount_per_charge: i32,
    spread_attempts: i32,
    growth_rounds: i32,
    spread_rounds: i32,
}

impl SculkPatch {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            charge_count: reader.field("charge_count")?.int()?,
            amount_per_charge: reader.field("amount_per_charge")?.int()?,
            spread_attempts: reader.field("spread_attempts")?.int()?,
            growth_rounds: reader.field("growth_rounds")?.int()?,
            spread_rounds: reader.field("spread_rounds")?.int()?,
        })
    }
}

/// The spreader of world generation (`SculkSpreader.createWorldGenSpreader`).
const REPLACEABLE: &str = "minecraft:sculk_replaceable_world_gen";
const GROWTH_SPAWN_COST: i32 = 50;
const NO_GROWTH_RADIUS: i32 = 1;
const CHARGE_DECAY_RATE: i32 = 5;
const ADDITIONAL_DECAY_RATE: i32 = 10;
const MAX_CHARGE: i32 = 1000;
const MAX_CURSORS: usize = 32;
const MAX_CURSOR_DISTANCE: i32 = 1024;
/// How far from the origin, across, a cursor may move in world generation.
const MAX_WORLDGEN_SPREAD: i64 = 12;

/// What a block does with a charge (`SculkBehaviour`): sculk's, a vein's, or every
/// other block's (`SculkBehaviour.DEFAULT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Default,
    Sculk,
    Vein,
}

fn behaviour(state: BlockState) -> Behaviour {
    if same_block(state, blocks::SCULK) {
        Behaviour::Sculk
    } else if same_block(state, blocks::SCULK_VEIN) {
        Behaviour::Vein
    } else {
        Behaviour::Default
    }
}

/// A charge on its way (`SculkSpreader.ChargeCursor`).
#[derive(Debug, Clone)]
struct Cursor {
    pos: BlockPos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    /// The sides of the vein it was last on; none before it has moved.
    facings: Option<Vec<Face>>,
}

impl Cursor {
    fn new(pos: BlockPos, charge: i32) -> Self {
        Self {
            pos,
            charge,
            update_delay: 0,
            decay_delay: 1,
            facings: None,
        }
    }
}

/// `SculkSpreader.addCursors`: the charge in cursors of up to a thousand, as many as
/// there is room for.
fn add_cursors(cursors: &mut Vec<Cursor>, at: BlockPos, mut charge: i32) {
    while charge > 0 {
        let current = charge.min(MAX_CHARGE);
        if cursors.len() < MAX_CURSORS {
            cursors.push(Cursor::new(at, current));
        }
        charge -= current;
    }
}

impl Feature for SculkPatch {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !can_spread_from(context.level, origin) {
            return false;
        }
        let mut cursors = Vec::new();
        for round in 0..self.spread_rounds + self.growth_rounds {
            for _ in 0..self.charge_count {
                add_cursors(&mut cursors, origin, self.amount_per_charge);
            }
            let spread_veins = round < self.spread_rounds;
            for _ in 0..self.spread_attempts {
                update_cursors(context, origin, &mut cursors, spread_veins);
            }
            cursors.clear();
        }
        true
    }
}

/// `canSpreadFrom`: sculk or a vein, or air or a source of water beside a block whose
/// collision fills it.
fn can_spread_from(level: &mut Level<'_>, origin: BlockPos) -> bool {
    let start = level.block(origin);
    if behaviour(start) != Behaviour::Default {
        return true;
    }
    if !start.props().is_air()
        && (!same_block(start, blocks::WATER)
            || !start.props().fluid().is_some_and(|fluid| fluid.source))
    {
        return false;
    }
    Face::ALL.into_iter().any(|face| {
        level
            .block(relative(origin, face))
            .props()
            .collision_is_full_block()
    })
}

/// `SculkSpreader.updateCursors`: each cursor once, in order; those with charge left
/// stay.
fn update_cursors(
    context: &mut Context<'_, '_>,
    origin: BlockPos,
    cursors: &mut Vec<Cursor>,
    spread_veins: bool,
) {
    let previous = std::mem::take(cursors);
    for mut cursor in previous {
        let far = (cursor.pos.x - origin.x)
            .abs()
            .max((cursor.pos.y - origin.y).abs())
            .max((cursor.pos.z - origin.z).abs());
        if far > MAX_CURSOR_DISTANCE {
            continue;
        }
        update_cursor(context, origin, &mut cursor, spread_veins);
        if cursor.charge > 0 {
            cursors.push(cursor);
        }
    }
}

/// `ChargeCursor.update`.
fn update_cursor(
    context: &mut Context<'_, '_>,
    origin: BlockPos,
    cursor: &mut Cursor,
    spread_veins: bool,
) {
    if cursor.charge <= 0 {
        return;
    }
    if cursor.update_delay > 0 {
        cursor.update_delay -= 1;
        return;
    }
    let mut state = context.level.block(cursor.pos);
    let mut current = behaviour(state);
    if spread_veins
        && attempt_spread_vein(
            context.level,
            cursor.pos,
            state,
            cursor.facings.as_deref(),
            current,
        )
        && current != Behaviour::Sculk
    {
        // Every behaviour but sculk's may have changed the block.
        state = context.level.block(cursor.pos);
        current = behaviour(state);
    }
    cursor.charge = attempt_use_charge(context, origin, cursor, current, spread_veins);
    if cursor.charge <= 0 {
        on_discharged(context.level, state, cursor.pos);
        return;
    }
    match valid_movement_pos(context, cursor.pos, origin) {
        Some(to) => {
            on_discharged(context.level, state, cursor.pos);
            cursor.pos = to;
            state = context.level.block(to);
        }
        None => {
            // In world generation a charge that cannot move is spent.
            on_discharged(context.level, state, cursor.pos);
            cursor.charge = 0;
            return;
        }
    }
    if behaviour(state) != Behaviour::Default {
        cursor.facings = Some(available_faces(state));
    }
    // The behaviour of the block it left decides its delays.
    cursor.decay_delay = match current {
        Behaviour::Default => (cursor.decay_delay - 1).max(0),
        Behaviour::Sculk | Behaviour::Vein => 1,
    };
    cursor.update_delay = 1;
}

/// `attemptSpreadVein`.
fn attempt_spread_vein(
    level: &mut Level<'_>,
    pos: BlockPos,
    state: BlockState,
    facings: Option<&[Face]>,
    current: Behaviour,
) -> bool {
    // World generation marks what spreads for post-processing.
    let post_process = true;
    match (current, facings) {
        (Behaviour::Default, None) => {
            let here = level.block(pos);
            Spreader::sculk_same_space().spread_all(level, here, pos, post_process) > 0
        }
        (Behaviour::Default, Some(faces)) if !faces.is_empty() => {
            if !state.props().is_air() && !still_water(state) {
                return false;
            }
            regrow(level, pos, state, faces)
        }
        _ => Spreader::of(blocks::SCULK_VEIN).spread_all(level, state, pos, post_process) > 0,
    }
}

/// `SculkVeinBlock.regrow`: a vein on those of `faces` it can hold on to.
fn regrow(level: &mut Level<'_>, pos: BlockPos, existing: BlockState, faces: &[Face]) -> bool {
    let mut vein = blocks::SCULK_VEIN;
    let mut any = false;
    for face in faces {
        if can_attach(level, pos, *face) {
            vein = with_face(vein, *face, true);
            any = true;
        }
    }
    if !any {
        return false;
    }
    if existing.props().fluid().is_some() {
        vein = vein
            .with_property("waterlogged", "true")
            .expect("a sculk vein can be waterlogged");
    }
    level.set_block(pos, vein);
    true
}

/// `attemptUseCharge`: what charge is left after the block has used some.
fn attempt_use_charge(
    context: &mut Context<'_, '_>,
    origin: BlockPos,
    cursor: &Cursor,
    current: Behaviour,
    spread_veins: bool,
) -> i32 {
    match current {
        Behaviour::Default => {
            if cursor.decay_delay > 0 {
                cursor.charge
            } else {
                0
            }
        }
        Behaviour::Sculk => sculk_use_charge(context, origin, cursor),
        Behaviour::Vein => {
            if spread_veins && attempt_place_sculk(context, cursor.pos) {
                cursor.charge - 1
            } else if context.random.next_i32_bounded(CHARGE_DECAY_RATE) == 0 {
                (cursor.charge as f32 * 0.5).floor() as i32
            } else {
                cursor.charge
            }
        }
    }
}

/// `SculkBlock.attemptUseCharge`: by a chance, a sensor or shrieker grown above, or
/// the charge decays.
fn sculk_use_charge(context: &mut Context<'_, '_>, origin: BlockPos, cursor: &Cursor) -> i32 {
    let charge = cursor.charge;
    if charge == 0 || context.random.next_i32_bounded(CHARGE_DECAY_RATE) != 0 {
        return charge;
    }
    let pos = cursor.pos;
    let close = distance_squared(pos, origin) < f64::from(NO_GROWTH_RADIUS * NO_GROWTH_RADIUS);
    if !close && can_place_growth(context.level, pos) {
        if context.random.next_i32_bounded(GROWTH_SPAWN_COST) < charge {
            let above = relative(pos, Face::Up);
            let growth = random_growth(context, above);
            context.level.set_block(above, growth);
        }
        return (charge - GROWTH_SPAWN_COST).max(0);
    }
    if context.random.next_i32_bounded(ADDITIONAL_DECAY_RATE) != 0 {
        charge
    } else if close {
        charge - 1
    } else {
        charge - decay_penalty(pos, origin, charge)
    }
}

/// `Vec3i.distSqr`, in doubles as the game takes it.
fn distance_squared(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.x) - f64::from(b.x);
    let dy = f64::from(a.y) - f64::from(b.y);
    let dz = f64::from(a.z) - f64::from(b.z);
    dx * dx + dy * dy + dz * dz
}

/// `SculkBlock.getDecayPenalty`: more the further from the origin.
fn decay_penalty(pos: BlockPos, origin: BlockPos, charge: i32) -> i32 {
    let outside = distance_squared(pos, origin).sqrt() as f32 - NO_GROWTH_RADIUS as f32;
    let outside_squared = outside * outside;
    let reach = 24 - NO_GROWTH_RADIUS;
    let factor = (outside_squared / (reach * reach) as f32).min(1.0);
    ((charge as f32 * factor * 0.5) as i32).max(1)
}

/// `SculkBlock.canPlaceGrowth`: air or still water above, and no more than two
/// sensors and shriekers near.
fn can_place_growth(level: &mut Level<'_>, pos: BlockPos) -> bool {
    let above = level.block(relative(pos, Face::Up));
    if !above.props().is_air() && !(same_block(above, blocks::WATER) && still_water(above)) {
        return false;
    }
    let mut growths = 0;
    // `BlockPos.betweenClosed`: x fastest, then y, then z.
    for z in -4..=4 {
        for y in 0..=2 {
            for x in -4..=4 {
                let state = level.block(pos.offset(x, y, z));
                if same_block(state, blocks::SCULK_SENSOR)
                    || same_block(state, blocks::SCULK_SHRIEKER)
                {
                    growths += 1;
                }
                if growths > 2 {
                    return false;
                }
            }
        }
    }
    true
}

/// `SculkBlock.getRandomGrowthState`: a shrieker that can summon one time in eleven,
/// else a sensor, waterlogged where there is fluid.
fn random_growth(context: &mut Context<'_, '_>, at: BlockPos) -> BlockState {
    let state = if context.random.next_i32_bounded(11) == 0 {
        blocks::SCULK_SHRIEKER
            .with_property("can_summon", "true")
            .expect("a shrieker can summon")
    } else {
        blocks::SCULK_SENSOR
    };
    if state.property("waterlogged").is_some() && context.level.block(at).props().fluid().is_some()
    {
        state
            .with_property("waterlogged", "true")
            .expect("the state has the property")
    } else {
        state
    }
}

/// `SculkVeinBlock.attemptPlaceSculk`: the block behind one of the vein's sides, in a
/// shuffled order, turned to sculk where it may be; veins spread from it, and the
/// veins around it let go of it.
fn attempt_place_sculk(context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
    let state = context.level.block(pos);
    for face in all_shuffled(&mut *context.random) {
        if !has_face(state, face) {
            continue;
        }
        let support = relative(pos, face);
        if !in_block_tag(context.level.block(support), REPLACEABLE) {
            continue;
        }
        context.level.set_block(support, blocks::SCULK);
        Spreader::of(blocks::SCULK_VEIN).spread_all(context.level, blocks::SCULK, support, true);
        let skip = face.opposite();
        for direction in Face::ALL {
            if direction == skip {
                continue;
            }
            let vein_pos = relative(support, direction);
            let vein = context.level.block(vein_pos);
            if same_block(vein, blocks::SCULK_VEIN) {
                on_discharged(context.level, vein, vein_pos);
            }
        }
        return true;
    }
    false
}

/// `onDischarged`, which only a vein does anything in: it lets go of the sides that
/// lie on sculk, and is gone with none left.
fn on_discharged(level: &mut Level<'_>, mut state: BlockState, pos: BlockPos) {
    if !same_block(state, blocks::SCULK_VEIN) {
        return;
    }
    for face in Face::ALL {
        if has_face(state, face) && same_block(level.block(relative(pos, face)), blocks::SCULK) {
            state = with_face(state, face, false);
        }
    }
    if !has_any_face(state) {
        // The game asks the world, not the state, for the fluid there.
        let water = level.block(pos).props().fluid().is_some();
        state = if water { blocks::WATER } else { blocks::AIR };
    }
    level.set_block(pos, state);
}

/// `getValidMovementPos`: a neighbour that is sculk or a vein, not across a corner and
/// not through a full side; the first, in a shuffled order, that is a vein on
/// something sculk can replace, else the last that would do. In world generation, not
/// beyond twelve blocks across from the origin.
fn valid_movement_pos(
    context: &mut Context<'_, '_>,
    pos: BlockPos,
    origin: BlockPos,
) -> Option<BlockPos> {
    let mut offsets = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && (x != 0 || y != 0 || z != 0) {
                    offsets.push((x, y, z));
                }
            }
        }
    }
    shuffle(&mut *context.random, &mut offsets);
    let mut found = pos;
    for (x, y, z) in offsets {
        let neighbour = pos.offset(x, y, z);
        let dx = i64::from(origin.x) - i64::from(neighbour.x);
        let dz = i64::from(origin.z) - i64::from(neighbour.z);
        if dx * dx + dz * dz > MAX_WORLDGEN_SPREAD * MAX_WORLDGEN_SPREAD {
            continue;
        }
        let state = context.level.block(neighbour);
        if behaviour(state) == Behaviour::Default
            || !movement_unobstructed(context.level, pos, neighbour)
        {
            continue;
        }
        found = neighbour;
        if substrate_access(context.level, state, neighbour) {
            break;
        }
    }
    (found != pos).then_some(found)
}

/// `isMovementUnobstructed`: to a face neighbour always; to an edge neighbour, if one
/// of the two blocks on the way does not close it off with a full side.
fn movement_unobstructed(level: &mut Level<'_>, from: BlockPos, to: BlockPos) -> bool {
    let (dx, dy, dz) = (to.x - from.x, to.y - from.y, to.z - from.z);
    if dx.abs() + dy.abs() + dz.abs() == 1 {
        return true;
    }
    let x = if dx < 0 { Face::West } else { Face::East };
    let y = if dy < 0 { Face::Down } else { Face::Up };
    let z = if dz < 0 { Face::North } else { Face::South };
    let mut open = |face: Face| {
        let test = relative(from, face);
        !sturdy(level.block(test), face.opposite(), SupportType::Full)
    };
    if dx == 0 {
        open(y) || open(z)
    } else if dy == 0 {
        open(x) || open(z)
    } else {
        open(x) || open(y)
    }
}

/// `SculkVeinBlock.hasSubstrateAccess`: a vein with a side on a block sculk can
/// replace.
fn substrate_access(level: &mut Level<'_>, state: BlockState, pos: BlockPos) -> bool {
    if !same_block(state, blocks::SCULK_VEIN) {
        return false;
    }
    Face::ALL.into_iter().any(|face| {
        has_face(state, face)
            && in_block_tag(
                level.block(relative(pos, face)),
                "minecraft:sculk_replaceable",
            )
    })
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    static PATCH: Data = Data::Object(&[
        ("type", Data::Text("minecraft:sculk_patch")),
        ("amount_per_charge", Data::Number("32")),
        ("charge_count", Data::Number("10")),
        ("growth_rounds", Data::Number("0")),
        ("spread_attempts", Data::Number("64")),
        ("spread_rounds", Data::Number("1")),
    ]);

    fn patch() -> Box<dyn Feature> {
        crate::feature::read_configured(&Reader::new(&PATCH, "test")).unwrap()
    }

    #[test]
    fn a_patch_turns_the_deep_dark_s_floor_to_sculk_around_the_origin() {
        let mut world = World::flat(blocks::DEEPSLATE, -20);
        let origin = BlockPos::new(8, -19, 8);
        assert!(world.with_context(3, |context| patch().place(context, origin)));
        let sculk: Vec<BlockPos> = world
            .writes
            .iter()
            .filter(|(_, state)| *state == blocks::SCULK)
            .map(|(pos, _)| *pos)
            .collect();
        assert!(sculk.len() > 10, "{}", sculk.len());
        for pos in &sculk {
            assert_eq!(world.get(*pos), blocks::SCULK);
            let (dx, dz) = (i64::from(pos.x - 8), i64::from(pos.z - 8));
            // A cursor stays within twelve across; the sculk is the block behind its
            // vein, one further.
            assert!(dx * dx + dz * dz <= 13 * 13, "{pos:?}");
        }
        // Every vein left lies on something.
        for (pos, _) in &world.writes {
            let state = world.get(*pos);
            if same_block(state, blocks::SCULK_VEIN) {
                assert!(has_any_face(state), "{pos:?}");
            }
        }
        // Veins mark themselves for post-processing as they spread.
        assert!(!world.marks.is_empty());
    }

    #[test]
    fn a_patch_in_open_air_or_in_stone_is_not_placed() {
        let mut world = World::flat(blocks::DEEPSLATE, -60);
        assert!(!world.with_context(3, |context| patch().place(context, BlockPos::new(8, 0, 8))));
        assert!(!world.with_context(3, |context| {
            patch().place(context, BlockPos::new(8, -61, 8))
        }));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_patch_with_growth_rounds_grows_sensors_away_from_the_origin() {
        static GROWING: Data = Data::Object(&[
            ("type", Data::Text("minecraft:sculk_patch")),
            ("amount_per_charge", Data::Number("32")),
            ("charge_count", Data::Number("10")),
            ("growth_rounds", Data::Number("2")),
            ("spread_attempts", Data::Number("64")),
            ("spread_rounds", Data::Number("2")),
        ]);
        let growing = crate::feature::read_configured(&Reader::new(&GROWING, "test")).unwrap();
        let mut grown = 0;
        for seed in 0..8 {
            let mut world = World::flat(blocks::DEEPSLATE, -20);
            let origin = BlockPos::new(8, -19, 8);
            assert!(world.with_context(seed, |context| growing.place(context, origin)));
            for (pos, state) in &world.writes {
                if same_block(*state, blocks::SCULK_SENSOR)
                    || same_block(*state, blocks::SCULK_SHRIEKER)
                {
                    grown += 1;
                    assert!(same_block(world.get(pos.offset(0, -1, 0)), blocks::SCULK));
                    if same_block(*state, blocks::SCULK_SHRIEKER) {
                        assert_eq!(state.property("can_summon"), Some("true"));
                    }
                }
            }
        }
        assert!(grown > 0);
    }

    #[test]
    fn the_decay_penalty_grows_with_the_distance_from_the_origin() {
        let origin = BlockPos::new(0, 0, 0);
        assert_eq!(decay_penalty(BlockPos::new(1, 0, 0), origin, 100), 1);
        // Twelve out: (11 / 23)² of half the charge.
        assert_eq!(decay_penalty(BlockPos::new(12, 0, 0), origin, 1000), 114);
        assert_eq!(decay_penalty(BlockPos::new(40, 0, 0), origin, 100), 50);
    }

    #[test]
    fn charges_beyond_a_thousand_split_and_cursors_stop_at_thirty_two() {
        let mut cursors = Vec::new();
        add_cursors(&mut cursors, BlockPos::new(0, 0, 0), 2500);
        let charges: Vec<i32> = cursors.iter().map(|cursor| cursor.charge).collect();
        assert_eq!(charges, vec![1000, 1000, 500]);
        for _ in 0..40 {
            add_cursors(&mut cursors, BlockPos::new(0, 0, 0), 32);
        }
        assert_eq!(cursors.len(), MAX_CURSORS);
    }
}
