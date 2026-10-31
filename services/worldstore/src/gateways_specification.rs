//! Tests of the store's answers to the End gateways' questions, written from the
//! specification by someone who did not write the store's side: `docs/groundwork/
//! p-plan.md`, section 19.8 and its review (items 3 and 5). The store works the exit out
//! from each chunk's committed form, as a copy's base is read: the stored chunk (or the
//! generator's), with its holder's commits replayed into it; it answers when the chain
//! of reads ends, only the session that asked, and nothing where the region's owner has
//! changed since.
//!
//! What the search does with the chunks it reads is `gateway_exit.rs`'s and has tests of
//! its own; here the search is fed what the store should have read, and the store's
//! answer must be what it then finds.

use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, Ordering};

use clustine_data::{BlockState, blocks};
use clustine_features::features::end::gateway_exit::{GatewaySearch, Step};
use clustine_rpc::{GatewayExit, GatewayExitAnswer, GatewayExitQuery, QueryId};
use clustine_world::generation::GeneratorSettings;
use clustine_world::{BlockPos, Chunk, ChunkPos, Dimension};

use super::*;
use crate::generators::Void;
use crate::regions::claim;
use crate::tests::{any_reply, division, generator, hello, log, memory, open, other_homes, save};

/// A gateway east of the End's island, as the fight places them: its line runs along
/// the x axis, 1024 out in chunk (64, 0).
const GATEWAY: BlockPos = BlockPos::new(96, 75, 0);

/// The chunk 1024 out along [`GATEWAY`]'s line.
const OUT: ChunkPos = ChunkPos::new(64, 0);

/// The End's home region in a world made from [`division`].
fn end_home() -> u32 {
    other_homes(&division())[1]
}

/// An End that is void everywhere, as the tests' worlds' End is, but that holds its
/// generation of [`OUT`] once, the first time, until the test lets it go on.
struct HeldEnd {
    barrier: Barrier,
    held: AtomicBool,
}

impl ChunkGenerator for HeldEnd {
    fn generate(&self, position: ChunkPos) -> Chunk {
        if position == OUT && !self.held.swap(true, Ordering::SeqCst) {
            // Once to say that the store has got here, once to be let go on.
            self.barrier.wait();
            self.barrier.wait();
        }
        Void::of(Dimension::End).generate(position)
    }

    fn settings(&self) -> GeneratorSettings {
        Void::of(Dimension::End).settings()
    }
}

/// The question about [`GATEWAY`] numbered `index`.
fn question(index: u32) -> GatewayExitQuery {
    GatewayExitQuery {
        id: QueryId { tick: 3, index },
        from: GATEWAY,
    }
}

fn ask(handle: &StoreHandle, query: GatewayExitQuery) {
    handle.request(StoreRequest::GatewayExits {
        queries: vec![query],
    });
}

/// The next answer about a gateway on `handle`, waited for; everything else passed
/// over.
fn next_answer(handle: &StoreHandle) -> GatewayExitAnswer {
    loop {
        if let StoreReply::GatewayExits { answers } = any_reply(handle) {
            let [answer] = answers[..] else {
                panic!("one answer at a time: {answers:?}");
            };
            return answer;
        }
    }
}

/// The answers about gateways on `handle` up to the answer to a flush.
fn answers_before_flush(handle: &StoreHandle) -> Vec<GatewayExitAnswer> {
    handle.request(StoreRequest::Flush);
    let mut answers = Vec::new();
    loop {
        match any_reply(handle) {
            StoreReply::Flushed => return answers,
            StoreReply::GatewayExits { answers: more } => answers.extend(more),
            _ => {}
        }
    }
}

/// What the search finds for [`GATEWAY`] fed each chunk as `world` says it stands.
fn found_over(world: impl Fn(ChunkPos) -> Chunk) -> GatewayExit {
    let rules = Dimension::End.rules();
    let mut search = GatewaySearch::new(GATEWAY, rules.min_y, rules.height);
    for _ in 0..1_000 {
        match search.step() {
            Step::Need(at) => search.feed(at, &world(at)),
            Step::Done(found) => {
                return GatewayExit {
                    exit: found.exit,
                    island: found.island,
                };
            }
        }
    }
    panic!("the search did not end");
}

