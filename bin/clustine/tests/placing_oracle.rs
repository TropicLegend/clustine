//! Comparisons with the official server of what a click with the use button does:
//! placing a block with the state vanilla gives it, using a door, trapdoor, fence gate
//! or flower pot, and buckets and lily pads (`docs/groundwork/w6-plan.md`, sections 3
//! to 5, step W6f).
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them, with every
//! case's states printed, with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test placing_oracle -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Each test is a table of cases. A case is a few blocks of its own in a grid away from
//! the spawn: what is set up there first with plain blocks (stone, which no rule
//! changes), pits dug and water poured, then the clicks that are judged, and the
//! positions whose states are compared. Every case is run on the official server and
//! then on Clustine, by the same bot doing the same things, and all differences are
//! listed at the end rather than the first one alone.
//!
//! The states are read by a second bot that joins once every case has been run: the
//! chunks it is sent hold every block as the server has it then. The builder itself
//! cannot be trusted with that, since the official server sends what changes in one
//! section in one tick (a door's two halves, a bed's head and foot) in a
//! `section_blocks_update`, which the bots do not read. Both servers serve the same
//! version, so their numeric block state ids are the same and are compared as numbers.
//!
//! Where Clustine differs on purpose (section 7 of the plan: a neighbour does not
//! change in turn until B1, chests stay single, rails do not curve), only the block
//! clicked or placed into is compared, and its other half where it has one.
//!
//! Run on 2026-10-14, the official server left one or two clicks of the test undone
//! in every run, a different one each time: an animal that had wandered onto the spot,
//! as it turned out, since the official server does not place a block into a mob.
//! Since 2026-10-15 it starts with no mobs and none spawning (`Oracle::start`), and two
//! runs in a row matched in every case with nothing gone wrong on the way.

mod common;

use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};
use clustine_data::{BlockState, items};
use clustine_protocol::packets::play::face::{BOTTOM as DOWN, EAST, NORTH, SOUTH, TOP as UP, WEST};

use common::start;

/// How long a bot waits for what it should be sent soon: an acknowledgement, a block
/// it placed, the chunks of the grid.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the builder waits to be shown what a step of the setup placed before it
/// goes on and notes that it was not shown it.
const SETUP_PATIENCE: Duration = Duration::from_secs(5);

/// The hotbar slot the builder holds its item in, and one it never fills, for a click
/// with an empty hand.
const HELD_SLOT: i16 = 0;
const EMPTY_SLOT: i16 = 8;

/// The grid of cases: the x of the first column and the z of the first row of their
/// origins, the distance between two of them, and the number of columns. The grid
/// keeps clear of the column at the origin, where a joining player stands on Clustine,
/// and of the ten blocks around it the official server spreads its players over; it
/// lies within the chunks at x 0 to 3, a single region where `CLUSTINE_TEST_PINS=0,4`
/// divides the world; and the reader at the spawn holds all of it on either server.
const FIRST_X: i32 = 14;
const FIRST_Z: i32 = -42;
const SPACING: i32 = 6;
const COLUMNS: usize = 8;
const ROWS: usize = 17;

/// How high above the ground the builder flies from one case to the next: above
/// everything a case builds, which reaches no higher than two blocks above the ground.
const CRUISE: f64 = 3.0;

/// A position relative to a case's origin: x and z as in the world, and y from the
/// first block above the ground, so that the ground's top block is at y -1.
type Rel = (i32, i32, i32);

/// The builder's look, as yaw and pitch in degrees. A yaw of 0 looks south, 90 west,
/// 180 north and 270 east; a positive pitch looks down.
type Look = (f32, f32);

/// A block in the world as the builder was shown it: where, and its state if the
/// builder held its chunk.
type Seen = ((i32, i32, i32), Option<i32>);

const LOOKS_NORTH: Look = (180.0, 0.0);
const LOOKS_DOWN: Look = (0.0, 90.0);

/// Where the builder stands for most cases: three blocks south of the origin, on the
/// ground, in a lane between the cases that nothing is built in.
const SOUTH_OF_IT: (f64, f64, f64) = (0.5, 0.0, 3.5);

/// Where it stands to come at a case from the north.
const NORTH_OF_IT: (f64, f64, f64) = (0.5, 0.0, -2.5);

/// Where it hovers for a case in a pit: a block above the origin, so that it pours a
/// bucket straight down into the pit.
const ABOVE_IT: (f64, f64, f64) = (0.5, 1.0, 0.5);

/// What the builder holds for a step.
#[derive(Debug, Clone, Copy)]
enum Held {
    Nothing,
    Item(i32),
}

/// What the builder does in a step.
#[derive(Debug, Clone, Copy)]
enum Act {
    /// The use button on a face of a block, with where on the block the cursor is.
    Click { at: Rel, face: u8, cursor: [f32; 3] },
    /// The use button with no block in view as far as the client says: what a client
    /// sends for a bucket or a lily pad. The ray goes along the step's look.
    Use,
    /// Breaking a block, as a creative-mode player does.
    Dig(Rel),
}

#[derive(Debug, Clone, Copy)]
struct Step {
    held: Held,
    look: Look,
    sneaking: bool,
    act: Act,
}

impl Step {
    fn look(mut self, yaw: f32, pitch: f32) -> Self {
        self.look = (yaw, pitch);
        self
    }

    fn sneaking(mut self) -> Self {
        self.sneaking = true;
        self
    }

    /// Where on the clicked block the click lands, each from 0 to 1. The coordinate
    /// across the clicked face is put on that face, as a client's would be.
    fn cursor(mut self, x: f32, y: f32, z: f32) -> Self {
        if let Act::Click { face, cursor, .. } = &mut self.act {
            *cursor = on_face(*face, [x, y, z]);
        }
        self
    }

    /// A click on a side at the height `y` on the block, in the middle across.
    fn height(self, y: f32) -> Self {
        self.cursor(0.5, y, 0.5)
    }
}

/// `cursor` with the coordinate across `face` moved onto that face.
fn on_face(face: u8, cursor: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = cursor;
    match face {
        DOWN => [x, 0.0, z],
        UP => [x, 1.0, z],
        NORTH => [x, y, 0.0],
        SOUTH => [x, y, 1.0],
        WEST => [0.0, y, z],
        _ => [1.0, y, z],
    }
}

/// A click with `item` on the `face` of the block `at`, in the middle of the face,
/// looking north.
fn click(item: i32, at: Rel, face: u8) -> Step {
    Step {
        held: Held::Item(item),
        look: LOOKS_NORTH,
        sneaking: false,
        act: Act::Click {
            at,
            face,
            cursor: on_face(face, [0.5, 0.5, 0.5]),
        },
    }
}

/// A click with `item` on the top of the block below `at`, which places into `at`.
fn put(item: i32, at: Rel) -> Step {
    click(item, (at.0, at.1 - 1, at.2), UP)
}

/// Stone into `at`, on top of the block below it.
fn stone(at: Rel) -> Step {
    put(items::STONE, at)
}

/// A click with an empty hand.
fn bare(at: Rel, face: u8) -> Step {
    Step {
        held: Held::Nothing,
        ..click(items::STONE, at, face)
    }
}

/// Breaking the block `at`.
fn dig(at: Rel) -> Step {
    Step {
        held: Held::Nothing,
        look: LOOKS_DOWN,
        sneaking: false,
        act: Act::Dig(at),
    }
}

/// Using `item` looking straight down, from [`ABOVE_IT`]: a bucket or a lily pad.
fn pour(item: i32) -> Step {
    Step {
        held: Held::Item(item),
        look: LOOKS_DOWN,
        sneaking: false,
        act: Act::Use,
    }
}

