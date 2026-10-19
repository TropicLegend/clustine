//! Tests of dispensers, droppers and hoppers at a pinned line in step N6, written from
//! `docs/groundwork/n6-plan.md` (decision 10, section 6 item 4, section 7 item 14,
//! section 9 item 3, section 10 items 1, 5, 6 and 7), `docs/adr/0027-lending-at-a-line.md`
//! (sections 2 to 4), the porting brief `docs/groundwork/n6-brief.md` (11.4 and 11.5) and
//! the public API alone, by someone who did not write the code: one placed facing across
//! a line asks for the chunk it faces; one standing in a lent chunk keeps no lend by
//! standing there; a lender waits while one of its own players has a menu open on a
//! container in the chunk asked for; and what one does facing across a line it holds no
//! lend of.
//!
//! The line is at block x = 16 (chunk x = 1): the west region is pinned west of it, the
//! east region east of it, each with copies of the other's border chunks. Every player
//! of Clustine is a creative one.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, entity_types, items};
use clustine_sim::api::{ClickKind, Face, MenuKind, PlayerInput};
use clustine_sim::{LendAnswer, LendAsked, Region, TickInputs, TickOutput, Ticket};
use clustine_world::{BlockPos, ChunkPos, EntityData, EntityIds, Vec3};
use n6_entities::*;
use n6_scene::*;

const BY_THE_LINE: Vec3 = Vec3::new(14.5, -60.0, 2.5);
const THIS_SIDE: BlockPos = BlockPos::new(15, -60, 4);
const ACROSS: BlockPos = BlockPos::new(16, -60, 4);
const BEYOND: ChunkPos = ChunkPos::new(1, 0);

/// The lends a region asks for in `outputs`.
fn lends_asked(outputs: &[TickOutput]) -> Vec<ChunkPos> {
    let mut asked: Vec<ChunkPos> = outputs
        .iter()
        .flat_map(|output| output.lends.iter().copied())
        .collect();
    asked.sort();
    asked.dedup();
    asked
}

fn hotbar_slot(kind: MenuKind, n: usize) -> i16 {
    i16::try_from(kind.container_slots() + 27 + n).expect("fits")
}

// ------------------------------------------------------------------------------------
// Asking (plan section 6 item 4, section 7 item 14; ADR-0027 section 2).

/// Section 6 item 4 and section 7 item 14: a dropper is a circuit's part reaching the
/// block it faces: one placed by the line facing across it (the player looking west)
/// asks for the chunk beyond.
#[test]
fn a_dropper_placed_facing_across_a_pinned_line_asks_for_the_chunk_it_faces() {
    let mut scene = Scene::new(
        Layout::side(true),
        &World::default(),
        inventory(&[(0, stack(items::DROPPER, 64))]),
        BY_THE_LINE,
    );
    scene.input(PlayerInput::Move {
        position: None,
        rotation: Some((90.0, 0.0)),
        on_ground: true,
    });
    let mut outputs = vec![scene.click(THIS_SIDE.offset(0, -1, 0), Face::Top)];
    outputs.extend(scene.idle_for(2));
    assert_eq!(
        scene.block(THIS_SIDE),
        state("minecraft:dropper[facing=east,triggered=false]")
    );
    assert_eq!(lends_asked(&outputs), vec![BEYOND]);
}

// A hopper faces the block whose face was clicked, so one facing across the line from
// this side is placed by a click on a block across it, which the region passes on as a
// remote action (W6): the dropper above stands for the hoppers' reach in the block faced.

// ------------------------------------------------------------------------------------
// Giving back (plan section 10 item 1; ADR-0027 section 4).

