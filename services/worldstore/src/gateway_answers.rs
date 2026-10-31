//! Tests of the store's answers to the End gateways' questions
//! (`docs/groundwork/p-plan.md`, section 19.8): a region of the End is answered, after
//! the chunks the search reads have been read, with an exit for the gateway it asked
//! about; a region of another dimension is answered at once with none. Where the exit
//! is, chunk by chunk, is the search's own tests' (`gateway_exit.rs`).

use clustine_rpc::{GatewayExitQuery, QueryId};
use clustine_world::BlockPos;

use super::*;
use crate::disk::MemoryDisk;
use crate::regions::store_on;
use crate::tests::{any_reply, division, hello, open, other_homes};

/// The eastern region of [`division`], in the overworld.
const EAST: u32 = 1;

/// A gateway about the End's island, as the fight places them.
const GATEWAY: BlockPos = BlockPos::new(96, 75, 0);

/// The answer to one question about [`GATEWAY`] from the region of `handle`, waited for.
fn answer(handle: &StoreHandle) -> clustine_rpc::GatewayExitAnswer {
    let id = QueryId { tick: 3, index: 0 };
    handle.request(StoreRequest::GatewayExits {
        queries: vec![GatewayExitQuery { id, from: GATEWAY }],
    });
    loop {
        if let StoreReply::GatewayExits { answers } = any_reply(handle) {
            let [answer] = answers[..] else {
                panic!("one answer: {answers:?}");
            };
            assert_eq!((answer.id, answer.from), (id, GATEWAY));
            return answer;
        }
    }
}

#[test]
fn an_end_region_s_question_is_answered_with_an_exit_out_along_the_gateway_s_line() {
    let end_home = other_homes(&division())[1];
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let end = open(&store, hello(end_home, 1));
    let found = answer(&end).exit.expect("an exit in the End");
    // Out along the gateway's line, east of the island by some 768 to 1280 blocks.
    assert!(
        (700..=1300).contains(&found.exit.x) && found.exit.z.abs() <= 32,
        "{found:?}"
    );
}

#[test]
fn a_question_from_outside_the_end_is_answered_with_no_exit() {
    let disk = Arc::new(MemoryDisk::default());
    let store = store_on(&disk, &division()).unwrap();
    let east = open(&store, hello(EAST, 1));
    assert_eq!(answer(&east).exit, None);
}
