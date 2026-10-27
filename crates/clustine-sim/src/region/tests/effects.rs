//! Tests of effects between regions in the simulation
//! (`docs/adr/0025-effects-between-regions.md`, sections 3 to 6 and the exact delays
//! of section 8): border copies, what a region does in land it does not have loaded,
//! and what it does of what another region sent.
//!
//! Most compare one region with two over the same land: the floor at y = -61 in the
//! chunks from x = -2 to 3 and z = -1 to 1, all loaded by viewers. Divided, the western
//! region holds x up to 0 and copies x = 1, the eastern one holds x from 1 and copies
//! x = 0, so the line is between the blocks x = 15 and x = 16, and the chunks on
//! either side of it tick in both worlds. A post office of the test carries what each
//! of the two does to the other: its effects, numbered, into the other's next tick,
//! due as a store that couples them would convert it (both count the same ticks, so an
//! effect is due at the tick it was made in, and a scheduled tick that and its delay),
//! or from its taking where the test does not couple them, and its block changes into
//! the other's copies.

use clustine_world::{Due, Effect, EffectId, Effected, FiledEffect, SeamKind, Ticked};

use super::*;

const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// The floor's surface: the blocks just above it.
const ON_FLOOR: i32 = -60;

fn at(x: i32, z: i32) -> BlockPos {
    BlockPos::new(x, ON_FLOOR, z)
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn wall_torch(facing: &str) -> BlockState {
    state(&format!("minecraft:wall_torch[facing={facing}]"))
}

fn leaves(distance: u8) -> BlockState {
    state(&format!(
        "minecraft:oak_leaves[distance={distance},persistent=false]"
    ))
}

fn sand_tick() -> Ticked {
    Ticked::Block(clustine_blocks::data::tick_id(blocks::SAND))
}

/// The positions of the land's chunks.
fn land_chunks() -> Vec<ChunkPos> {
    (-2..=3)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// The land: the floor with `blocks` set.
fn land(blocks: &[(BlockPos, BlockState)]) -> BTreeMap<ChunkPos, Chunk> {
    let mut land: BTreeMap<ChunkPos, Chunk> = land_chunks()
        .into_iter()
        .map(|position| (position, floor()))
        .collect();
    for (position, state) in blocks {
        let chunk = land.get_mut(&position.chunk()).expect("within the land");
        let (x, z) = position.in_chunk();
        chunk.set(x, position.y, z, *state);
    }
    land
}

/// A region pinned to `area` that holds the chunks of `land` in it, loaded for
/// viewers, and has copies of `copied`, as it starts.
fn region_of(
    area: ChunkArea,
    ids: u32,
    land: &BTreeMap<ChunkPos, Chunk>,
    copied: &[ChunkPos],
) -> Region {
    let chunks: Vec<(ChunkPos, Chunk)> = land
        .iter()
        .filter(|(position, _)| area.contains(**position))
        .map(|(position, chunk)| (*position, chunk.clone()))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![area],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let copies = copied
        .iter()
        .map(|position| (*position, land[position].clone()))
        .collect();
    let state = RegionState::new(EntityIds::block(ids).expect("a block of ids"));
    Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies)
}

fn column(x: i32) -> Vec<ChunkPos> {
    (-1..=1).map(|z| ChunkPos::new(x, z)).collect()
}

/// What one of the two regions sends the other with its next tick.
#[derive(Debug, Default)]
struct Mail {
    /// The effects, each with the tick of its maker it was made in.
    effects: Vec<(u64, Effect)>,
    /// The block changes, for the receiver's copies.
    changes: Vec<(BlockPos, BlockState)>,
}

/// One region over the land, and two over the same land with the post office between
/// them.
struct Divided {
    whole: Region,
    west: Region,
    east: Region,
    to_west: Mail,
    to_east: Mail,
    /// Block changes the eastern region makes as if in its next tick, which reach the
    /// western region's copies only with what that tick sends.
    held_back: Vec<(BlockPos, BlockState)>,
    sequence: u64,
    /// Whether the store would have the two coupled, and so convert their effects to
    /// be due at the ticks they were made in.
    coupled: bool,
    /// Every block change in the one region, and in the two, with its tick.
    whole_log: Vec<(u64, BlockPos, BlockState)>,
    divided_log: Vec<(u64, BlockPos, BlockState)>,
    /// What each tick of the two regions put out, for the test of determinism.
    outputs: Vec<(TickOutput, TickOutput)>,
}

/// The inputs that carry `mail` to `to`: the effects for chunks it has loaded, which
/// are the only ones a runner hands in, numbered in the order made, and the changes in
/// chunks it copies.
fn inputs(sequence: &mut u64, coupled: bool, to: &Region, mail: Mail) -> TickInputs {
    *sequence += 1;
    let effects = mail
        .effects
        .into_iter()
        .filter(|(_, effect)| to.chunk(effect.at.chunk()).is_some())
        .enumerate()
        .map(|(index, (made, effect))| {
            let due = match effect.what {
                _ if !coupled => Due::FromTaking,
                Effected::Tick { delay, .. } => Due::At {
                    tick: made.saturating_add_signed(i64::from(delay)),
                },
                _ => Due::At { tick: made },
            };
            FiledEffect {
                id: EffectId {
                    sequence: *sequence,
                    index: u32::try_from(index).expect("few effects"),
                },
                effect,
                due,
            }
        })
        .collect();
    let mut copy_updates: Vec<(ChunkPos, Vec<clustine_world::Change>)> = Vec::new();
    for (position, state) in mail.changes {
        let chunk = position.chunk();
        if to.copy(chunk).is_none() {
            continue;
        }
        match copy_updates.last_mut() {
            Some((last, changes)) if *last == chunk => changes.push((position, state).into()),
            _ => copy_updates.push((chunk, vec![(position, state).into()])),
        }
    }
    TickInputs {
        effects,
        copy_updates,
        ..TickInputs::default()
    }
}

fn idle_tick(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

fn changes_of(output: &TickOutput) -> Vec<(BlockPos, BlockState)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockChanged {
                position, state, ..
            } => Some((*position, *state)),
            _ => None,
        })
        .collect()
}

