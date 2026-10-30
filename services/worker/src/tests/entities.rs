//! Tests of what the runner makes durable of entities other than players, written from
//! `docs/adr/0028-entities.md`, section 3, and `docs/groundwork/n2-plan.md`, sections 6
//! and 11: a tick's entity changes go into its commit, in their order, and make it
//! needed; they mark only the entities of their chunk unsaved, and its blocks too
//! where the chunk is still as generated; a save names the parts that changed, and the
//! checkpoint at a stop every part; and a change for a chunk the region has not loaded
//! is refused. The sim of this step makes no entity changes, so a test has the runner's
//! next tick make them (`injected_entity_changes`).

use clustine_world::{
    ChunkGenerator, Entity, EntityChange, EntityData, EntityField, EntityId, ItemStack,
};

use super::*;

/// The chunk the tests watch, in the western stripe, away from the home chunk.
const K: ChunkPos = ChunkPos::new(-3, 0);

fn item(key: u64, x: f64, z: f64) -> Entity {
    Entity {
        portal: Default::default(),
        key: EntityId(key),
        kind: 71,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position: [x, -60.0, z],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        data: EntityData::Item {
            stack: ItemStack::new(1, 1),
            pick_up_delay: 40,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: -1,
        riding: Default::default(),
    }
}

fn lying() -> Entity {
    item(7, -40.5, 3.5)
}

/// The world of [`Divided::stripes`], with [`K`] stored changed, so that it is no
/// longer as generated, if `stored`.
fn world(stored: bool) -> Divided {
    let world = Divided::stripes();
    if stored {
        let (handle, _) = world.open(RegionId(0), 1);
        let mut chunk = clustine_worldgen::FlatGenerator::classic().generate(K);
        chunk.set(1, 100, 1, clustine_data::blocks::STONE);
        handle.request(StoreRequest::Save {
            position: K,
            tick: 0,
            chunk,
            parts: Parts::ALL,
        });
        handle.flush();
    }
    world
}

/// A gated runner of the western stripe, with an edge linked to it, that has loaded
/// [`K`].
async fn with_k_loaded(world: &Divided) -> (RegionRunner, Arc<GateControl>, TestEdge) {
    let (runner, gate) = gated_as(&world.store, world.hello(RegionId(0), 2), settings(0));
    let mut runner = runner.with_id(RegionId(0));
    let (edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    // The runner checkpoints only where a test asks it to.
    runner.checkpoint_interval = 1_000_000;
    edge.send(asking_for(vec![K])).await.unwrap();
    step_until(&mut runner, |runner| runner.region().chunk(K).is_some());
    (runner, gate, edge)
}

/// The entity changes of the commits the runner asked for, by tick.
fn committed_entity_changes(gate: &GateControl) -> Vec<(u64, Vec<(ChunkPos, EntityChange)>)> {
    gate.committed
        .lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            StoreRequest::Commit {
                tick,
                entity_changes,
                ..
            } if !entity_changes.is_empty() => Some((*tick, entity_changes.clone())),
            _ => None,
        })
        .collect()
}

/// Has the runner's next tick make `changes`, and steps until it has committed them.
/// Returns the tick.
fn make(
    runner: &mut RegionRunner,
    gate: &GateControl,
    changes: Vec<(ChunkPos, EntityChange)>,
) -> u64 {
    let before = committed_entity_changes(gate).len();
    runner.injected_entity_changes = changes.clone();
    step_until(runner, |_| committed_entity_changes(gate).len() > before);
    let (tick, committed) = committed_entity_changes(gate)
        .pop()
        .expect("stepped until there was one");
    assert_eq!(committed, changes, "the commit carries them in their order");
    tick
}

fn saved_parts(gate: &GateControl, position: ChunkPos) -> Vec<Parts> {
    gate.saved_parts
        .lock()
        .unwrap()
        .iter()
        .filter(|(at, _)| *at == position)
        .map(|(_, parts)| *parts)
        .collect()
}

/// A tick that changes entities and nothing else is committed with its changes in
/// their order, and a region opened again from the store without a save in between
/// has them: a crash after the commit loses none.
#[tokio::test]
async fn a_ticks_entity_changes_are_committed_with_it_and_a_restore_has_them() {
    let world = world(true);
    let (mut runner, gate, _edge) = with_k_loaded(&world).await;
    let second = item(8, -35.0, 9.25);
    let moved = EntityField::Position([-39.5, -60.0, 3.5]);
    make(
        &mut runner,
        &gate,
        vec![
            (K, EntityChange::Whole(lying())),
            (K, EntityChange::Whole(second.clone())),
            (K, EntityChange::Set(lying().key, vec![moved.clone()])),
        ],
    );
    make(
        &mut runner,
        &gate,
        vec![(K, EntityChange::Gone(second.key))],
    );
    assert!(gate.saved_parts.lock().unwrap().is_empty());

    drop(runner);
    let (handle, _) = world.open(RegionId(0), 3);
    let mut expected = lying();
    moved.set(&mut expected);
    assert_eq!(stored_chunk(&handle, K).entities(), [expected]);
}