/// A west region that holds the east's chunk (1, 0) by a lend, with `world` in it and in
/// its own land, loaded; and the outputs of the ticks that loaded it.
fn borrower(world: &World) -> (Region, Vec<TickOutput>) {
    let side = Layout::side(true);
    let mut held = side.held.clone();
    held.push(BEYOND);
    let mut loaded = side.loaded.clone();
    loaded.push(BEYOND);
    let others: Vec<ChunkPos> = side
        .foreign
        .iter()
        .copied()
        .filter(|chunk| *chunk != BEYOND)
        .collect();
    let holdings = clustine_sim::Holdings {
        held,
        pinned: vec![side.pinned],
        lent: vec![BEYOND],
        lent_away: Vec::new(),
    };
    let mut state = clustine_sim::RegionState::new(EntityIds::block(1).expect("a block"));
    state.edges.insert(
        EDGE,
        clustine_sim::EdgeState {
            start: 1,
            ..clustine_sim::EdgeState::default()
        },
    );
    let mut region = Region::restore(config(inventory(&[])), state, holdings);
    let mut outputs = vec![
        region.tick(&TickInputs {
            tickets_added: loaded
                .iter()
                .chain(&others)
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            ..TickInputs::default()
        }),
    ];
    outputs.push(
        region.tick(&TickInputs {
            chunks_loaded: loaded
                .iter()
                .map(|chunk| (*chunk, world.chunk(*chunk)))
                .collect(),
            copy_bases: others
                .iter()
                .map(|chunk| (*chunk, world.chunk(*chunk)))
                .collect(),
            foreign: others.iter().map(|chunk| (*chunk, OTHER)).collect(),
            ..TickInputs::default()
        }),
    );
    assert!(region.chunk(BEYOND).is_some(), "the lent chunk is loaded");
    for _ in 0..10 {
        outputs.push(region.tick(&TickInputs::default()));
    }
    (region, outputs)
}

fn gave_back(outputs: &[TickOutput]) -> bool {
    outputs
        .iter()
        .any(|output| output.returns.contains(&BEYOND))
}

/// Section 10 item 1: a dispenser keeps a lend only by its reach leaving its own chunk,
/// never by standing in it: one in the lent chunk facing east, into the lent chunk
/// itself, keeps nothing, and the chunk is given back once it is loaded (ADR-0027 section
/// 4, "once its lent chunks are loaded after a restore").
#[test]
fn a_dispenser_standing_in_a_lent_chunk_keeps_no_lend_by_standing_there() {
    let world = World::of(&[(
        BlockPos::new(20, -60, 4),
        state("minecraft:dispenser[facing=east,triggered=false]"),
    )]);
    let (_, outputs) = borrower(&world);
    assert!(gave_back(&outputs), "given back");
}

/// The controls of the one before, by ADR-0027 section 4: a dispenser in the borrower's
/// own land facing into the lent chunk keeps it, and so does dust in it, a circuit's part
/// that keeps a lend by lying in the chunk.
#[test]
fn a_dispenser_facing_into_a_lent_chunk_or_dust_in_it_keeps_the_lend() {
    for (what, at, block) in [
        (
            "a dispenser facing into it",
            THIS_SIDE,
            state("minecraft:dispenser[facing=east,triggered=false]"),
        ),
        (
            "dust in it",
            BlockPos::new(20, -60, 4),
            clustine_data::blocks::REDSTONE_WIRE,
        ),
    ] {
        let (mut region, mut outputs) = borrower(&World::of(&[(at, block)]));
        for _ in 0..30 {
            outputs.push(region.tick(&TickInputs::default()));
        }
        assert!(!gave_back(&outputs), "{what} keeps it");
    }
}

// ------------------------------------------------------------------------------------
// The lender's wait (plan section 9 item 3, section 10 item 6; ADR-0027 section 3).

/// The east's chunk asked for, the west's neighbour across the line.
const ASKED: ChunkPos = ChunkPos::new(1, 0);
/// Where player 1 of the east stands: in chunk (1, -1), beside the one asked for.
const BESIDE_ASKED: Vec3 = Vec3::new(17.5, -60.0, -1.5);