impl Divided {
    /// The two coupled.
    fn new(blocks: &[(BlockPos, BlockState)]) -> Self {
        Self::with(blocks, true)
    }

    /// The two not coupled: what each sends the other counts from its taking.
    fn uncoupled(blocks: &[(BlockPos, BlockState)]) -> Self {
        Self::with(blocks, false)
    }

    fn with(blocks: &[(BlockPos, BlockState)], coupled: bool) -> Self {
        let land = land(blocks);
        Self {
            whole: region_of(ChunkArea::EVERYWHERE, 0, &land, &[]),
            west: region_of(WESTERN, 1, &land, &column(1)),
            east: region_of(EASTERN, 2, &land, &column(0)),
            to_west: Mail::default(),
            to_east: Mail::default(),
            held_back: Vec::new(),
            sequence: 0,
            coupled,
            whole_log: Vec::new(),
            divided_log: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn tick(&self) -> u64 {
        self.whole.tick_number()
    }

    /// Ticks the three regions once.
    fn step(&mut self) {
        let whole = self.whole.tick(&TickInputs::default());
        let tick = whole.tick;
        self.whole_log
            .extend(changes_of(&whole).into_iter().map(|(p, s)| (tick, p, s)));
        let to_west = mem::take(&mut self.to_west);
        let to_east = mem::take(&mut self.to_east);
        let west_inputs = inputs(&mut self.sequence, self.coupled, &self.west, to_west);
        let east_inputs = inputs(&mut self.sequence, self.coupled, &self.east, to_east);
        let west = self.west.tick(&west_inputs);
        let east = self.east.tick(&east_inputs);
        assert_eq!((west.tick, east.tick), (tick, tick));
        for output in [&west, &east] {
            self.divided_log
                .extend(changes_of(output).into_iter().map(|(p, s)| (tick, p, s)));
        }
        self.to_east = Mail {
            effects: west
                .effects
                .iter()
                .map(|effect| (tick, effect.clone()))
                .collect(),
            changes: changes_of(&west),
        };
        let mut changes = mem::take(&mut self.held_back);
        changes.extend(changes_of(&east));
        self.to_west = Mail {
            effects: east
                .effects
                .iter()
                .map(|effect| (tick, effect.clone()))
                .collect(),
            changes,
        };
        self.outputs.push((west, east));
    }

    fn steps(&mut self, count: usize) {
        for _ in 0..count {
            self.step();
        }
    }

    /// Sets the block at `position` between two ticks, in the one region and in the
    /// one of the two that holds it, with every update.
    fn set(&mut self, position: BlockPos, state: BlockState) {
        self.set_as(position, state, false);
    }

    /// Sets the block in the eastern region as if in its next tick: the change reaches
    /// the western region's copy a tick later than [`Divided::set`]'s.
    fn set_in_the_next_tick(&mut self, position: BlockPos, state: BlockState) {
        assert!(position.x >= 16, "in the east");
        self.set_as(position, state, true);
    }

    /// A change between two ticks is logged with the next, in which its effects go out.
    fn set_as(&mut self, position: BlockPos, state: BlockState, late: bool) {
        let tick = self.tick() + 1;
        let mut scratch = TickOutput::default();
        self.whole.set_block(position, state, &mut scratch);
        self.whole_log
            .extend(changes_of(&scratch).into_iter().map(|(p, s)| (tick, p, s)));
        let mut scratch = TickOutput::default();
        let west = position.x < 16;
        let (region, mail) = if west {
            (&mut self.west, &mut self.to_east)
        } else {
            (&mut self.east, &mut self.to_west)
        };
        region.set_block(position, state, &mut scratch);
        let changes = changes_of(&scratch);
        self.divided_log
            .extend(changes.iter().map(|(p, s)| (tick, *p, *s)));
        if late {
            self.held_back.extend(changes);
        } else {
            mail.changes.extend(changes);
        }
    }

    /// The block at `position` in the one region, and in the one of the two that
    /// holds it.
    fn blocks_at(&self, position: BlockPos) -> (Option<BlockState>, Option<BlockState>) {
        let holder = if position.x < 16 {
            &self.west
        } else {
            &self.east
        };
        (self.whole.own_block(position), holder.own_block(position))
    }

    /// The first tick in which the block at `position` changed, in the one region and
    /// in the two.
    fn first_change(&self, position: BlockPos) -> (Option<u64>, Option<u64>) {
        let first = |log: &[(u64, BlockPos, BlockState)]| {
            log.iter()
                .find(|(_, at, _)| *at == position)
                .map(|(tick, _, _)| *tick)
        };
        (first(&self.whole_log), first(&self.divided_log))
    }

    /// Asserts that every block of the land near the floor is the same in the one
    /// region as in the one of the two that holds it.
    fn assert_settled_alike(&self) {
        for position in land_chunks() {
            let holder = if position.x <= 0 {
                &self.west
            } else {
                &self.east
            };
            let (whole, divided) = (
                self.whole.chunk(position).expect("loaded"),
                holder.chunk(position).expect("loaded"),
            );
            for y in -64..-40 {
                for x in 0..16 {
                    for z in 0..16 {
                        assert_eq!(
                            whole.get(x, y, z),
                            divided.get(x, y, z),
                            "at {x}, {y}, {z} of {position:?}, tick {}",
                            self.tick()
                        );
                    }
                }
            }
        }
        // And nothing of either is left waiting at the line.
        for region in [&self.west, &self.east] {
            for position in land_chunks() {
                assert_eq!(region.seams(position), [], "{position:?}");
            }
        }
    }
}

/// Water from a source on either side, flowing across the line both ways, in the
/// two regions as in one.
fn water_both_ways(mut world: Divided) -> Divided {
    world.set(at(13, 3), blocks::WATER);
    world.set(at(18, 12), blocks::WATER);
    world.steps(120);
    world.assert_settled_alike();
    let flowing = |state: Option<BlockState>| state.is_some_and(|s| s.property("level").is_some());
    for position in [at(16, 3), at(19, 3), at(15, 12), at(12, 12)] {
        let (whole, divided) = world.blocks_at(position);
        assert!(flowing(whole) && whole == divided, "{position:?}");
    }
    world
}

/// The first tick each block changed in, in the one region and in the two.
fn first_changes(world: &Divided) -> Vec<(BlockPos, u64, Option<u64>)> {
    let mut seen = BTreeSet::new();
    let mut firsts = Vec::new();
    for (_, position, _) in &world.whole_log {
        if seen.insert(*position) {
            let (whole, divided) = world.first_change(*position);
            firsts.push((*position, whole.expect("logged"), divided));
        }
    }
    firsts
}

/// Coupled, water crosses the line at its own pace, as in one region: the block it
/// spreads into across the line is set a tick late, by the holder, but its tick counts
/// from the tick the spread was made in, so every block beyond changes on the very
/// tick it does in one region.
#[test]
fn water_crosses_a_coupled_line_both_ways_on_the_ticks_it_would_in_one_region() {
    let world = water_both_ways(Divided::new(&[]));
    let firsts = first_changes(&world);
    for (position, whole, divided) in &firsts {
        let at_the_line = [15, 16].contains(&position.x);
        if at_the_line && *divided == Some(whole + 1) {
            continue;
        }
        assert_eq!(*divided, Some(*whole), "{position:?}");
    }
    // The spreads across were a tick late, both ways.
    for position in [at(16, 3), at(15, 12)] {
        let (whole, divided) = world.first_change(position);
        assert_eq!(
            divided,
            Some(whole.expect("water came") + 1),
            "{position:?}"
        );
    }
    for position in [at(17, 3), at(19, 3), at(14, 12), at(12, 12)] {
        let (whole, divided) = world.first_change(position);
        assert!(whole.is_some(), "{position:?}");
        assert_eq!(divided, whole, "{position:?}");
    }
}

/// Not coupled, what crosses counts from its taking: water settles alike, but each
/// block beyond the line changes a tick later than in one region.
#[test]
fn water_crosses_a_line_not_coupled_a_tick_late_and_settles_as_in_one_region() {
    let world = water_both_ways(Divided::uncoupled(&[]));
    for position in [
        at(16, 3),
        at(17, 3),
        at(19, 3),
        at(15, 12),
        at(14, 12),
        at(12, 12),
    ] {
        let (whole, divided) = world.first_change(position);
        assert_eq!(
            divided,
            Some(whole.expect("water came") + 1),
            "{position:?}"
        );
    }
}

/// The western water's spread is made from a copy that does not yet show the stone
/// the eastern region set in the same tick; the eastern region, running the target's
/// side, finds the stone and the water stays out, as in one region, where the stone
/// was there first.
#[test]
fn water_walled_off_across_the_line_before_its_spread_arrives_stays_out() {
    let source = at(13, 8);
    let wall: Vec<BlockPos> = (0..16).map(|z| at(16, z)).collect();
    // Learn the tick in which the western region first spreads across.
    let mut probe = Divided::new(&[]);
    probe.set(source, blocks::WATER);
    let mut spreads_in = None;
    for _ in 0..60 {
        probe.step();
        let (west, _) = probe.outputs.last().expect("ticked");
        if west
            .effects
            .iter()
            .any(|effect| matches!(effect.what, Effected::Spread { .. }))
        {
            spreads_in = Some(west.tick);
            break;
        }
    }
    let spreads_in = spreads_in.expect("the water reaches the line");

    let mut world = Divided::new(&[]);
    world.set(source, blocks::WATER);
    while world.tick() + 1 < spreads_in {
        world.step();
    }
    for position in &wall {
        world.set_in_the_next_tick(*position, blocks::STONE);
    }
    world.step();
    let (west, _) = world.outputs.last().expect("ticked");
    assert!(
        west.effects
            .iter()
            .any(|effect| matches!(effect.what, Effected::Spread { .. })),
        "the western region spread from a copy without the wall"
    );
    world.steps(60);
    world.assert_settled_alike();
    for position in &wall {
        assert_eq!(
            world.blocks_at(*position),
            (Some(blocks::STONE), Some(blocks::STONE))
        );
    }
    assert_eq!(
        world.blocks_at(at(17, 8)),
        (Some(blocks::AIR), Some(blocks::AIR))
    );
}

/// The block under a torch's support is broken on one side; the torch on the other
/// goes a tick later than in one region, where it goes at once.
#[test]
fn a_torch_whose_support_is_broken_across_the_line_goes_a_tick_later() {
    let (support, torch) = (at(15, 8), at(16, 8));
    let mut world = Divided::new(&[(support, blocks::STONE), (torch, wall_torch("east"))]);
    world.step();
    world.set(support, blocks::AIR);
    assert_eq!(
        world.blocks_at(torch),
        (Some(blocks::AIR), Some(wall_torch("east")))
    );
    world.step();
    let (west, _) = world.outputs.last().expect("ticked");
    assert!(west.effects.iter().any(|effect| effect.at == torch
        && matches!(
            effect.what,
            Effected::Update {
                kind: SeamKind::Shape,
                ..
            }
        )));
    assert_eq!(world.blocks_at(torch).1, Some(wall_torch("east")));
    world.step();
    assert_eq!(world.blocks_at(torch).1, Some(blocks::AIR));
    assert_eq!(world.first_change(torch), (Some(2), Some(3)));
    world.steps(5);
    world.assert_settled_alike();
}

/// A rail placed beside a rail across the line turns it towards itself, by an effect
/// that sets it only where it is still the rail the placing region read.
#[test]
fn a_rail_placed_at_the_line_turns_the_rail_across_it() {
    let (placed, across) = (at(15, 8), at(16, 8));
    let straight = state("minecraft:rail[shape=north_south]");
    let turned = state("minecraft:rail[shape=east_west]");
    let mut world = Divided::new(&[(across, straight)]);
    world.step();
    world.set(placed, straight);
    assert_eq!(world.blocks_at(placed), (Some(turned), Some(turned)));
    world.step();
    let (west, _) = world.outputs.last().expect("ticked");
    assert!(west.effects.contains(&Effect {
        at: across,
        what: Effected::SetIf {
            expected: straight,
            state: turned,
            flags: updates::UPDATE_ALL,
        },
    }));
    world.steps(3);
    assert_eq!(world.blocks_at(across), (Some(turned), Some(turned)));
    world.assert_settled_alike();
}

/// A set by an effect is not made where the block is no longer what the sender read.
#[test]
fn a_set_across_the_line_is_not_made_where_the_block_has_changed_since() {
    let (placed, across) = (at(15, 8), at(16, 8));
    let straight = state("minecraft:rail[shape=north_south]");
    let mut world = Divided::new(&[(across, straight)]);
    world.step();
    world.set(placed, straight);
    // The eastern region breaks its rail in the tick that takes the effect.
    let mut scratch = TickOutput::default();
    world.east.set_block(across, blocks::AIR, &mut scratch);
    world.steps(3);
    assert_eq!(world.blocks_at(across).1, Some(blocks::AIR));
}

/// A row of fences on both sides of the line, placed one by one, joins as in one
/// region, and lets go of a fence broken in its middle.
#[test]
fn a_fence_row_along_the_line_joins_as_in_one_region() {
    let mut world = Divided::new(&[]);
    for z in 2..9 {
        world.set(at(15, z), blocks::OAK_FENCE);
        world.step();
        world.set(at(16, z), blocks::OAK_FENCE);
        world.step();
    }
    world.steps(3);
    world.assert_settled_alike();
    world.set(at(16, 5), blocks::AIR);
    world.set(at(15, 6), blocks::AIR);
    world.steps(3);
    world.assert_settled_alike();
}

/// Leaves across the line from their log decay once it is gone, as in one region:
/// their distance crosses the line a tick late each time, and they go by the same
/// random ticks.
#[test]
fn leaves_across_the_line_decay_as_in_one_region() {
    let log = at(14, 8);
    let blocks = [
        (log, state("minecraft:oak_log")),
        (at(15, 8), leaves(1)),
        (at(16, 8), leaves(2)),
        (at(17, 8), leaves(3)),
        (at(17, 9), leaves(4)),
        (at(16, 9), leaves(3)),
        (at(15, 9), leaves(2)),
    ];
    let mut world = Divided::new(&blocks);
    world.step();
    world.set(log, blocks::AIR);
    let leafy = |region: &Region| {
        blocks[1..]
            .iter()
            .any(|(position, _)| region.own_block(*position) != Some(blocks::AIR))
    };
    for _ in 0..60_000 {
        if !leafy(&world.whole) && !leafy(&world.west) && !leafy(&world.east) {
            break;
        }
        world.step();
    }
    assert!(!leafy(&world.whole), "the leaves of one region decayed");
    world.assert_settled_alike();
    // The distance reached the far leaves later than in one region.
    let (whole, divided) = world.first_change(at(17, 9));
    // Coupled, the distance crosses the line on the tick it would in one region: the
    // update is a tick late, and the leaf's tick it schedules counts from where it was
    // made.
    assert!(whole.is_some() && divided.is_some());
}

/// The same inputs give the same outputs: two runs of water across the line put out
/// the same, tick for tick.
#[test]
fn regions_that_send_each_other_effects_do_the_same_run_twice() {
    let run = || {
        let mut world =
            Divided::new(&[(at(16, 8), wall_torch("east")), (at(15, 8), blocks::STONE)]);
        world.set(at(13, 3), blocks::WATER);
        world.set(at(18, 12), blocks::LAVA);
        world.step();
        world.set(at(15, 8), blocks::AIR);
        world.steps(80);
        world.outputs
    };
    let first = run();
    assert!(first.iter().any(|(west, _)| !west.effects.is_empty()));
    assert!(first.iter().any(|(_, east)| !east.taken.is_empty()));
    assert_eq!(first, run());
}

// ---------------------------------------------------------------------------------------
// When a scheduled tick sent across is due (section 8, "Exact delays")
// ---------------------------------------------------------------------------------------

/// Sand over air at x = 8 of chunk (1, 0) in a region that holds the eastern chunks,
/// after its first tick.
fn sand_east() -> (Region, BlockPos) {
    let sand = BlockPos::new(24, -55, 8);
    let land = land(&[(sand, blocks::SAND)]);
    let mut region = region_of(EASTERN, 2, &land, &column(0));
    idle_tick(&mut region);
    (region, sand)
}

fn sent_tick(id: u32, at: BlockPos, delay: i32, due: Due) -> FiledEffect {
    FiledEffect {
        id: EffectId {
            sequence: 7,
            index: id,
        },
        effect: Effect {
            at,
            what: Effected::Tick {
                of: sand_tick(),
                delay,
                priority: 0,
            },
        },
        due,
    }
}

/// The tick in which the sand fell, ticking the region on from its tick `taking` in
/// which `effect` is handed in.
fn falls_in(region: &mut Region, sand: BlockPos, effect: FiledEffect) -> u64 {
    let output = region.tick(&TickInputs {
        effects: vec![effect.clone()],
        ..TickInputs::default()
    });
    assert_eq!(output.taken, [effect.id]);
    let mut output = output;
    for _ in 0..10 {
        if changes_of(&output).contains(&(sand, blocks::AIR)) {
            return output.tick;
        }
        output = idle_tick(region);
    }
    panic!("the sand did not fall");
}

/// A tick sent from a coupled region is due at the tick of this region the store
/// converted it to; one from a region not coupled counts its delay from the taking;
/// one whose due tick has passed runs in the tick that takes it.
#[test]
fn a_tick_sent_across_is_due_as_converted_from_the_taking_or_at_once_if_past() {
    let (mut region, sand) = sand_east();
    let taking = region.tick_number() + 1;
    let due = Due::At { tick: taking + 3 };
    assert_eq!(
        falls_in(&mut region, sand, sent_tick(0, sand, 2, due)),
        taking + 3
    );

    let (mut region, sand) = sand_east();
    let taking = region.tick_number() + 1;
    let effect = sent_tick(1, sand, 2, Due::FromTaking);
    assert_eq!(falls_in(&mut region, sand, effect), taking + 2);

    let (mut region, sand) = sand_east();
    let taking = region.tick_number() + 1;
    let past = Due::At { tick: 0 };
    assert_eq!(
        falls_in(&mut region, sand, sent_tick(2, sand, 2, past)),
        taking
    );
}

/// A tick scheduled into the other region's land while the two are coupled runs on
/// the tick it would in one region.
#[test]
fn a_tick_sent_across_a_coupled_line_runs_on_the_tick_it_would_in_one_region() {
    let sand = BlockPos::new(18, -55, 8);
    let mut world = Divided::new(&[(sand, blocks::SAND)]);
    world.steps(2);
    let made = world.tick();
    world.whole.schedule_tick(sand, sand_tick(), 4, 0);
    world.west.schedule_tick(sand, sand_tick(), 4, 0);
    // Made in the western region's tick `made`, as a rule of that tick would.
    let effects = mem::take(&mut world.west.made);
    assert_eq!(effects.len(), 1);
    world.to_east.effects = effects.into_iter().map(|effect| (made, effect)).collect();
    world.steps(8);
    let (whole, divided) = world.first_change(sand);
    assert_eq!(whole, Some(made + 4));
    assert_eq!(divided, whole);
}

// ---------------------------------------------------------------------------------------
// Updates that cannot run yet, and seam entries sent on (sections 5 and 6)
// ---------------------------------------------------------------------------------------

/// An update sent to a block beside a chunk the region can read neither loaded nor
/// copied is kept as a seam entry in its own chunk, taken in that tick, and made once
/// the neighbour is copied.
#[test]
fn an_update_that_cannot_run_yet_waits_in_its_own_chunk_and_runs_once_it_can() {
    let torch = at(16, 8);
    let land = land(&[(torch, wall_torch("east"))]);
    let mut region = region_of(EASTERN, 2, &land, &[]);
    idle_tick(&mut region);
    let id = EffectId {
        sequence: 3,
        index: 0,
    };
    let update = Effect {
        at: torch,
        what: Effected::Update {
            direction: Face::West as u8,
            kind: SeamKind::Shape,
            source: blocks::AIR,
            flags: updates::UPDATE_CLIENTS,
            depth: updates::UPDATE_LIMIT - 1,
        },
    };
    let output = region.tick(&TickInputs {
        effects: vec![FiledEffect {
            id,
            effect: update,
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    });
    let line = ChunkPos::new(1, 0);
    assert_eq!(output.taken, [id]);
    assert_eq!(output.seams_changed, [line]);
    assert_eq!(region.seams(line).len(), 1);
    assert_eq!(region.seams(line)[0].at, torch);
    assert_eq!(region.own_block(torch), Some(wall_torch("east")));
    let output = idle_tick(&mut region);
    assert_eq!(output.taken, []);
    assert_eq!(region.seams(line).len(), 1, "still waiting");

    let output = region.tick(&TickInputs {
        copy_bases: vec![(ChunkPos::new(0, 0), floor())],
        ..TickInputs::default()
    });
    assert_eq!(region.own_block(torch), Some(blocks::AIR));
    assert_eq!(region.seams(line), []);
    assert_eq!(output.seams_changed, [line]);
}

/// The western region's update into land it has asked for waits as a seam entry; once
/// the store answers that another region holds the land, it is sent on as an effect.
#[test]
fn a_seam_entry_whose_block_comes_to_be_another_regions_is_sent_on() {
    let (wall, across) = (at(15, 8), at(16, 8));
    let land = land(&[(wall, blocks::STONE)]);
    let mut west = region_of(WESTERN, 1, &land, &[]);
    let beyond = ChunkPos::new(1, 0);
    checked(&mut west, &tickets(vec![beyond], vec![]));
    assert_eq!(west.knowledge(beyond), Knowledge::Asked);
    let mut scratch = TickOutput::default();
    west.set_block(wall, blocks::AIR, &mut scratch);
    let line = ChunkPos::new(0, 0);
    let reaching = |region: &Region, position: BlockPos| {
        let seams = region.seams(line);
        seams.iter().filter(|seam| seam.at == position).count()
    };
    // The neighbour told and its shape asked again; the updates of blocks of the
    // region's own beside the line wait as well, for want of the chunk across.
    assert_eq!(reaching(&west, across), 2);
    assert_eq!(reaching(&west, at(15, 7)), 2);
    let output = checked(&mut west, &answers(vec![], vec![(beyond, RegionId(2))]));
    assert_eq!(reaching(&west, across), 0);
    assert_eq!(reaching(&west, at(15, 7)), 2);
    assert_eq!(output.seams_changed, [line]);
    let sent: Vec<BlockPos> = output.effects.iter().map(|effect| effect.at).collect();
    assert_eq!(sent, [across, across]);
}

/// An update into a chunk the region knows nothing of and does not want is an effect,
/// and so is a scheduled tick, in the order made.
#[test]
fn what_a_region_does_in_land_nobody_has_told_it_of_and_it_does_not_want_is_an_effect() {
    let wall = at(15, 8);
    let land = land(&[(wall, blocks::STONE)]);
    let mut west = region_of(WESTERN, 1, &land, &column(1));
    idle_tick(&mut west);
    let mut scratch = TickOutput::default();
    west.set_block(wall, blocks::AIR, &mut scratch);
    west.schedule_tick(at(17, 8), sand_tick(), 2, 0);
    let output = idle_tick(&mut west);
    assert_eq!(west.seams(ChunkPos::new(0, 0)), []);
    let kinds: Vec<(BlockPos, &str)> = output
        .effects
        .iter()
        .map(|effect| {
            let kind = match effect.what {
                Effected::Update {
                    kind: SeamKind::Shape,
                    ..
                } => "shape",
                Effected::Update {
                    kind: SeamKind::Neighbour,
                    ..
                } => "neighbour",
                Effected::Tick { .. } => "tick",
                Effected::Spread { .. } => "spread",
                Effected::SetIf { .. } => "set",
                Effected::OnEntity { .. } => "on an entity",
                Effected::Exploded { .. } => "exploded",
                Effected::Burnt { .. } => "burnt",
                Effected::PlayerInside { .. } => "a player inside",
                Effected::Given { .. } => "given",
                Effected::Seated { .. } => "seated",
                Effected::OnPlayer { .. } => "on a player",
                Effected::Release { .. } => "a release",
            };
            (effect.at, kind)
        })
        .collect();
    // In the game's order: the neighbours told, then their shapes asked again.
    assert_eq!(
        kinds,
        [
            (at(16, 8), "neighbour"),
            (at(16, 8), "shape"),
            (at(17, 8), "tick")
        ]
    );
}

/// A region restored with its copies goes on as the region they were taken from: the
/// chunk beside them ticks in both, and its ticks run alike.
#[test]
fn a_region_restored_with_its_copies_runs_its_border_chunk_as_the_original_does() {
    let sand = BlockPos::new(15, -55, 8);
    let land = land(&[(sand, blocks::SAND)]);
    let mut original = region_of(WESTERN, 1, &land, &column(1));
    idle_tick(&mut original);
    original.schedule_tick(sand, sand_tick(), 3, 0);
    idle_tick(&mut original);
    let mut copy = super::scheduled_ticks::restored(&original);
    let line = ChunkPos::new(0, 0);
    let mut fell = None;
    for _ in 0..5 {
        let theirs = idle_tick(&mut original);
        let ours = idle_tick(&mut copy);
        assert_eq!(
            changes_of(&ours),
            changes_of(&theirs),
            "tick {}",
            theirs.tick
        );
        assert_eq!(copy.saved_chunk(line), original.saved_chunk(line));
        if changes_of(&theirs).contains(&(sand, blocks::AIR)) {
            fell = Some(theirs.tick);
        }
    }
    assert!(fell.is_some(), "the chunk beside the copies ticks");
}

/// An update kept for land the region was granted and has not loaded yet waits; once
/// the region gives the land back and does not want it, the entry is sent on, as it
/// would otherwise wait for good.
#[test]
fn a_seam_entry_whose_block_is_in_land_given_back_is_sent_on() {
    let (wall, across) = (at(15, 8), at(16, 8));
    let land = land(&[(wall, blocks::STONE)]);
    let mut west = region_of(WESTERN, 1, &land, &[]);
    let beyond = ChunkPos::new(1, 0);
    checked(&mut west, &tickets(vec![beyond], vec![]));
    checked(&mut west, &answers(vec![beyond], vec![]));
    assert_eq!(west.knowledge(beyond), Knowledge::Held);
    let mut scratch = TickOutput::default();
    west.set_block(wall, blocks::AIR, &mut scratch);
    let line = ChunkPos::new(0, 0);
    let reaching = |region: &Region| {
        let seams = region.seams(line);
        seams.iter().filter(|seam| seam.at == across).count()
    };
    assert_eq!(reaching(&west), 2);
    let output = checked(&mut west, &tickets(vec![], vec![beyond]));
    assert_eq!(output.returns, [beyond]);
    assert_eq!(output.effects, []);
    assert_eq!(reaching(&west), 2);
    let output = idle_tick(&mut west);
    assert_eq!(reaching(&west), 0);
    assert_eq!(output.seams_changed, [line]);
    let sent: Vec<BlockPos> = output.effects.iter().map(|effect| effect.at).collect();
    assert_eq!(sent, [across, across]);
}

/// A set sent across is made with its own flags: with 2, as a vine grows, no neighbour
/// is told, and a rail on the block taken away stays; with every update it goes.
#[test]
fn a_set_sent_across_is_made_with_the_flags_it_was_made_with() {
    let (under, rail) = (BlockPos::new(24, ON_FLOOR - 1, 8), at(24, 8));
    let straight = state("minecraft:rail[shape=north_south]");
    for (flags, stays) in [
        (updates::UPDATE_CLIENTS, true),
        (updates::UPDATE_ALL, false),
    ] {
        let land = land(&[(rail, straight)]);
        let mut east = region_of(EASTERN, 2, &land, &column(0));
        idle_tick(&mut east);
        let set = Effect {
            at: under,
            what: Effected::SetIf {
                expected: blocks::STONE,
                state: blocks::AIR,
                flags,
            },
        };
        east.tick(&TickInputs {
            effects: vec![FiledEffect {
                id: EffectId {
                    sequence: 1,
                    index: 0,
                },
                effect: set,
                due: Due::FromTaking,
            }],
            ..TickInputs::default()
        });
        assert_eq!(east.own_block(under), Some(blocks::AIR));
        let expected = if stays { straight } else { blocks::AIR };
        assert_eq!(east.own_block(rail), Some(expected), "flags {flags}");
    }
}

/// The world's features of the seed 13579, as a server's regions are given them.
fn features() -> crate::Features {
    crate::Features(Some(std::sync::Arc::new(
        clustine_features::FeatureSettings::new(13579, clustine_features::Dimension::Overworld)
            .expect("the pinned jar's features are read whole"),
    )))
}

/// The plan of N8, section 7.2: a tree a region grows in its own land is set through its
/// set with the tree's flags, its logs and leaves as the feature places them.
#[test]
fn a_tree_placed_in_the_region_s_land_stands_there() {
    let sapling = at(8, 8);
    let land = land(&[(BlockPos::new(8, ON_FLOOR - 1, 8), blocks::DIRT)]);
    let mut west = region_of(WESTERN, 1, &land, &column(1));
    west.config.features = features();
    let mut output = TickOutput::default();
    assert!(west.place_feature("minecraft:oak", sapling, 19, 7, &mut output));
    assert!(
        west.block(sapling)
            .is_some_and(|state| clustine_blocks::data::is_block(state, blocks::OAK_LOG))
    );
    assert!(output.effects.is_empty(), "nothing across the line");
}

/// The plan of N8, sections 7.2 and 7.4: a tree beside a pinned line sets what it places
/// beyond it by `SetIf` effects, one a cell, expecting what the region's copy holds there.
#[test]
fn a_tree_beside_a_line_sends_what_it_places_beyond_it_as_effects() {
    let sapling = at(15, 8);
    let land = land(&[(BlockPos::new(15, ON_FLOOR - 1, 8), blocks::DIRT)]);
    let mut west = region_of(WESTERN, 1, &land, &column(1));
    west.config.features = features();
    let mut output = TickOutput::default();
    assert!(west.place_feature("minecraft:oak", sapling, 19, 7, &mut output));
    let across: Vec<&Effect> = west
        .made
        .iter()
        .filter(|effect| matches!(effect.what, Effected::SetIf { .. }))
        .collect();
    assert!(!across.is_empty(), "the leaves beyond the line");
    let mut cells: Vec<BlockPos> = across.iter().map(|effect| effect.at).collect();
    cells.dedup();
    assert_eq!(cells.len(), across.len(), "one effect a cell");
    for effect in across {
        assert_eq!(effect.at.chunk().x, 1, "{:?} is beyond the line", effect.at);
        let Effected::SetIf {
            expected, flags, ..
        } = effect.what
        else {
            unreachable!()
        };
        assert_eq!(expected, blocks::AIR);
        assert_eq!(flags, 19);
    }
}