/// An entity change marks only the entities of a stored chunk unsaved. The chunk's
/// first save writes every part all the same, so that its two files are of one save;
/// a later one writes the entities alone, and the checkpoint at a stop every part.
#[tokio::test]
async fn an_entity_change_marks_only_the_entities_unsaved_and_a_stop_saves_every_part() {
    let world = world(true);
    let (mut runner, gate, _edge) = with_k_loaded(&world).await;
    make(&mut runner, &gate, vec![(K, EntityChange::Whole(lying()))]);
    assert_eq!(runner.unsaved.get(&K), Some(&Parts::ENTITIES));
    runner.checkpoint();
    assert_eq!(saved_parts(&gate, K), [Parts::ALL]);
    assert!(runner.unsaved.is_empty());

    let aged = |age| EntityChange::Set(lying().key, vec![EntityField::Lived(age)]);
    make(&mut runner, &gate, vec![(K, aged(9))]);
    runner.checkpoint();
    assert_eq!(saved_parts(&gate, K), [Parts::ALL, Parts::ENTITIES]);

    make(&mut runner, &gate, vec![(K, aged(10))]);
    runner.checkpoint_at_stop();
    assert_eq!(
        saved_parts(&gate, K),
        [Parts::ALL, Parts::ENTITIES, Parts::ALL]
    );
}

/// An entity change to a chunk still as generated marks only its entities unsaved: the
/// runner's first save of the chunk is whole, and the store writes a chunk not stored
/// yet whole whatever is named; after that an animal's step does not rewrite its blocks,
/// and the generated mark stays (`docs/groundwork/n7l-plan.md`, section 9 item 5).
#[tokio::test]
async fn an_entity_change_to_a_chunk_as_generated_saves_its_blocks_only_the_first_time() {
    let world = world(false);
    let (mut runner, gate, _edge) = with_k_loaded(&world).await;
    assert_ne!(runner.region().chunk(K).unwrap().generated(), 0);
    make(&mut runner, &gate, vec![(K, EntityChange::Whole(lying()))]);
    assert_eq!(runner.unsaved.get(&K), Some(&Parts::ENTITIES));
    runner.checkpoint();
    assert_eq!(saved_parts(&gate, K), [Parts::ALL]);
    make(
        &mut runner,
        &gate,
        vec![(K, EntityChange::Gone(lying().key))],
    );
    runner.checkpoint();
    assert_eq!(saved_parts(&gate, K), [Parts::ALL, Parts::ENTITIES]);
    assert_ne!(runner.region().chunk(K).unwrap().generated(), 0);
}

/// The sim makes no entity change for a chunk it has not loaded, and the runner holds
/// it to that: such a change would be committed for a chunk whose saves the region
/// does not make.
#[tokio::test]
#[should_panic(expected = "which is not loaded")]
async fn an_entity_change_for_a_chunk_not_loaded_is_refused() {
    let world = world(true);
    let (mut runner, gate, _edge) = with_k_loaded(&world).await;
    let far = ChunkPos::new(-30, 30);
    make(
        &mut runner,
        &gate,
        vec![(far, EntityChange::Gone(EntityId(1)))],
    );
}

/// An item that lies in a chunk that ticks ages without a commit, and its age reaches
/// the store at the next checkpoint and at the region's stop all the same, so that a
/// restart, a merge or a split does not make it young again
/// (`docs/groundwork/n4-plan.md`, section 7).
#[tokio::test]
async fn an_item_that_lies_and_commits_nothing_has_its_age_saved_at_a_checkpoint_and_at_a_stop() {
    // An item lies in K as the store has it.
    let world = Divided::stripes();
    {
        let (handle, _) = world.open(RegionId(0), 1);
        let mut chunk = clustine_worldgen::FlatGenerator::classic().generate(K);
        chunk.set(1, 100, 1, clustine_data::blocks::STONE);
        chunk.set_entities(vec![lying()]);
        handle.request(StoreRequest::Save {
            position: K,
            tick: 0,
            chunk,
            parts: Parts::ALL,
        });
        handle.flush();
    }
    let (runner, gate) = gated_as(&world.store, world.hello(RegionId(0), 2), settings(0));
    let mut runner = runner.with_id(RegionId(0));
    let (edge, worker_end) = in_process(256);
    runner.links().attach(worker_end);
    runner.checkpoint_interval = 1_000_000;
    // K and its neighbours, so that K ticks.
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|dx| (-1..=1).map(move |dz| ChunkPos::new(K.x + dx, K.z + dz)))
        .collect();
    edge.send(asking_for(around.clone())).await.unwrap();
    step_until(&mut runner, |runner| {
        around
            .iter()
            .all(|position| runner.region().chunk(*position).is_some())
    });
    let age_in = |runner: &RegionRunner| runner.region().chunk(K).unwrap().entities()[0].lived;
    step_until(&mut runner, |runner| age_in(runner) >= 30);
    let committed = committed_entity_changes(&gate).len();
    runner.checkpoint();
    assert_eq!(
        saved_parts(&gate, K),
        [Parts::ALL],
        "its first save since it was loaded, which is of every part"
    );
    step_until(&mut runner, |runner| age_in(runner) >= 60);
    runner.checkpoint();
    assert_eq!(saved_parts(&gate, K).last(), Some(&Parts::ENTITIES));
    step_until(&mut runner, |runner| age_in(runner) >= 90);
    let aged = age_in(&runner);
    runner.checkpoint_at_stop();
    assert_eq!(
        committed_entity_changes(&gate).len(),
        committed,
        "the item's ageing made no commit"
    );
    drop(runner);
    let (handle, _) = world.open(RegionId(0), 3);
    assert_eq!(stored_chunk(&handle, K).entities()[0].lived, aged);
}