fn ask(scene: &mut Scene, lend: u64) -> Vec<LendAnswer> {
    scene
        .region
        .tick(&TickInputs {
            lends_asked: vec![LendAsked {
                lend,
                chunks: vec![ASKED],
                to: OTHER,
            }],
            ..TickInputs::default()
        })
        .lend_answers
}

fn wait(lend: u64) -> Vec<LendAnswer> {
    vec![LendAnswer::Refused {
        lend,
        chunks: vec![ASKED],
        again: true,
    }]
}

fn lending(lend: u64) -> Vec<LendAnswer> {
    vec![LendAnswer::Lending {
        lend,
        chunks: vec![ASKED],
    }]
}

/// Section 9 item 3 (third bullet): the lender waits before lending a chunk in which one
/// of its players' menus shows a container, so that a lend never closes it; once the
/// menu is closed the chunk is lent.
#[test]
fn a_lender_waits_while_its_player_has_a_chest_open_in_the_chunk_asked_for() {
    let chest = BlockPos::new(17, -60, 0);
    let world = World::of(&[(chest, state("minecraft:chest[facing=north,type=single]"))]);
    let mut scene = Scene::new(Layout::side(false), &world, inventory(&[]), BESIDE_ASKED);
    let (serial, output) = scene.open(chest);
    assert_eq!(the_opened(&output, player(1)).kind, MenuKind::Rows3);
    assert_eq!(ask(&mut scene, 1), wait(1), "a menu open on it");
    assert_eq!(
        closed(&scene.idle(), player(1)),
        vec![],
        "the menu stays open"
    );
    scene.close(serial);
    assert_eq!(ask(&mut scene, 2), lending(2), "lent once it is closed");
}

/// Section 10 item 6: the wait covers either half of a double chest whose block is in
/// the asked chunk: a player at the half in chunk (1, -1) holds back the lend of the
/// chunk with the other half.
#[test]
fn a_lender_waits_while_its_player_has_open_a_double_chest_with_a_half_in_the_chunk() {
    let near = BlockPos::new(17, -60, -1);
    let far = BlockPos::new(17, -60, 0);
    let world = World::of(&[
        (near, state("minecraft:chest[facing=east,type=left]")),
        (far, state("minecraft:chest[facing=east,type=right]")),
    ]);
    let mut scene = Scene::new(
        Layout::side(false),
        &world,
        inventory(&[]),
        Vec3::new(18.5, -60.0, -2.5),
    );
    let (serial, output) = scene.open(near);
    assert_eq!(the_opened(&output, player(1)).kind, MenuKind::Rows6);
    assert_eq!(ask(&mut scene, 1), wait(1));
    scene.close(serial);
    assert_eq!(ask(&mut scene, 2), lending(2));
}

/// Section 10 item 6: the wait covers a container vehicle whose entity is in the asked
/// chunk: a chest cart there, open from the chunk beside.
#[test]
fn a_lender_waits_while_its_player_has_a_chest_cart_open_in_the_chunk() {
    let world = World::default().with_entities(&[cart_of(
        1,
        entity_types::CHEST_MINECART,
        [17.5, -60.0, 0.5],
        Vec::new(),
    )]);
    let mut scene = Scene::new(Layout::side(false), &world, inventory(&[]), BESIDE_ASKED);
    let serial = scene.next_input(0);
    let output = scene.input(PlayerInput::Interact {
        entity: key(1),
        location: [0.0, 0.5, 0.0],
        sneaking: false,
    });
    assert_eq!(the_opened(&output, player(1)).kind, MenuKind::Rows3);
    assert_eq!(ask(&mut scene, 1), wait(1));
    scene.close(serial);
    assert_eq!(ask(&mut scene, 2), lending(2));
}

// ------------------------------------------------------------------------------------
// Facing across a line with no lend (plan decision 10, section 6 item 4; brief 11.4,
// 11.5).

