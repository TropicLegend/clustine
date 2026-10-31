//! The End gateways' exits (`docs/groundwork/p-plan.md`, section 19.8): the store works
//! out `findOrCreateValidTeleportPos` for a gateway a region asks about, from each
//! chunk's committed form, a chunk at a time. Each chunk is read as a copy's base is
//! (its stored form, or the generator's where it was never made, with its holder's
//! commits in the log replayed into it) on the thread for chunks, and handed back here
//! to the search (`gateway_exit.rs`), which asks for the next until it knows the exit.
//! The answer goes on the asking region's stream as of then; it promises nothing about
//! commits, so it may come long after the question. A region whose owner changed since
//! is answered nothing: its new owner asks again.
//!
//! Named: a chunk the search generates is not stored, so a region that loads it later
//! has the generator make it again, as a copy's base is not stored either.

use std::collections::BTreeMap;
use std::sync::Arc;

use clustine_features::features::end::gateway_exit::{GatewaySearch, Step};
use clustine_rpc::{GatewayExit, GatewayExitAnswer, GatewayExitQuery, StoreReply};
use clustine_world::{Chunk, Dimension, LevelChunk};

use super::{Lanes, Streamed};
use crate::Session;
use crate::chunks::{Job, Listed};

/// The searches under way, by their numbers.
#[derive(Default)]
pub(super) struct Searches {
    next: u64,
    under_way: BTreeMap<u64, Search>,
}

/// A search for the gateway a region's session asked about.
struct Search {
    session: Session,
    query: GatewayExitQuery,
    search: GatewaySearch,
}

impl Lanes {
    /// Begins the search for the exit of the gateway `query` asks about, in the End.
    pub(super) fn start_gateway_search(&mut self, session: Session, query: GatewayExitQuery) {
        let rules = Dimension::End.rules();
        let number = self.gateway_searches.next;
        self.gateway_searches.next += 1;
        let search = GatewaySearch::new(query.from, rules.min_y, rules.height);
        self.gateway_searches.under_way.insert(
            number,
            Search {
                session,
                query,
                search,
            },
        );
        self.drive_gateway_search(number);
    }

    /// The thread for chunks read `position` for the search `number`: none where the
    /// chunk could not be made, which ends the search with no exit.
    pub(super) fn gateway_search_fed(
        &mut self,
        number: u64,
        session: Session,
        position: LevelChunk,
        chunk: Option<Chunk>,
    ) {
        let Some(run) = self.gateway_searches.under_way.get_mut(&number) else {
            return;
        };
        if run.session != session {
            return;
        }
        match chunk {
            Some(chunk) => run.search.feed(position.position, &chunk),
            None => {
                if let Some(run) = self.gateway_searches.under_way.remove(&number) {
                    self.answer_gateway(run.session, run.query, None);
                }
                return;
            }
        }
        self.drive_gateway_search(number);
    }

    /// Asks for the chunk the search `number` needs next, or answers it where it is done.
    fn drive_gateway_search(&mut self, number: u64) {
        let Some(run) = self.gateway_searches.under_way.get_mut(&number) else {
            return;
        };
        match run.search.step() {
            Step::Need(position) => {
                let session = run.session;
                self.read_for_search(number, session, LevelChunk::new(Dimension::End, position));
            }
            Step::Done(found) => {
                if let Some(run) = self.gateway_searches.under_way.remove(&number) {
                    let exit = GatewayExit {
                        exit: found.exit,
                        island: found.island,
                    };
                    self.answer_gateway(run.session, run.query, Some(exit));
                }
            }
        }
    }

    /// Has the thread for chunks read `chunk` as a copy's base for the search `number`,
    /// with its holder's commits in the log: none where the asking session's region has
    /// another owner now, which ends the search.
    fn read_for_search(&mut self, number: u64, session: Session, chunk: LevelChunk) {
        let peer = self
            .regions
            .get(&session.region)
            .filter(|lane| lane.current == Some(session.number))
            .and_then(|lane| lane.owner.as_ref())
            .map(|owner| Arc::clone(&owner.peer));
        let Some(peer) = peer else {
            self.gateway_searches.under_way.remove(&number);
            return;
        };
        let holder = self.table.holder(chunk);
        let from = holder.and_then(|holder| self.table.held_from(holder, chunk));
        let listed = holder
            .and_then(|holder| self.regions.get(&holder))
            .map_or_else(Vec::new, |lane| {
                lane.live
                    .iter()
                    .map(|entry| Listed {
                        path: self.log.path(entry.segment),
                        offset: entry.offset,
                        length: entry.length,
                    })
                    .collect()
            });
        let _ = self.jobs.send(Job::Copy {
            position: chunk,
            listed,
            from,
            sequence: 0,
            last_change: 0,
            peer,
            search: Some(number),
        });
    }

    /// Answers the question on the asking region's stream, where the session that asked
    /// is its owner still.
    fn answer_gateway(
        &mut self,
        session: Session,
        query: GatewayExitQuery,
        exit: Option<GatewayExit>,
    ) {
        let current = self
            .regions
            .get(&session.region)
            .is_some_and(|lane| lane.current == Some(session.number));
        if !current {
            return;
        }
        let answers = vec![GatewayExitAnswer {
            id: query.id,
            from: query.from,
            exit,
        }];
        self.group.stream(
            session.region,
            Streamed::Reply(Box::new(StoreReply::GatewayExits { answers })),
        );
    }
}