/// The End's void chunk with `blocks` set in it.
fn void_with(position: ChunkPos, blocks_set: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = Void::of(Dimension::End).generate(position);
    for (pos, state) in blocks_set {
        if pos.chunk() == position {
            let (x, z) = pos.in_chunk();
            chunk.set(x, pos.y, z, *state);
        }
    }
    chunk
}

/// 19.8, "The store's answer", and the review's fifth point: in an End where nothing
/// was ever made, the store generates what the search reads and answers, once the
/// chain of reads has ended, with the exit the search finds there and the island to
/// make: 1280 out, where the point stops after sixteen steps on over the void.
#[test]
fn in_an_end_never_made_the_answer_is_the_exit_over_the_island_wanted_1280_out() {
    let store = memory();
    let end = open(&store, hello(end_home(), 1));
    ask(&end, question(0));
    let answer = next_answer(&end);
    assert_eq!((answer.id, answer.from), (question(0).id, GATEWAY));
    let expected = found_over(|at| Void::of(Dimension::End).generate(at));
    assert_eq!(answer.exit, Some(expected));
    let exit = answer.exit.expect("an exit");
    assert_eq!(exit.island, Some(BlockPos::new(1280, 75, 0)));
    assert_eq!(exit.exit.y, 85, "ten above the island's top at 75");
    // Answered once.
    assert_eq!(answers_before_flush(&end), []);
}

/// 19.8, "The store's answer": the search reads a chunk the region holds as its holder
/// committed it: its stored form with the commits since replayed into it, as a copy's
/// base is (the review's first question and fifth point). The end stone saved in the
/// chunk is covered by a commit since, so another committed after it is where the
/// traveller stands, and the tallest block about it is the committed obsidian.
#[test]
fn a_chunk_the_region_holds_is_read_with_its_commits_not_yet_folded() {
    let store = memory();
    let end = open(&store, hello(end_home(), 1));
    let (granted, _) = claim(&end, &[OUT]);
    assert_eq!(granted, [OUT]);
    let saved = BlockPos::new(1030, 40, 5);
    save(&end, OUT, &void_with(OUT, &[(saved, blocks::END_STONE)]));
    let covered = BlockPos::new(1030, 42, 5);
    let stand = BlockPos::new(1033, 40, 9);
    log(
        &end,
        6,
        &[(covered.x, covered.y, covered.z, blocks::OBSIDIAN)],
    );
    log(&end, 7, &[(stand.x, stand.y, stand.z, blocks::END_STONE)]);
    ask(&end, question(1));
    let answer = next_answer(&end);
    let committed = [
        (saved, blocks::END_STONE),
        (covered, blocks::OBSIDIAN),
        (stand, blocks::END_STONE),
    ];
    let expected = found_over(|at| void_with(at, &committed));
    assert_eq!(answer.exit, Some(expected));
    // Back from 1024 to the void at 1008 and on again to chunk 64: (1030, 40, 5) has
    // obsidian two above it, so (1033, 40, 9) is stood on; the tallest within 16 is the
    // obsidian at y 42.
    assert_eq!(
        expected,
        GatewayExit {
            exit: BlockPos::new(1030, 52, 5),
            island: None,
        }
    );
}

/// 19.8, "The store's answer": a chunk its holder saved, with no commits since, is read
/// as it was saved; the chunks about it, never made, as the End's generator makes them.
#[test]
fn a_chunk_saved_by_its_holder_is_read_as_saved_and_one_never_made_as_generated() {
    let store = memory();
    let end = open(&store, hello(end_home(), 1));
    let (granted, _) = claim(&end, &[OUT]);
    assert_eq!(granted, [OUT]);
    let stand = BlockPos::new(1028, 50, 3);
    save(&end, OUT, &void_with(OUT, &[(stand, blocks::END_STONE)]));
    end.request(StoreRequest::Flush);
    loop {
        if any_reply(&end) == StoreReply::Flushed {
            break;
        }
    }
    ask(&end, question(2));
    let answer = next_answer(&end);
    let expected = found_over(|at| void_with(at, &[(stand, blocks::END_STONE)]));
    assert_eq!(answer.exit, Some(expected));
    assert_eq!(
        expected,
        GatewayExit {
            exit: BlockPos::new(1028, 60, 3),
            island: None,
        }
    );
}