/// Player 1 of the west by the line, with 64 dirt in hotbar slot 0 and redstone blocks
/// in slot 1, beside `block` at [`THIS_SIDE`] facing east, and `more`.
fn at_the_line(block: &str, more: &[(BlockPos, BlockState)], held: i32) -> Scene {
    let mut blocks = vec![(THIS_SIDE, state(block))];
    blocks.extend_from_slice(more);
    Scene::new(
        Layout::side(true),
        &World::of(&blocks),
        inventory(&[(0, stack(held, 64)), (1, stack(items::REDSTONE_BLOCK, 64))]),
        BY_THE_LINE,
    )
}

/// Opens the container at `at`, shift-clicks hotbar slot 0 into it and closes it.
fn fill(scene: &mut Scene, at: BlockPos, kind: MenuKind) {
    let (serial, output) = scene.open(at);
    assert_eq!(the_opened(&output, player(1)).kind, kind);
    scene.menu_click(serial, hotbar_slot(kind, 0), 0, ClickKind::QuickMove);
    scene.close(serial);
}

/// What the container at `at` holds, as its menu shows it.
fn holds_at(scene: &mut Scene, at: BlockPos) -> Vec<Option<clustine_sim::api::ItemStack>> {
    let (serial, output) = scene.open(at);
    let opened = the_opened(&output, player(1));
    scene.close(serial);
    opened.slots[..opened.kind.container_slots()].to_vec()
}

/// Places a redstone block on the floor north of [`THIS_SIDE`], which powers it.
fn power(scene: &mut Scene) -> TickOutput {
    scene.select(1);
    let output = scene.click(THIS_SIDE.offset(0, -1, -1), Face::Top);
    assert_eq!(
        scene.block(THIS_SIDE.offset(0, 0, -1)),
        clustine_data::blocks::REDSTONE_BLOCK
    );
    output
}

/// Section 6 item 4: a drop made in a foreign block is made in the dispenser's region and
/// crosses as an arrival: a dispenser by the line facing across it drops its item at 0.7
/// out, beyond the line, which the tick sends on.
#[test]
fn a_dispensed_item_across_the_line_is_made_here_and_sent_on() {
    let mut scene = at_the_line(
        "minecraft:dispenser[facing=east,triggered=false]",
        &[],
        items::DIRT,
    );
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene)];
    outputs.extend(scene.idle_for(10));
    let sent: Vec<_> = outputs
        .iter()
        .flat_map(|output| output.arrivals.iter().cloned())
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, BEYOND);
    match &sent[0].1.data {
        EntityData::Item { stack: sent, .. } => assert_eq!(*sent, stack(items::DIRT, 1)),
        other => panic!("not an item: {other:?}"),
    }
    assert!(items_lying(&scene.all()).is_empty(), "none left here");
    assert_eq!(
        holds_at(&mut scene, THIS_SIDE)[0],
        Some(stack(items::DIRT, 63))
    );
}

/// Section 6 item 4: primed TNT made in a foreign block crosses as an arrival.
#[test]
fn tnt_dispensed_across_the_line_is_primed_here_and_sent_on() {
    let mut scene = at_the_line(
        "minecraft:dispenser[facing=east,triggered=false]",
        &[],
        items::TNT,
    );
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene)];
    outputs.extend(scene.idle_for(10));
    let sent: Vec<_> = outputs
        .iter()
        .flat_map(|output| output.arrivals.iter().cloned())
        .collect();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, BEYOND);
    assert_eq!(i32::from(sent[0].1.kind), entity_types::TNT);
    assert!(scene.of_kind(entity_types::TNT).is_empty());
}

/// Decision 10 and section 6 item 4 ("waits is kept only for what is out of reach
/// still"), brief 11.4: a dropper facing a chest across a line it holds no lend of puts
/// nothing into it and keeps its item, neither losing nor doubling it.
#[test]
fn a_dropper_facing_a_chest_across_the_line_keeps_its_item() {
    let mut scene = at_the_line(
        "minecraft:dropper[facing=east,triggered=false]",
        &[(ACROSS, state("minecraft:chest[facing=north,type=single]"))],
        items::DIRT,
    );
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene)];
    outputs.extend(scene.idle_for(10));
    assert_eq!(
        holds_at(&mut scene, THIS_SIDE)[0],
        Some(stack(items::DIRT, 64))
    );
    assert!(items_lying(&scene.all()).is_empty());
    assert!(outputs.iter().all(|output| output.arrivals.is_empty()));
}