/// Stone at (1, 0, 0) and (1, 1, 0) and, against the latter, at (0, 1, 0): a block
/// over the origin whose underside can be clicked, which places into the origin.
fn overhang() -> [Step; 3] {
    [
        stone((1, 0, 0)),
        click(items::STONE, (1, 0, 0), UP),
        click(items::STONE, (1, 1, 0), WEST),
    ]
}

/// A pit one block deep at the origin, its sides and floor the ground's.
fn pit() -> [Step; 1] {
    [dig((0, -1, 0))]
}

/// A pit two blocks deep at the origin.
fn deep_pit() -> [Step; 2] {
    [dig((0, -1, 0)), dig((0, -2, 0))]
}

/// A pit one block deep with a water source in it, poured from above the origin.
fn pond() -> [Step; 2] {
    [dig((0, -1, 0)), pour(items::WATER_BUCKET)]
}

/// Two stones, one on the other, at `x` and `z` beside the origin.
fn pillar(x: i32, z: i32) -> [Step; 2] {
    [stone((x, 0, z)), click(items::STONE, (x, 0, z), UP)]
}

struct Case {
    name: String,
    /// The origin in the world as x and z, for a case that has to be at a given
    /// place; otherwise it has a cell of the grid.
    origin: Option<(i32, i32)>,
    /// Where the builder's feet are for the whole case.
    stand: (f64, f64, f64),
    /// What is done first; each step is waited for until the builder is shown it.
    setup: Vec<Step>,
    /// What is judged.
    steps: Vec<Step>,
    /// The positions compared afterwards.
    compared: Vec<Rel>,
}

/// A case doing `step` and comparing the positions `compared`.
fn case(name: &str, step: Step, compared: &[Rel]) -> Case {
    Case {
        name: name.to_owned(),
        origin: None,
        stand: SOUTH_OF_IT,
        setup: Vec::new(),
        steps: vec![step],
        compared: compared.to_vec(),
    }
}

impl Case {
    fn setup(mut self, steps: impl IntoIterator<Item = Step>) -> Self {
        self.setup.extend(steps);
        self
    }