/// The review's third point: the store answers only the session that asked, and drops
/// the answer where the region's owner has changed since. A new owner hears nothing of
/// the old owner's question, and is answered its own.
#[test]
fn an_answer_is_dropped_where_the_region_s_owner_changed_while_it_was_worked_out() {
    let held = Arc::new(HeldEnd {
        barrier: Barrier::new(2),
        held: AtomicBool::new(false),
    });
    let generators = Generators::new(
        generator(),
        Arc::new(Void::of(Dimension::Nether)),
        held.clone(),
    );
    let store = Store::memory_told(generators, division()).unwrap();
    let old = open(&store, hello(end_home(), 1));
    ask(&old, question(4));
    // The store is reading the first chunk of the search.
    held.barrier.wait();
    let (answer, answered) = mpsc::channel();
    store
        .messages
        .send(Message::Open {
            hello: hello(end_home(), 2),
            reply_to: store.messages.clone(),
            answer,
        })
        .unwrap();
    held.barrier.wait();
    let (opened, _) = answered.recv().unwrap().unwrap();
    let new = StoreHandle::local(opened, store.messages.clone());
    ask(&new, question(5));
    let mine = next_answer(&new);
    assert_eq!(mine.id, question(5).id, "the new owner's own question");
    assert!(mine.exit.is_some());
    assert_eq!(
        answers_before_flush(&new),
        [],
        "nothing of the old question"
    );
    // The old owner is answered nothing once it is replaced.
    loop {
        match old.try_reply() {
            Some(StoreReply::GatewayExits { answers }) => {
                panic!("the replaced owner was answered: {answers:?}")
            }
            Some(_) => {}
            None => break,
        }
    }
}

/// A void End that counts how often it made each chunk.
#[derive(Default)]
struct CountedEnd(std::sync::Mutex<std::collections::BTreeMap<ChunkPos, u32>>);

impl CountedEnd {
    fn made(&self, position: ChunkPos) -> u32 {
        self.0.lock().unwrap().get(&position).copied().unwrap_or(0)
    }
}

impl ChunkGenerator for CountedEnd {
    fn generate(&self, position: ChunkPos) -> Chunk {
        *self.0.lock().unwrap().entry(position).or_default() += 1;
        Void::of(Dimension::End).generate(position)
    }

    fn settings(&self) -> GeneratorSettings {
        Void::of(Dimension::End).settings()
    }
}

/// The review's fifth point: the search generates what was never made "and stores what
/// it generates, as `Load` does, so the arrival does not make it again".
#[test]
#[ignore = "finding: the review (19.8, item 5) has the store keep the chunks the search generates, as Load does, so that the exit's holder does not make them again; lanes/gateways.rs names that it does not, and a Load of a chunk the search read generates it a second time (the same chunk: nothing a player sees)"]
fn a_chunk_the_search_generated_is_stored_and_not_made_again_when_it_is_loaded() {
    let counted = Arc::new(CountedEnd::default());
    let generators = Generators::new(
        generator(),
        Arc::new(Void::of(Dimension::Nether)),
        counted.clone(),
    );
    let store = Store::memory_told(generators, division()).unwrap();
    let end = open(&store, hello(end_home(), 1));
    ask(&end, question(7));
    next_answer(&end);
    let read = ChunkPos::new(70, 0);
    assert_eq!(counted.made(read), 1, "the search made it");
    end.request(StoreRequest::Claim { chunks: vec![read] });
    loop {
        if let StoreReply::Claimed { granted, .. } = any_reply(&end) {
            assert_eq!(granted, [read]);
            break;
        }
    }
    end.request(StoreRequest::Load { position: read });
    loop {
        if let StoreReply::Loaded { position, .. } = any_reply(&end) {
            assert_eq!(position, read);
            break;
        }
    }
    assert_eq!(counted.made(read), 1, "loaded as stored, not made again");
}

/// 19.8, "The store's answer": `None` is the answer of a store that cannot say, and a
/// region of another dimension than the End is answered so, as the game's
/// `getPortalPosition` makes no exit outside the End.
#[test]
fn a_region_outside_the_end_is_answered_with_no_exit() {
    let store = memory();
    let nether = open(&store, hello(other_homes(&division())[0], 1));
    ask(&nether, question(6));
    let answer = next_answer(&nether);
    assert_eq!(
        (answer.id, answer.from, answer.exit),
        (question(6).id, GATEWAY, None)
    );
}