/// Decision 10, brief 11.5: a hopper facing a chest across a line it holds no lend of
/// moves nothing and keeps its items.
#[test]
fn a_hopper_facing_a_chest_across_the_line_keeps_its_items() {
    let mut scene = at_the_line(
        "minecraft:hopper[enabled=true,facing=east]",
        &[(ACROSS, state("minecraft:chest[facing=north,type=single]"))],
        items::DIRT,
    );
    fill(&mut scene, THIS_SIDE, MenuKind::Hopper);
    let outputs = scene.idle_for(40);
    assert_eq!(
        holds_at(&mut scene, THIS_SIDE)[0],
        Some(stack(items::DIRT, 64))
    );
    assert!(outputs.iter().all(|output| output.arrivals.is_empty()));
}

/// The control of the lender's waits: a menu open on a chest in the chunk beside the
/// one asked for holds nothing back.
#[test]
fn a_lender_lends_while_its_player_has_a_chest_open_beside_the_chunk_asked_for() {
    let chest = BlockPos::new(17, -60, -1);
    let world = World::of(&[(chest, state("minecraft:chest[facing=north,type=single]"))]);
    let mut scene = Scene::new(
        Layout::side(false),
        &world,
        inventory(&[]),
        Vec3::new(18.5, -60.0, -2.5),
    );
    let (_, output) = scene.open(chest);
    assert_eq!(the_opened(&output, player(1)).kind, MenuKind::Rows3);
    assert_eq!(ask(&mut scene, 1), lending(1));
}

// ------------------------------------------------------------------------------------
// With the lend (plan section 6 item 4; ADR-0027 section 4).

/// The borrower of [`borrower`] with player 1 in it by the line, holding dirt and
/// redstone blocks.
fn borrowing(world: &World) -> Scene {
    let (region, _) = borrower(world);
    let held = inventory(&[
        (0, stack(items::DIRT, 64)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ]);
    let mut scene = Scene {
        region,
        layout: Layout::side(true),
        inventory: held,
        actors: Vec::new(),
    };
    scene.join(1, BY_THE_LINE);
    scene
}

/// Section 6 item 4: with the chunk it faces lent, a dropper by the line puts its item
/// into the chest across the line as into any other.
#[test]
fn a_dropper_by_the_line_fills_a_chest_in_the_chunk_lent_to_it() {
    let world = World::of(&[
        (
            THIS_SIDE,
            state("minecraft:dropper[facing=east,triggered=false]"),
        ),
        (ACROSS, state("minecraft:chest[facing=north,type=single]")),
    ]);
    let mut scene = borrowing(&world);
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    power(&mut scene);
    scene.idle_for(8);
    assert_eq!(holds_at(&mut scene, ACROSS)[0], Some(stack(items::DIRT, 1)));
    assert_eq!(
        holds_at(&mut scene, THIS_SIDE)[0],
        Some(stack(items::DIRT, 63))
    );
}

/// ADR-0027 section 4 ("It looks whenever a block within reach of a lent chunk changes
/// from a circuit's part to something else, by a player"): the dropper that kept the lend
/// broken, the chunk is given back.
#[test]
fn breaking_the_dropper_that_kept_a_lend_gives_the_chunk_back() {
    let world = World::of(&[(
        THIS_SIDE,
        state("minecraft:dropper[facing=east,triggered=false]"),
    )]);
    let mut scene = borrowing(&world);
    let outputs = scene.idle_for(20);
    assert!(!gave_back(&outputs), "kept while the dropper stands");
    let mut outputs = vec![scene.dig(THIS_SIDE)];
    assert_eq!(scene.block(THIS_SIDE), clustine_data::blocks::AIR);
    outputs.extend(scene.idle_for(10));
    assert!(gave_back(&outputs), "given back once it is gone");
}

/// Brief 11.5 ("A hopper cart crossing carries its contents, `enabled` and nothing
/// else") and 5.9: a disabled hopper cart rolling over the line arrives disabled, with
/// what it holds.
#[test]
fn a_hopper_cart_crossing_the_line_carries_its_contents_and_whether_it_is_enabled() {
    let mut cart = cart_of(
        1,
        entity_types::HOPPER_MINECART,
        [14.5, -60.0, 8.5],
        vec![(0, stack(items::DIRT, 3))],
    );
    cart.motion = [0.4, 0.0, 0.0];
    if let EntityData::Minecart { enabled, .. } = &mut cart.data {
        *enabled = false;
    }
    let mut blocks = Vec::new();
    for x in 8..24 {
        blocks.push((
            BlockPos::new(x, -60, 8),
            state("minecraft:rail[shape=east_west]"),
        ));
    }
    let world = World::of(&blocks).with_entities(&[cart]);
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), BY_THE_LINE);
    let mut sent = Vec::new();
    for _ in 0..20 {
        let output = scene.idle();
        sent.extend(output.arrivals.iter().cloned());
        if !sent.is_empty() {
            break;
        }
    }
    assert_eq!(sent.len(), 1, "sent across");
    assert_eq!(sent[0].0, BEYOND);
    assert_eq!(contents(&sent[0].1), vec![(0, stack(items::DIRT, 3))]);
    match &sent[0].1.data {
        EntityData::Minecart { enabled, .. } => assert!(!enabled, "still disabled"),
        other => panic!("{other:?}"),
    }
}