    fn then(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    fn stand(mut self, stand: (f64, f64, f64)) -> Self {
        self.stand = stand;
        self
    }

    fn at(mut self, x: i32, z: i32) -> Self {
        self.origin = Some((x, z));
        self
    }
}

const O: Rel = (0, 0, 0);
const ABOVE: Rel = (0, 1, 0);

// The placing rules of section 4.4 that orient a block, a row of the plan's table at a
// time.
fn placing_cases() -> Vec<Case> {
    use items::*;

    vec![
        // RotatedPillarBlock and ChainBlock: the axis of the clicked face.
        case("a log on the ground stands upright", put(OAK_LOG, O), &[O]),
        case(
            "a log against the east side of a stone lies along x",
            click(OAK_LOG, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a log against the south side of a stone lies along z",
            click(OAK_LOG, (0, 0, -1), SOUTH),
            &[O],
        )
        .setup([stone((0, 0, -1))]),
        case(
            "a chain against the east side of a stone lies along x",
            click(IRON_CHAIN, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a chain on the ground stands upright",
            put(IRON_CHAIN, O),
            &[O],
        ),
        // Facing the opposite of the horizontal direction.
        case(
            "a furnace placed looking north faces south",
            put(FURNACE, O),
            &[O],
        ),
        case(
            "a furnace placed looking west faces east",
            put(FURNACE, O).look(90.0, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of 44.9 faces north",
            put(FURNACE, O).look(44.9, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of 45 faces as vanilla's rounding says",
            put(FURNACE, O).look(45.0, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of 45.1 faces east",
            put(FURNACE, O).look(45.1, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of 134.9 faces east",
            put(FURNACE, O).look(134.9, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of 135.1 faces south",
            put(FURNACE, O).look(135.1, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of -44.9 faces north",
            put(FURNACE, O).look(-44.9, 0.0),
            &[O],
        ),
        case(
            "a furnace placed at a yaw of -45.1 faces west",
            put(FURNACE, O).look(-45.1, 0.0),
            &[O],
        ),
        case(
            "a repeater placed looking south faces north",
            put(REPEATER, O).look(0.0, 30.0),
            &[O],
        ),
        case(
            "a repeater placed looking east faces west",
            put(REPEATER, O).look(270.0, 30.0),
            &[O],
        ),
        case(
            "a carved pumpkin placed looking south faces north",
            put(CARVED_PUMPKIN, O).look(0.0, 0.0),
            &[O],
        ),
        case(
            "glazed terracotta placed looking west faces east",
            put(WHITE_GLAZED_TERRACOTTA, O).look(90.0, 0.0),
            &[O],
        ),
        // ChestBlock: as the furnace, and single.
        case(
            "a chest placed looking south faces north",
            put(CHEST, O).look(0.0, 0.0),
            &[O],
        ),
        case(
            "a chest placed looking east faces west",
            put(CHEST, O).look(270.0, 0.0),
            &[O],
        ),
        // CampfireBlock.
        case(
            "a campfire placed looking south faces south",
            put(CAMPFIRE, O).look(0.0, 0.0),
            &[O],
        ),
        case(
            "a campfire placed looking west faces west",
            put(CAMPFIRE, O).look(90.0, 0.0),
            &[O],
        ),
        case(
            "a campfire on hay is a signal fire",
            click(CAMPFIRE, O, UP),
            &[ABOVE],
        )
        .setup([put(HAY_BLOCK, O)]),
        case(
            "a campfire placed into water is waterlogged and not lit",
            click(CAMPFIRE, (0, -2, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        // AnvilBlock: the horizontal direction turned clockwise.
        case("an anvil placed looking north", put(ANVIL, O), &[O]),
        case(
            "an anvil placed looking west",
            put(ANVIL, O).look(90.0, 0.0),
            &[O],
        ),
        // BarrelBlock, DispenserBlock, PistonBaseBlock: the opposite of the nearest
        // looking direction.
        case(
            "a barrel placed looking down faces up",
            put(BARREL, O).look(0.0, 60.0),
            &[O],
        ),
        case(
            "a barrel placed looking south faces north",
            put(BARREL, O).look(0.0, 10.0),
            &[O],
        ),
        case(
            "a dispenser placed looking up faces down",
            click(DISPENSER, (-1, 0, 0), EAST).look(0.0, -60.0),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a piston placed at a pitch of 44.9 faces north",
            put(PISTON, O).look(0.0, 44.9),
            &[O],
        ),
        case(
            "a piston placed at a pitch of 45 faces as vanilla's sine table says",
            put(PISTON, O).look(0.0, 45.0),
            &[O],
        ),
        case(
            "a piston placed at a pitch of 45.1 faces up",
            put(PISTON, O).look(0.0, 45.1),
            &[O],
        ),
        case(
            "a piston placed at a yaw of 44.9 faces north",
            put(PISTON, O).look(44.9, 20.0),
            &[O],
        ),
        case(
            "a piston placed at a yaw of 45 faces as vanilla's sine table says",
            put(PISTON, O).look(45.0, 20.0),
            &[O],
        ),
        case(
            "a piston placed at a yaw of 45.1 faces east",
            put(PISTON, O).look(45.1, 20.0),
            &[O],
        ),
        case(
            "a barrel placed at a pitch of -44.9 faces north",
            put(BARREL, O).look(0.0, -44.9),
            &[O],
        ),
        case(
            "a barrel placed at a pitch of -45.1 faces down",
            put(BARREL, O).look(0.0, -45.1),
            &[O],
        ),
        // ObserverBlock: the nearest looking direction.
        case(
            "an observer placed at a pitch of 44.9 faces south",
            put(OBSERVER, O).look(0.0, 44.9),
            &[O],
        ),
        case(
            "an observer placed at a pitch of 45.1 faces down",
            put(OBSERVER, O).look(0.0, 45.1),
            &[O],
        ),
        case(
            "an observer placed looking west faces west",
            put(OBSERVER, O).look(90.0, 0.0),
            &[O],
        ),
        // EndRodBlock and LightningRodBlock.
        case("an end rod on the ground faces up", put(END_ROD, O), &[O]),
        case(
            "an end rod against the east side of a stone faces east",
            click(END_ROD, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "an end rod on top of an end rod facing up faces down",
            click(END_ROD, O, UP),
            &[ABOVE],
        )
        .setup([put(END_ROD, O)]),
        case(
            "a lightning rod on the ground faces up",
            put(LIGHTNING_ROD, O),
            &[O],
        ),
        case(
            "a lightning rod against the east side of a stone faces east",
            click(LIGHTNING_ROD, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // StairBlock: facing, half and shape.
        case(
            "stairs on the ground placed looking north",
            put(OAK_STAIRS, O),
            &[O],
        ),
        case(
            "stairs on the ground placed looking west",
            put(OAK_STAIRS, O).look(90.0, 0.0),
            &[O],
        ),
        case(
            "stairs placed at a yaw of 44.9",
            put(OAK_STAIRS, O).look(44.9, 0.0),
            &[O],
        ),
        case(
            "stairs placed at a yaw of 45.1",
            put(OAK_STAIRS, O).look(45.1, 0.0),
            &[O],
        ),
        case(
            "stairs against the upper half of a stone's side are upside down",
            click(OAK_STAIRS, (-1, 0, 0), EAST).height(0.75),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "stairs against the lower half of a stone's side are upright",
            click(OAK_STAIRS, (-1, 0, 0), EAST).height(0.25),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "stairs against the underside of a block are upside down",
            click(OAK_STAIRS, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case(
            "stairs facing west in front of stairs facing north make an inner corner",
            put(OAK_STAIRS, O).look(90.0, 0.0),
            &[O],
        )
        .setup([put(OAK_STAIRS, (1, 0, 0))]),
        case(
            "stairs facing west behind stairs facing north make an outer corner",
            put(OAK_STAIRS, O).look(90.0, 0.0),
            &[O],
        )
        .setup([put(OAK_STAIRS, (-1, 0, 0))]),
        case(
            "stairs beside stairs facing the same way stay straight",
            put(OAK_STAIRS, O),
            &[O],
        )
        .setup([put(OAK_STAIRS, (-1, 0, 0)), stone((1, 0, 0))]),
        case(
            "stairs on stairs of the other half make no corner",
            put(OAK_STAIRS, O).look(90.0, 0.0),
            &[O],
        )
        .setup([
            stone((2, 0, 0)),
            click(OAK_STAIRS, (2, 0, 0), WEST).height(0.75),
        ]),
        // SlabBlock.
        case(
            "a slab on the ground is a bottom slab",
            put(STONE_SLAB, O),
            &[O],
        ),
        case(
            "a slab against the upper half of a stone's side is a top slab",
            click(STONE_SLAB, (-1, 0, 0), EAST).height(0.75),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a slab against the lower half of a stone's side is a bottom slab",
            click(STONE_SLAB, (-1, 0, 0), EAST).height(0.25),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a slab against the underside of a block is a top slab",
            click(STONE_SLAB, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case(
            "a slab on top of a bottom slab makes a double slab",
            click(STONE_SLAB, O, UP),
            &[O, ABOVE],
        )
        .setup([put(STONE_SLAB, O)]),
        case(
            "a slab against the underside of a top slab makes a double slab",
            click(STONE_SLAB, ABOVE, DOWN),
            &[O, ABOVE],
        )
        .setup([
            stone((1, 0, 0)),
            click(STONE, (1, 0, 0), UP),
            click(STONE_SLAB, (1, 1, 0), WEST).height(0.75),
        ]),
        case(
            "a slab against a stone beside a bottom slab joins it from the side",
            click(STONE_SLAB, (-1, 0, 0), EAST).height(0.25),
            &[O],
        )
        .setup([stone((-1, 0, 0)), put(STONE_SLAB, O)]),
        case(
            "a slab clicked high on the side of a bottom slab joins it",
            click(STONE_SLAB, O, SOUTH).height(0.75),
            &[O, (0, 0, 1)],
        )
        .setup([put(STONE_SLAB, O)]),
        case(
            "a slab clicked low on the side of a bottom slab goes beside it",
            click(STONE_SLAB, O, SOUTH).height(0.25),
            &[O, (0, 0, 1)],
        )
        .setup([put(STONE_SLAB, O)]),
        case(
            "a slab of another kind on a bottom slab goes above it",
            click(OAK_SLAB, O, UP),
            &[O, ABOVE],
        )
        .setup([put(STONE_SLAB, O)]),
        case(
            "a slab placed into water is waterlogged",
            click(STONE_SLAB, (0, -2, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "a slab joining a waterlogged slab makes a double slab without water",
            click(STONE_SLAB, (0, -1, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond())
        .setup([click(STONE_SLAB, (0, -2, 0), UP)]),
        // DoorBlock: facing, hinge, both halves.
        case("a door placed looking north", put(OAK_DOOR, O), &[O, ABOVE]),
        case(
            "a door placed looking west",
            put(OAK_DOOR, O).look(90.0, 0.0),
            &[O, ABOVE],
        ),
        case(
            "a door placed with the cursor on the west of the block",
            put(OAK_DOOR, O).cursor(0.2, 1.0, 0.5),
            &[O, ABOVE],
        ),
        case(
            "a door placed with the cursor on the east of the block",
            put(OAK_DOOR, O).cursor(0.8, 1.0, 0.5),
            &[O, ABOVE],
        ),
        case(
            "a door beside a wall on its west",
            put(OAK_DOOR, O).cursor(0.8, 1.0, 0.5),
            &[O, ABOVE],
        )
        .setup(pillar(-1, 0)),
        case(
            "a door beside a wall on its east",
            put(OAK_DOOR, O).cursor(0.2, 1.0, 0.5),
            &[O, ABOVE],
        )
        .setup(pillar(1, 0)),
        case(
            "a door beside a door on its west makes a double door",
            put(OAK_DOOR, O).cursor(0.2, 1.0, 0.5),
            &[O, ABOVE],
        )
        .setup([put(OAK_DOOR, (-1, 0, 0)).cursor(0.2, 1.0, 0.5)]),
        case(
            "a door under a block is not placed",
            put(OAK_DOOR, O),
            &[O, ABOVE],
        )
        .setup(overhang()),
        // TrapDoorBlock.
        case(
            "a trapdoor against the upper half of a stone's side",
            click(OAK_TRAPDOOR, (-1, 0, 0), EAST).height(0.75),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a trapdoor against the lower half of a stone's side",
            click(OAK_TRAPDOOR, (-1, 0, 0), EAST).height(0.25),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a trapdoor on the ground placed looking north",
            put(OAK_TRAPDOOR, O),
            &[O],
        ),
        case(
            "a trapdoor on the ground placed looking east",
            put(OAK_TRAPDOOR, O).look(270.0, 0.0),
            &[O],
        ),
        case(
            "a trapdoor against the underside of a block",
            click(OAK_TRAPDOOR, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        // FenceGateBlock.
        case(
            "a fence gate placed looking north",
            put(OAK_FENCE_GATE, O),
            &[O],
        ),
        case(
            "a fence gate placed looking west",
            put(OAK_FENCE_GATE, O).look(90.0, 0.0),
            &[O],
        ),
        case(
            "a fence gate between walls across its axis is in the wall",
            put(OAK_FENCE_GATE, O),
            &[O],
        )
        .setup([
            put(COBBLESTONE_WALL, (-1, 0, 0)),
            put(COBBLESTONE_WALL, (1, 0, 0)),
        ]),
        case(
            "a fence gate between walls along its axis is not in the wall",
            put(OAK_FENCE_GATE, O).look(90.0, 0.0),
            &[O],
        )
        .setup([
            put(COBBLESTONE_WALL, (-1, 0, 0)),
            put(COBBLESTONE_WALL, (1, 0, 0)),
        ]),
    ]
}

// The rest of the placing rules of section 4.4: connections, attachments, two-block
// plants, snow and plants, and what 4.2 says can be replaced.
fn attaching_cases() -> Vec<Case> {
    use items::*;

    let mut cases = vec![
        // FenceBlock, IronBarsBlock, WallBlock: connections from the neighbours as
        // placed. The neighbours are not compared: they change in turn only in vanilla.
        case(
            "a fence beside stone, a fence, leaves and a pane",
            put(OAK_FENCE, O),
            &[O],
        )
        .setup([
            stone((1, 0, 0)),
            put(OAK_FENCE, (-1, 0, 0)),
            put(OAK_LEAVES, (0, 0, -1)),
            put(GLASS_PANE, (0, 0, 1)),
        ]),
        case(
            "a fence beside a nether brick fence, gates and a wall",
            put(OAK_FENCE, O),
            &[O],
        )
        .setup([
            put(NETHER_BRICK_FENCE, (1, 0, 0)),
            put(OAK_FENCE_GATE, (-1, 0, 0)),
            put(COBBLESTONE_WALL, (0, 0, -1)),
            put(OAK_FENCE_GATE, (0, 0, 1)),
        ]),
        case(
            "a fence beside the back and the front of stairs",
            put(OAK_FENCE, O),
            &[O],
        )
        .setup([
            put(OAK_STAIRS, (-1, 0, 0)).look(270.0, 0.0),
            put(OAK_STAIRS, (1, 0, 0)).look(270.0, 0.0),
            put(OAK_SLAB, (0, 0, -1)),
            put(GLASS, (0, 0, 1)),
        ]),
        case(
            "a nether brick fence beside a wooden fence and stone",
            put(NETHER_BRICK_FENCE, O),
            &[O],
        )
        .setup([put(OAK_FENCE, (-1, 0, 0)), stone((1, 0, 0))]),
        case(
            "a pane beside stone, a pane, bars and a wall",
            put(GLASS_PANE, O),
            &[O],
        )
        .setup([
            stone((1, 0, 0)),
            put(GLASS_PANE, (-1, 0, 0)),
            put(IRON_BARS, (0, 0, -1)),
            put(COBBLESTONE_WALL, (0, 0, 1)),
        ]),
        case(
            "a pane beside leaves, a fence and glass",
            put(GLASS_PANE, O),
            &[O],
        )
        .setup([
            put(OAK_LEAVES, (1, 0, 0)),
            put(OAK_FENCE, (-1, 0, 0)),
            put(GLASS, (0, 0, -1)),
        ]),
        case("a pane between two stones", put(GLASS_PANE, O), &[O])
            .setup([stone((1, 0, 0)), stone((-1, 0, 0))]),
        case("a pane alone", put(GLASS_PANE, O), &[O]),
        case("iron bars beside a pane and stone", put(IRON_BARS, O), &[O])
            .setup([put(GLASS_PANE, (1, 0, 0)), stone((0, 0, -1))]),
        case(
            "a wall beside stone, a wall, a fence and a pane",
            put(COBBLESTONE_WALL, O),
            &[O],
        )
        .setup([
            stone((1, 0, 0)),
            put(COBBLESTONE_WALL, (-1, 0, 0)),
            put(OAK_FENCE, (0, 0, -1)),
            put(GLASS_PANE, (0, 0, 1)),
        ]),
        case(
            "a wall beside a gate across, a gate along and leaves",
            put(COBBLESTONE_WALL, O),
            &[O],
        )
        .setup([
            put(OAK_FENCE_GATE, (1, 0, 0)),
            put(OAK_FENCE_GATE, (0, 0, -1)),
            put(OAK_LEAVES, (-1, 0, 0)),
        ]),
        case(
            "a wall between two walls in a line",
            put(COBBLESTONE_WALL, O),
            &[O],
        )
        .setup([
            put(COBBLESTONE_WALL, (1, 0, 0)),
            put(COBBLESTONE_WALL, (-1, 0, 0)),
        ]),
        case("a wall alone", put(COBBLESTONE_WALL, O), &[O]),
        case(
            "a wall under a block beside stone",
            click(COBBLESTONE_WALL, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case("a wall under a wall", put(COBBLESTONE_WALL, O), &[O]).setup([
            stone((1, 0, 0)),
            click(STONE, (1, 0, 0), UP),
            click(COBBLESTONE_WALL, (1, 1, 0), WEST),
            stone((-1, 0, 0)),
        ]),
        // RedStoneWireBlock.
        case("redstone alone is a cross", put(REDSTONE, O), &[O]),
        case("redstone beside redstone", put(REDSTONE, O), &[O]).setup([put(REDSTONE, (-1, 0, 0))]),
        case(
            "redstone beside a repeater facing it and one across",
            put(REDSTONE, O),
            &[O],
        )
        .setup([put(REPEATER, (0, 0, -1)), put(REPEATER, (1, 0, 0))]),
        case(
            "redstone beside a stone with redstone on it",
            put(REDSTONE, O),
            &[O],
        )
        .setup([stone((1, 0, 0)), click(REDSTONE, (1, 0, 0), UP)]),
        // BaseRailBlock.
        case(
            "a rail placed looking north runs north and south",
            put(RAIL, O),
            &[O],
        ),
        case(
            "a rail placed looking west runs east and west",
            put(RAIL, O).look(90.0, 0.0),
            &[O],
        ),
        case(
            "a powered rail placed looking east",
            put(POWERED_RAIL, O).look(270.0, 0.0),
            &[O],
        ),
        // BedBlock.
        case(
            "a bed placed looking north has its head north",
            put(RED_BED, O),
            &[O, (0, 0, -1)],
        ),
        case(
            "a bed placed looking west has its head west",
            put(RED_BED, O).look(90.0, 0.0),
            &[O, (-1, 0, 0)],
        ),
        case(
            "a bed whose head would be in stone is not placed",
            put(RED_BED, O),
            &[O, (0, 0, -1)],
        )
        .setup([stone((0, 0, -1))]),
        // Standing or on a wall.
        case("a torch on the ground stands", put(TORCH, O), &[O]),
        case(
            "a torch against the east side of a stone hangs on it",
            click(TORCH, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a torch against the underside of a block stands on the ground",
            click(TORCH, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case(
            "a soul torch against the north side of a stone",
            click(SOUL_TORCH, (0, 0, 1), NORTH),
            &[O],
        )
        .stand(NORTH_OF_IT)
        .setup([stone((0, 0, 1))]),
        case(
            "a redstone torch against the east side of a stone",
            click(REDSTONE_TORCH, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a sign on the ground placed at a yaw of 30",
            put(OAK_SIGN, O).look(30.0, 0.0),
            &[O],
        ),
        case(
            "a sign on the ground placed at a yaw of 200",
            put(OAK_SIGN, O).look(200.0, 0.0),
            &[O],
        ),
        case(
            "a sign against the east side of a stone",
            click(OAK_SIGN, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a banner on the ground placed at a yaw of 100",
            put(WHITE_BANNER, O).look(100.0, 0.0),
            &[O],
        ),
        case(
            "a banner against the south side of a stone",
            click(WHITE_BANNER, (0, 0, -1), SOUTH),
            &[O],
        )
        .setup([stone((0, 0, -1))]),
        case(
            "a skull on the ground placed at a yaw of 30",
            put(SKELETON_SKULL, O).look(30.0, 0.0),
            &[O],
        ),
        case(
            "a skull on the ground placed at a yaw of 200",
            put(SKELETON_SKULL, O).look(200.0, 0.0),
            &[O],
        ),
        case(
            "a skull against the east side of a stone",
            click(SKELETON_SKULL, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // Hanging signs.
        case(
            "a hanging sign under a stone placed at a yaw of 30",
            click(OAK_HANGING_SIGN, ABOVE, DOWN).look(30.0, 0.0),
            &[O],
        )
        .setup(overhang()),
        case(
            "a hanging sign under a stone placed sneaking at a yaw of 30",
            click(OAK_HANGING_SIGN, ABOVE, DOWN)
                .look(30.0, 0.0)
                .sneaking(),
            &[O],
        )
        .setup(overhang()),
        case(
            "a hanging sign under a fence placed at a yaw of 30",
            click(OAK_HANGING_SIGN, ABOVE, DOWN).look(30.0, 0.0),
            &[O],
        )
        .setup([
            stone((1, 0, 0)),
            click(STONE, (1, 0, 0), UP),
            click(OAK_FENCE, (1, 1, 0), WEST),
        ]),
        case(
            "a hanging sign against the east side of a stone",
            click(OAK_HANGING_SIGN, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // FaceAttachedHorizontalDirectionalBlock.
        case(
            "a button on the ground placed looking west",
            put(STONE_BUTTON, O).look(90.0, 30.0),
            &[O],
        ),
        case(
            "a button against the east side of a stone",
            click(STONE_BUTTON, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a button against the underside of a block",
            click(STONE_BUTTON, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case(
            "a lever on the ground placed looking north",
            put(LEVER, O).look(180.0, 30.0),
            &[O],
        ),
        case(
            "a lever against the east side of a stone",
            click(LEVER, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a grindstone on the ground placed looking east",
            put(GRINDSTONE, O).look(270.0, 0.0),
            &[O],
        ),
        case(
            "a grindstone against the east side of a stone",
            click(GRINDSTONE, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // LadderBlock.
        case(
            "a ladder against the east side of a stone",
            click(LADDER, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        case(
            "a ladder on the ground in front of a stone placed looking north",
            put(LADDER, O),
            &[O],
        )
        .setup([stone((0, 0, -1))]),
        case(
            "a ladder on the ground with nothing beside it is not placed",
            put(LADDER, O),
            &[O],
        ),
        case(
            "a ladder against the face of a ladder is not placed",
            click(LADDER, O, EAST),
            &[O, (1, 0, 0)],
        )
        .setup([stone((-1, 0, 0)), click(LADDER, (-1, 0, 0), EAST)]),
        // LanternBlock.
        case("a lantern on the ground stands", put(LANTERN, O), &[O]),
        case(
            "a lantern against the underside of a block hangs",
            click(LANTERN, ABOVE, DOWN),
            &[O],
        )
        .setup(overhang()),
        case(
            "a lantern against the east side of a stone stands on the ground",
            click(LANTERN, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // DoublePlantBlock and TallFlowerBlock.
        case(
            "tall grass is placed as two halves",
            put(TALL_GRASS, O),
            &[O, ABOVE],
        ),
        case(
            "a sunflower is placed as two halves",
            put(SUNFLOWER, O),
            &[O, ABOVE],
        ),
        case(
            "tall grass under a block is not placed",
            put(TALL_GRASS, O),
            &[O, ABOVE],
        )
        .setup(overhang()),
        // SnowLayerBlock.
        case("snow on the ground is one layer", put(SNOW, O), &[O]),
        case(
            "snow on top of snow adds a layer",
            click(SNOW, O, UP),
            &[O, ABOVE],
        )
        .setup([put(SNOW, O)]),
        case(
            "snow against the side of snow goes beside it",
            click(SNOW, O, SOUTH).height(0.1),
            &[O, (0, 0, 1)],
        )
        .setup([put(SNOW, O)]),
        case(
            "stone on one layer of snow replaces it",
            click(STONE, O, UP),
            &[O, ABOVE],
        )
        .setup([put(SNOW, O)]),
        case(
            "stone on two layers of snow goes above them",
            click(STONE, O, UP),
            &[O, ABOVE],
        )
        .setup([put(SNOW, O), click(SNOW, O, UP)]),
        // Plants, crops and the rest: the default state, where they can survive.
        case("a poppy on grass", put(POPPY, O), &[O]),
        case(
            "a poppy on stone is not placed",
            click(POPPY, O, UP),
            &[ABOVE],
        )
        .setup([stone(O)]),
        case(
            "wheat seeds on grass are not placed",
            put(WHEAT_SEEDS, O),
            &[O],
        ),
        case("a sapling on grass", put(OAK_SAPLING, O), &[O]),
        case("a cactus on grass is not placed", put(CACTUS, O), &[O]),
        case(
            "sugar cane on grass away from water is not placed",
            put(SUGAR_CANE, O),
            &[O],
        ),
        case(
            "sugar cane on grass beside water",
            put(SUGAR_CANE, (1, 0, 0)),
            &[(1, 0, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case("scaffolding on the ground", put(SCAFFOLDING, O), &[O]),
        case("stone on the ground", put(STONE, O), &[O]),
        case(
            "glass against the east side of a stone",
            click(GLASS, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))]),
        // What can be replaced (4.2).
        case(
            "stone on short grass replaces it",
            click(STONE, O, UP),
            &[O, ABOVE],
        )
        .setup([put(SHORT_GRASS, O)]),
        case(
            "stone against a stone's side into short grass replaces it",
            click(STONE, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0)), put(SHORT_GRASS, O)]),
        case(
            "short grass on short grass is not placed",
            click(SHORT_GRASS, O, UP),
            &[O, ABOVE],
        )
        .setup([put(SHORT_GRASS, O)]),
        case(
            "stone placed into water replaces it",
            click(STONE, (0, -2, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "a fence placed into water is waterlogged",
            click(OAK_FENCE, (0, -2, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "stairs placed into water are waterlogged",
            click(OAK_STAIRS, (0, -2, 0), UP),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "a candle on a candle adds one",
            click(CANDLE, O, UP),
            &[O, ABOVE],
        )
        .setup([put(CANDLE, O)]),
        case(
            "a candle on a candle while sneaking does not add one",
            click(CANDLE, O, UP).sneaking(),
            &[O, ABOVE],
        )
        .setup([put(CANDLE, O)]),
    ];

    // Across a pinned border (chunk x 0, where `CLUSTINE_TEST_PINS=0,4` puts one): the
    // clicked block in one region, the block placed into in the other. These rules
    // read the clicked block only, which travels with the click (4.5).
    cases.push(
        case(
            "a torch against a stone across the border at x 0",
            click(TORCH, (-1, 0, 0), EAST),
            &[O],
        )
        .setup([stone((-1, 0, 0))])
        .at(0, -42),
    );
    cases.push(
        case(
            "a log against a stone across the border at x 0",
            click(OAK_LOG, (0, 0, 0), WEST),
            &[(-1, 0, 0)],
        )
        .setup([stone(O)])
        .at(0, -30),
    );
    cases
}

// Clicks on blocks that are used: doors, trapdoors, fence gates and flower pots, and
// sneaking against them.
fn using_cases() -> Vec<Case> {
    use items::*;

    let door = || put(OAK_DOOR, O);
    let trapdoor = || [stone((-1, 0, 0)), click(OAK_TRAPDOOR, (-1, 0, 0), EAST)];
    let gate = || put(OAK_FENCE_GATE, O);
    let pot = || put(FLOWER_POT, O);
    vec![
        case(
            "a door clicked with an empty hand opens",
            bare(O, SOUTH),
            &[O, ABOVE],
        )
        .setup([door()]),
        case(
            "a door clicked twice opens and closes",
            bare(O, SOUTH),
            &[O, ABOVE],
        )
        .setup([door()])
        .then(bare(ABOVE, SOUTH)),
        case(
            "a door clicked on its upper half opens",
            bare(ABOVE, SOUTH),
            &[O, ABOVE],
        )
        .setup([door()]),
        case(
            "a door clicked with stone in hand opens and nothing is placed",
            click(STONE, O, SOUTH),
            &[O, ABOVE, (0, 0, 1)],
        )
        .setup([door()]),
        case(
            "a door clicked sneaking with stone in hand stays shut and stone is placed",
            click(STONE, O, SOUTH).sneaking(),
            &[O, ABOVE, (0, 0, 1)],
        )
        .setup([door()]),
        case(
            "a door clicked sneaking with an empty hand opens",
            bare(O, SOUTH).sneaking(),
            &[O, ABOVE],
        )
        .setup([door()]),
        case(
            "one door of a double door clicked opens alone",
            bare(O, SOUTH),
            &[O, ABOVE, (-1, 0, 0), (-1, 1, 0)],
        )
        .setup([
            put(OAK_DOOR, (-1, 0, 0)).cursor(0.2, 1.0, 0.5),
            put(OAK_DOOR, O).cursor(0.2, 1.0, 0.5),
        ]),
        case("a trapdoor clicked opens", bare(O, SOUTH), &[O]).setup(trapdoor()),
        case(
            "a trapdoor clicked twice opens and closes",
            bare(O, SOUTH),
            &[O],
        )
        .setup(trapdoor())
        .then(bare(O, SOUTH)),
        case(
            "a trapdoor clicked sneaking with stone in hand stays shut",
            click(STONE, O, SOUTH).sneaking(),
            &[O, (0, 0, 1)],
        )
        .setup(trapdoor()),
        case(
            "a fence gate opened from in front keeps its facing",
            bare(O, SOUTH),
            &[O],
        )
        .setup([gate()]),
        case(
            "a fence gate opened from behind turns to face away",
            bare(O, NORTH).look(0.0, 0.0),
            &[O],
        )
        .stand(NORTH_OF_IT)
        .setup([gate()]),
        case(
            "a fence gate opened from the side keeps its facing",
            bare(O, SOUTH).look(90.0, 0.0),
            &[O],
        )
        .setup([gate()]),
        case(
            "a fence gate clicked twice opens and closes",
            bare(O, SOUTH),
            &[O],
        )
        .setup([gate()])
        .then(bare(O, SOUTH)),
        case(
            "a fence gate opened from behind and closed from in front",
            bare(O, NORTH).look(0.0, 0.0),
            &[O],
        )
        .stand(NORTH_OF_IT)
        .setup([gate()])
        .then(bare(O, NORTH).look(180.0, 0.0)),
        case(
            "a poppy clicked on a flower pot goes into it",
            click(POPPY, O, SOUTH),
            &[O, (0, 0, 1)],
        )
        .setup([pot()]),
        case(
            "a sapling clicked on a flower pot goes into it",
            click(OAK_SAPLING, O, UP),
            &[O, ABOVE],
        )
        .setup([pot()]),
        case(
            "a second flower clicked on a potted flower does nothing",
            click(DANDELION, O, SOUTH),
            &[O, (0, 0, 1)],
        )
        .setup([pot(), click(POPPY, O, SOUTH)]),
        case(
            "stone clicked on a flower pot places nothing",
            click(STONE, O, SOUTH),
            &[O, (0, 0, 1)],
        )
        .setup([pot()]),
        case(
            "stone clicked on a fence is placed against it",
            click(STONE, O, SOUTH),
            // Not the fence, which the official server joins to the stone by a
            // neighbour update and Clustine does not until phase B (B1).
            &[(0, 0, 1)],
        )
        .setup([put(OAK_FENCE, O)]),
        case(
            "an empty hand on a fence does nothing",
            bare(O, SOUTH),
            &[O, (0, 0, 1)],
        )
        .setup([put(OAK_FENCE, O)]),
        case(
            "stone clicked on a chest places nothing",
            click(STONE, O, SOUTH),
            &[O, (0, 0, 1)],
        )
        .setup([put(CHEST, O)]),
        case(
            "stone clicked sneaking on a chest is placed against it",
            click(STONE, O, SOUTH).sneaking(),
            &[O, (0, 0, 1)],
        )
        .setup([put(CHEST, O)]),
        case(
            "a lever clicked does not place the stone in hand",
            click(STONE, O, SOUTH),
            &[(0, 0, 1)],
        )
        .setup([put(LEVER, O)]),
    ]
}

// Buckets and lily pads (section 5), all in pits so that nothing flows out of them on
// the official server, where water and lava flow.
fn bucket_cases() -> Vec<Case> {
    use items::*;

    let slab_in_pit = || click(STONE_SLAB, (0, -2, 0), UP);
    let slab_in_deep_pit = || click(STONE_SLAB, (0, -3, 0), UP);
    vec![
        case(
            "a water bucket on the floor of a pit fills it",
            pour(WATER_BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pit()),
        case(
            "a lava bucket on the floor of a pit fills it",
            pour(LAVA_BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pit()),
        case(
            "a water bucket on a slab waterlogs it",
            pour(WATER_BUCKET),
            &[(0, -1, 0), O],
        )
        .stand(ABOVE_IT)
        .setup(pit())
        .setup([slab_in_pit()]),
        case(
            "a water bucket sneaking on a slab fills the block above it",
            pour(WATER_BUCKET).sneaking(),
            &[(0, -2, 0), (0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(deep_pit())
        .setup([slab_in_deep_pit()]),
        case(
            "a lava bucket on a slab fills the block above it",
            pour(LAVA_BUCKET),
            &[(0, -2, 0), (0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(deep_pit())
        .setup([slab_in_deep_pit()]),
        case(
            "an empty bucket on water empties the pit",
            pour(BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "an empty bucket on a waterlogged slab takes the water",
            pour(BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pond())
        .setup([slab_in_pit()]),
        case(
            "an empty bucket on lava empties the pit",
            pour(BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(pit())
        .setup([pour(LAVA_BUCKET)]),
        case(
            "a lily pad on water floats on it",
            pour(LILY_PAD),
            &[(0, -1, 0), O],
        )
        .stand(ABOVE_IT)
        .setup(pond()),
        case(
            "a lily pad on the ground is not placed",
            pour(LILY_PAD),
            &[O],
        )
        .stand(ABOVE_IT),
        // Looking down past the torch, at the floor beside it: the torch is not solid,
        // so the water goes into its block.
        case(
            "a water bucket on the floor beside a torch washes it away",
            pour(WATER_BUCKET),
            &[(0, -2, 0), (0, -1, 0)],
        )
        .stand((0.2, 1.0, 0.5))
        .setup(deep_pit())
        .setup([click(TORCH, (0, -3, 0), UP)]),
        // Looking down at the torch itself: it cannot hold water, so the water goes
        // above it. Below that, the official server's water flows onto the torch, and
        // Clustine's does not flow yet (B5), so only the block above is compared.
        case(
            "a water bucket on a torch fills the block above it",
            pour(WATER_BUCKET),
            &[(0, -1, 0)],
        )
        .stand(ABOVE_IT)
        .setup(deep_pit())
        .setup([click(TORCH, (0, -3, 0), UP)]),
    ]
}

/// Placing a block with each rule of section 4.4 that orients it gives the state the
/// official server gives.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn placing_matches_the_official_server() {
    compare(&placing_cases()).await;
}

/// Placing a block that connects, hangs, stands on something or replaces what is there
/// gives the state the official server gives.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn attaching_matches_the_official_server() {
    compare(&attaching_cases()).await;
}

/// Doors, trapdoors, fence gates and flower pots are used as on the official server,
/// and sneaking places against them instead.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn using_blocks_matches_the_official_server() {
    compare(&using_cases()).await;
}

/// Buckets and lily pads do what they do on the official server.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn buckets_and_lily_pads_match_the_official_server() {
    compare(&bucket_cases()).await;
}

/// What one server ended with: the states of each case's compared positions, and what
/// went wrong on the way.
struct Ran {
    /// For each case, the states of its compared positions in order; empty for a case
    /// that was not read.
    states: Vec<Vec<Option<i32>>>,
    /// What went wrong, each said in full.
    troubles: Vec<String>,
}

/// Runs `cases` on both servers and fails, listing every difference, if they differ.
async fn compare(cases: &[Case]) {
    assert!(
        cases.len() <= COLUMNS * ROWS,
        "{} cases do not fit into the grid of {COLUMNS} by {ROWS}",
        cases.len()
    );
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;

    let theirs = run(oracle.address(), cases).await;
    let ours = run(&address, cases).await;
    server.stop().await;
    drop(oracle);

    let mut differences = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        println!("{index:3} {}", case.name);
        for (number, at) in case.compared.iter().enumerate() {
            let their = theirs.states[index].get(number).copied().flatten();
            let our = ours.states[index].get(number).copied().flatten();
            let same = their.is_some() && their == our;
            println!(
                "      {at:?}: {} {}{}",
                named(their),
                if same { "=" } else { "differs from Clustine's" },
                if same {
                    String::new()
                } else {
                    format!(" {}", named(our))
                }
            );
            if !same {
                differences.push(format!(
                    "case {index}, \"{}\", at {at:?} from its origin: the official server has \
                     {} and Clustine has {}",
                    case.name,
                    named(their),
                    named(our)
                ));
            }
        }
    }
    for (server, ran) in [("the official server", &theirs), ("Clustine", &ours)] {
        for trouble in &ran.troubles {
            println!("on {server}: {trouble}");
        }
    }
    let troubles: Vec<String> = theirs
        .troubles
        .iter()
        .map(|trouble| format!("on the official server: {trouble}"))
        .chain(
            ours.troubles
                .iter()
                .map(|trouble| format!("on Clustine: {trouble}")),
        )
        .collect();
    assert!(
        differences.is_empty(),
        "{} of the compared blocks differ:\n  {}\n{}",
        differences.len(),
        differences.join("\n  "),
        if troubles.is_empty() {
            String::new()
        } else {
            format!(
                "and these went wrong on the way, which may explain some:\n  {}",
                troubles.join("\n  ")
            )
        }
    );
}

/// A state by its name and properties, as the game writes it.
fn named(state: Option<i32>) -> String {
    let Some(id) = state else {
        return "nothing (the chunk was not there or the case was not run)".to_owned();
    };
    let Some(block) = u16::try_from(id)
        .ok()
        .map(BlockState)
        .filter(|s| s.block().is_some())
    else {
        return format!("the state {id}, which is no state of 26.3");
    };
    let name = block.block().map_or("?", |block| block.name);
    let properties: Vec<String> = block
        .properties()
        .map(|(property, value)| format!("{property}={value}"))
        .collect();
    if properties.is_empty() {
        format!("{name} ({id})")
    } else {
        format!("{name}[{}] ({id})", properties.join(","))
    }
}

/// Where a case is in the world: its origin's x, the height of the first block above
/// the ground, and z.
#[derive(Debug, Clone, Copy)]
struct Place {
    x: i32,
    feet: i32,
    z: i32,
}

impl Place {
    fn of(case: &Case, index: usize, feet: i32) -> Self {
        let (x, z) = case.origin.unwrap_or_else(|| {
            let row = index / COLUMNS;
            let column = index % COLUMNS;
            // Every other row from east to west, so that the builder never flies far.
            let column = if row % 2 == 0 {
                column
            } else {
                COLUMNS - 1 - column
            };
            (
                FIRST_X + SPACING * column as i32,
                FIRST_Z + SPACING * row as i32,
            )
        });
        Self { x, feet, z }
    }

    fn block(&self, at: Rel) -> (i32, i32, i32) {
        (self.x + at.0, self.feet + at.1, self.z + at.2)
    }

    fn point(&self, at: (f64, f64, f64)) -> (f64, f64, f64) {
        (
            f64::from(self.x) + at.0,
            f64::from(self.feet) + at.1,
            f64::from(self.z) + at.2,
        )
    }
}

/// Runs every case on the server at `address`, one after the other, and then reads the
/// compared positions with a bot that joins afterwards.
async fn run(address: &str, cases: &[Case]) -> Ran {
    let mut troubles = Vec::new();
    let mut builder = Bot::join(address, "Builder")
        .await
        .unwrap_or_else(|error| panic!("the builder could not join {address}: {error:#}"));
    builder
        .wait_until(PATIENCE, Bot::is_loaded)
        .await
        .unwrap_or_else(|error| panic!("the builder did not load on {address}: {error:#}"));
    let feet = builder.location.1.floor() as i32;
    // Clustine gives a new player a hotbar of blocks and the official server gives
    // nothing: the slot of the empty hand is emptied on both.
    builder
        .empty_hotbar_slot(EMPTY_SLOT)
        .await
        .unwrap_or_else(|error| {
            panic!("the builder's hand was not emptied on {address}: {error:#}")
        });

    for (index, case) in cases.iter().enumerate() {
        let place = Place::of(case, index, feet);
        let teleports = builder.stats.teleports_confirmed;
        if let Err(error) = perform(&mut builder, case, place, &mut troubles).await {
            troubles.push(format!(
                "case {index}, \"{}\": {error:#}; this and the cases after it were not read",
                case.name
            ));
            let read = vec![Vec::new(); cases.len() - index];
            return Ran {
                states: read_all(address, &cases[..index], feet, &mut troubles)
                    .await
                    .into_iter()
                    .chain(read)
                    .collect(),
                troubles,
            };
        }
        if builder.stats.teleports_confirmed != teleports {
            troubles.push(format!(
                "case {index}, \"{}\": the server put the builder back, to {:?}, which may \
                 have moved it out of reach",
                case.name, builder.location
            ));
        }
    }
    let states = read_all(address, cases, feet, &mut troubles).await;
    Ran { states, troubles }
}

/// Reads the compared positions of `cases` with a bot that joins now and is sent every
/// chunk as it is.
async fn read_all(
    address: &str,
    cases: &[Case],
    feet: i32,
    troubles: &mut Vec<String>,
) -> Vec<Vec<Option<i32>>> {
    let positions: Vec<Vec<(i32, i32, i32)>> = cases
        .iter()
        .enumerate()
        .map(|(index, case)| {
            let place = Place::of(case, index, feet);
            case.compared.iter().map(|at| place.block(*at)).collect()
        })
        .collect();
    let mut reader = match Bot::join(address, "Reader").await {
        Ok(reader) => reader,
        Err(error) => {
            troubles.push(format!("the reader could not join: {error:#}"));
            return vec![Vec::new(); cases.len()];
        }
    };
    let holds_all = |bot: &Bot| {
        positions
            .iter()
            .flatten()
            .all(|&(x, y, z)| matches!(bot.block_at(x, y, z), Ok(Some(_))))
    };
    if let Err(error) = reader.wait_until(PATIENCE, holds_all).await {
        troubles.push(format!(
            "the reader was not sent every chunk of the grid: {error:#}"
        ));
    }
    positions
        .iter()
        .map(|positions| {
            positions
                .iter()
                .map(|&(x, y, z)| reader.block_at(x, y, z).ok().flatten())
                .collect()
        })
        .collect()
}

/// The builder goes to the case's place and does its steps.
async fn perform(
    builder: &mut Bot,
    case: &Case,
    place: Place,
    troubles: &mut Vec<String>,
) -> anyhow::Result<()> {
    let first_look = case
        .setup
        .iter()
        .chain(&case.steps)
        .next()
        .map_or(LOOKS_NORTH, |step| step.look);
    let (x, y, z) = place.point(case.stand);
    let cruise = f64::from(place.feet) + CRUISE;
    // Up from wherever it is, across above everything built, and down into the lane:
    // the official server does not let a player move into a block.
    let (from_x, _, from_z) = builder.location;
    builder
        .fly_to((from_x, cruise, from_z), first_look, 1.0)
        .await?;
    builder.fly_to((x, cruise, z), first_look, 1.0).await?;
    builder.fly_to((x, y, z), first_look, 1.0).await?;

    let mut sneaking = false;
    for (number, step) in case.setup.iter().enumerate() {
        let before = before(builder, step, place);
        do_step(builder, step, place, &mut sneaking).await?;
        if let Some(missing) = shown(builder, step, &before).await {
            troubles.push(format!(
                "case \"{}\", step {number} of its setup: {missing}",
                case.name
            ));
        }
    }
    for step in &case.steps {
        do_step(builder, step, place, &mut sneaking).await?;
    }
    if sneaking {
        builder.sneak(false).await?;
    }
    Ok(())
}

/// Turns the builder, sneaks or stops, takes what the step holds into its hand, does
/// the step and waits for the server to acknowledge it.
async fn do_step(
    builder: &mut Bot,
    step: &Step,
    place: Place,
    sneaking: &mut bool,
) -> anyhow::Result<()> {
    // A position with the look, even where the builder is already, so that the server
    // has the look the step needs before the click.
    let location = builder.location;
    builder.fly_to(location, step.look, 1.0).await?;
    if *sneaking != step.sneaking {
        builder.sneak(step.sneaking).await?;
        *sneaking = step.sneaking;
    }
    match step.held {
        Held::Nothing => builder.select_slot(EMPTY_SLOT).await?,
        Held::Item(item) => {
            builder
                .take_from_creative_inventory(HELD_SLOT, item)
                .await?;
            builder.select_slot(HELD_SLOT).await?;
        }
    }
    let sequence = match step.act {
        Act::Click { at, face, cursor } => {
            let (x, y, z) = place.block(at);
            builder.click(x, y, z, face, cursor).await?
        }
        Act::Use => builder.use_item(step.look.0, step.look.1).await?,
        Act::Dig(at) => {
            let (x, y, z) = place.block(at);
            builder.dig(x, y, z).await?
        }
    };
    builder
        .wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .map_err(|error| anyhow::anyhow!("{step:?} was not acknowledged: {error:#}"))
}

/// What the builder is shown of the blocks a step of the setup changes, taken before
/// the step: for a click, the clicked block and the one beside it, which the official
/// server sends the placer on their own; for a dig, the block dug. Nothing for a pour,
/// whose block the official server sends only with the rest of its section's changes.
fn before(builder: &Bot, step: &Step, place: Place) -> Vec<Seen> {
    let watched = match step.act {
        Act::Click { at, face, .. } => vec![place.block(at), place.block(beside(at, face))],
        Act::Dig(at) => vec![place.block(at)],
        Act::Use => Vec::new(),
    };
    watched
        .into_iter()
        .map(|(x, y, z)| ((x, y, z), builder.block_at(x, y, z).ok().flatten()))
        .collect()
}

/// Waits until the builder is shown a change at one of the blocks `before` holds, so
/// that a step reading it (a fence beside it, a door used) comes after it on Clustine
/// also where it went to another region. Says what was not shown, if anything.
async fn shown(builder: &mut Bot, step: &Step, before: &[Seen]) -> Option<String> {
    let changed = |bot: &Bot| {
        before
            .iter()
            .any(|&((x, y, z), was)| bot.block_at(x, y, z).ok().flatten() != was)
    };
    if before.is_empty() || changed(builder) {
        return None;
    }
    match builder.wait_until(SETUP_PATIENCE, changed).await {
        Ok(()) => None,
        Err(error) => Some(format!(
            "the builder was not shown a change at any of {before:?} after {step:?} \
             ({error:#}); nothing was changed, or the official server sent it in a \
             section_blocks_update, which the bots do not read"
        )),
    }
}

/// The block that a click on the `face` of the block `at` places into when the clicked
/// block is not replaced.
fn beside(at: Rel, face: u8) -> Rel {
    let (x, y, z) = at;
    match face {
        DOWN => (x, y - 1, z),
        UP => (x, y + 1, z),
        NORTH => (x, y, z - 1),
        SOUTH => (x, y, z + 1),
        WEST => (x - 1, y, z),
        _ => (x + 1, y, z),
    }
}

/// The tables can be checked without a server: every case fits into the grid, keeps a
/// block of air between itself and every other case, does not build where the builder
/// stands, and clicks only what the builder reaches.
#[test]
fn the_cases_keep_apart_and_within_reach() {
    use std::collections::BTreeSet;

    for (table, cases) in [
        ("placing", placing_cases()),
        ("attaching", attaching_cases()),
        ("using", using_cases()),
        ("buckets", bucket_cases()),
    ] {
        assert!(cases.len() <= COLUMNS * ROWS, "{table}: too many cases");
        // Any height will do: the cases are compared with each other.
        let feet = -60;
        let mut columns: Vec<[BTreeSet<(i32, i32)>; 2]> = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let place = Place::of(case, index, feet);
            let mut built = BTreeSet::new();
            for step in case.setup.iter().chain(&case.steps) {
                match step.act {
                    Act::Click { at, face, .. } => {
                        built.insert(place.block(at));
                        built.insert(place.block(beside(at, face)));
                        let (x, y, z) = place.block(at);
                        let (eye_x, eye_y, eye_z) = place.point(case.stand);
                        let eye_y = eye_y + 1.62;
                        // To the nearest point of the block, as both servers measure
                        // it, and well within the six blocks they allow.
                        let gap = |low: i32, eye: f64| {
                            (f64::from(low) - eye)
                                .max(eye - f64::from(low) - 1.0)
                                .max(0.0)
                        };
                        let distance =
                            (gap(x, eye_x).powi(2) + gap(y, eye_y).powi(2) + gap(z, eye_z).powi(2))
                                .sqrt();
                        assert!(
                            distance <= 5.0,
                            "{table}, case \"{}\": {at:?} is {distance:.1} from the eyes",
                            case.name
                        );
                    }
                    Act::Dig(at) => {
                        built.insert(place.block(at));
                    }
                    Act::Use => {}
                }
            }
            built.extend(case.compared.iter().map(|&at| place.block(at)));
            let (x, y, z) = place.point(case.stand);
            let stands = [0.0, 1.0, 1.8]
                .map(|up| (x.floor() as i32, (y + up).floor() as i32, z.floor() as i32));
            for block in stands {
                assert!(
                    !built.contains(&block),
                    "{table}, case \"{}\" builds where the builder stands, at {block:?}",
                    case.name
                );
            }
            // A case's blocks keep a column of air from every other case's blocks and
            // stay out of the columns other cases are stood in; two cases may share
            // the column the builder stands in.
            let taken: BTreeSet<(i32, i32)> = built.iter().map(|&(x, _, z)| (x, z)).collect();
            let stood: BTreeSet<(i32, i32)> = stands.iter().map(|&(x, _, z)| (x, z)).collect();
            for (other, [other_taken, other_stood]) in columns.iter().enumerate() {
                let near = |(x, z): (i32, i32)| {
                    [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                        .into_iter()
                        .any(|(dx, dz)| other_taken.contains(&(x + dx, z + dz)))
                };
                assert!(
                    !taken
                        .iter()
                        .any(|&column| near(column) || other_stood.contains(&column))
                        && !stood.iter().any(|column| other_taken.contains(column)),
                    "{table}: case \"{}\" comes within a block of case \"{}\"",
                    case.name,
                    cases[other].name
                );
            }
            columns.push([taken, stood]);
        }
    }
}