/// Section 10 item 4: a shulker box facing across a line reads the copy for room to
/// open: with a stone block across the line in front of its lid it does not open; with
/// air there it does.
#[test]
fn a_shulker_box_facing_across_the_line_reads_the_copy_for_room_to_open() {
    for blocked in [true, false] {
        let mut blocks = vec![(THIS_SIDE, state("minecraft:shulker_box[facing=east]"))];
        if blocked {
            blocks.push((ACROSS, clustine_data::blocks::STONE));
        }
        let mut scene = Scene::new(
            Layout::side(true),
            &World::of(&blocks),
            inventory(&[]),
            BY_THE_LINE,
        );
        let (_, output) = scene.open(THIS_SIDE);
        let found = opened(&output, player(1));
        if blocked {
            assert!(found.is_empty(), "the stone across stops its lid");
        } else {
            assert_eq!(found.len(), 1, "room to open");
            assert_eq!(found[0].kind, MenuKind::ShulkerBox);
        }
    }
}

/// Brief 11.5 ("an item across the line is not taken until it or the hopper's region
/// holds it"): an item in the copy whose box reaches into a hopper's suck box is left
/// alone: the hopper takes nothing, and nothing of the item crosses.
#[test]
fn a_hopper_by_the_line_takes_no_item_of_the_copy() {
    let world = World::of(&[(
        THIS_SIDE,
        state("minecraft:hopper[enabled=true,facing=down]"),
    )])
    .with_entities(&[dirt(1, [16.05, -59.2, 4.5])]);
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), BY_THE_LINE);
    let outputs = scene.idle_for(30);
    assert_eq!(holds_at(&mut scene, THIS_SIDE), vec![None; 5]);
    assert_eq!(
        scene.region.copy(BEYOND).map(|copy| copy.entities().len()),
        Some(1),
        "the item is still the east's"
    );
    for output in &outputs {
        assert!(output.arrivals.is_empty());
        assert!(
            output
                .effects
                .iter()
                .all(|effect| !matches!(effect.what, clustine_world::Effected::OnEntity { .. }))
        );
    }
}

/// Brief 11.5 ("a cart across the line is not pulled from or pushed into"): a chest
/// cart of the copy in the box above a hopper by the line is skipped as if absent.
#[test]
fn a_hopper_by_the_line_pulls_nothing_from_a_cart_of_the_copy() {
    let world = World::of(&[(
        THIS_SIDE,
        state("minecraft:hopper[enabled=true,facing=down]"),
    )])
    .with_entities(&[cart_of(
        1,
        entity_types::CHEST_MINECART,
        [16.2, -59.0, 4.5],
        vec![(0, stack(items::DIRT, 5))],
    )]);
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), BY_THE_LINE);
    scene.idle_for(30);
    assert_eq!(holds_at(&mut scene, THIS_SIDE), vec![None; 5]);
    let copy = scene.region.copy(BEYOND).expect("the copy");
    assert_eq!(
        held_count(&copy.entities()[0]),
        5,
        "the cart's dirt is all there"
    );
}

/// Plan section 6 item 4 ("waits is kept only for what is out of reach still") and
/// brief 11.4: a dispenser of armour facing an armour stand across the line it holds no
/// lend of neither puts the helmet on the stand of its copy nor loses or doubles it: it
/// is still in the dispenser, or crossed as an item.
#[test]
fn a_dispenser_of_armour_facing_a_stand_across_the_line_keeps_or_sends_the_helmet() {
    let world = World::of(&[(
        THIS_SIDE,
        state("minecraft:dispenser[facing=east,triggered=false]"),
    )])
    .with_entities(&[stand(1, [16.5, -60.0, 4.5])]);
    let held = inventory(&[
        (0, stack(items::IRON_HELMET, 1)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ]);
    let mut scene = Scene::new(Layout::side(true), &world, held, BY_THE_LINE);
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene)];
    outputs.extend(scene.idle_for(10));
    assert!(
        outputs.iter().any(|output| level_events(output)
            .iter()
            .any(|(kind, at, _)| *at == THIS_SIDE && (*kind == 1000 || *kind == 1001))),
        "it fired"
    );
    let kept = holds_at(&mut scene, THIS_SIDE)
        .iter()
        .flatten()
        .filter(|held| held.item == items::IRON_HELMET)
        .count();
    let sent = outputs
        .iter()
        .flat_map(|output| output.arrivals.iter())
        .filter(|(_, entity)| {
            matches!(&entity.data, EntityData::Item { stack, .. } if stack.item == items::IRON_HELMET)
        })
        .count();
    assert_eq!(kept + sent, 1, "kept {kept}, sent {sent}");
    let copy = scene.region.copy(BEYOND).expect("the copy");
    match &copy.entities()[0].data {
        EntityData::ArmorStand { equipment, .. } => {
            assert!(equipment.is_empty(), "the copy's stand is not changed here")
        }
        other => panic!("{other:?}"),
    }
}

/// Plan section 6 item 4 and brief 11.4 ("most behaviours decide on what they read at
/// F ... so they cannot be split into a read here and a write there without the item
/// being lost or doubled"): a dispenser of a water bucket facing across a line it holds
/// no lend of keeps the full bucket and places no water there.
#[test]
fn a_dispenser_of_water_facing_across_the_line_keeps_its_full_bucket() {
    let mut scene = at_the_line(
        "minecraft:dispenser[facing=east,triggered=false]",
        &[],
        items::WATER_BUCKET,
    );
    fill(&mut scene, THIS_SIDE, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene)];
    outputs.extend(scene.idle_for(10));
    let slots = holds_at(&mut scene, THIS_SIDE);
    assert_eq!(slots[0], Some(stack(items::WATER_BUCKET, 1)), "{slots:?}");
    for output in &outputs {
        for effect in &output.effects {
            if let clustine_world::Effected::SetIf { state, .. } = effect.what {
                assert_ne!(state, clustine_data::blocks::WATER, "no water set across");
            }
        }
    }
}
