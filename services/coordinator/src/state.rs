//! Which worker runs which region: the coordinator's decisions as a state machine.
//!
//! Nothing here does I/O or reads a clock. Whoever drives the coordinator passes the
//! time in, so the same calls always lead to the same decisions and each of them can be
//! tested. The service turns messages into calls and [`Changes`] into messages.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use clustine_region::{RegionId, RegionRoute, RoutingTable};
use clustine_rpc::{Assignment, Decline, Off, PlayersOf, RegionList, Vouch};
use clustine_world::{BlockPos, ChunkPos, Dimension, EntityId, EntityIds};
use tracing::{info, warn};

use crate::policy::Policy;

mod follow;

use follow::{Kept, Noted};

/// The module that the lines of the log are written under which say what the
/// coordinator begins by itself, wherever below this module they are written:
/// whoever reads the log finds them where
/// `docs/adr/0016-when-to-merge-and-split.md`, section 10, says they are.
const LOG: &str = module_path!();

/// What a coordinator is created with.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinatorConfig {
    /// How long a worker may be silent before it loses its regions, and how long a
    /// region may go without being vouched for before it loses its owner. A new
    /// coordinator also waits this long before it gives any region away; see
    /// [`Coordinator::new`].
    pub lease: Duration,
    /// What the coordinator goes by when it merges and splits regions by itself, or
    /// `None` for one that does so only when somebody asks. See
    /// `docs/adr/0016-when-to-merge-and-split.md`, section 8, and "Deciding by
    /// itself" at [`Coordinator`].
    pub follow: Option<Policy>,
}

/// Why a region is not moved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoveRefusal {
    #[error("the world has no region {0}")]
    NoSuchRegion(RegionId),
    #[error("region {0} has no owner to take it from")]
    NoOwner(RegionId),
    #[error("region {region} is being released already, by {from} for {to}")]
    BeingReleased {
        region: RegionId,
        from: String,
        to: String,
    },
    #[error("no other worker is there that region {0} could be moved to")]
    NoTarget(RegionId),
    #[error("region {region} cannot be moved to {worker}, which {why}")]
    NotATarget {
        region: RegionId,
        worker: String,
        /// What speaks against the worker, as the end of the sentence above.
        why: &'static str,
    },
    #[error("region {0} is part of a merge or a split that is under way")]
    Reserved(RegionId),
}

/// Why a merge or a split is not begun. Of several reasons it is the first of these
/// that holds, and for a merge the survivor is looked at before the region to absorb.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReshapeRefusal {
    #[error("the world has no region {0}")]
    NoSuchRegion(RegionId),
    #[error("a region cannot absorb itself")]
    Same,
    #[error("the region to absorb is the home region, which is never absorbed")]
    Home,
    #[error("region {0} is part of a merge or a split that is under way")]
    Reserved(RegionId),
    #[error("region {0} is being released")]
    BeingReleased(RegionId),
    #[error("region {0} has no owner")]
    NoOwner(RegionId),
    #[error("the worker {worker}, which would have to do it, {why}")]
    Unfit {
        worker: String,
        /// What speaks against the worker, as the end of the sentence above.
        why: &'static str,
    },
    #[error("no chunks were named whose players are to be split off")]
    NoChunks,
    /// A split names the region it makes, by the next id of the store's list, so a
    /// coordinator that has never been handed the list cannot ask for one.
    #[error("the world store's list of regions has not been read yet")]
    Unlisted,
    #[error("epochs have run out")]
    NoEpoch,
}

/// What a worker is to be told about a merge or a split; see
/// `docs/adr/0014-merging-and-splitting.md`, sections 4 and 5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// A merge or a split of `region`, which the worker owns with `epoch`, is coming:
    /// it is about to absorb a region, or to be split. Checkpoint it now. Not
    /// answered. See `docs/adr/0016-when-to-merge-and-split.md`, section 5.6, for
    /// when it is said before a split.
    Prepare { region: RegionId, epoch: u64 },
    /// Have `region`, which the worker owns with `epoch`, absorb `absorbed`, which
    /// nobody runs; open that one with `as_epoch`.
    Absorb {
        region: RegionId,
        epoch: u64,
        absorbed: RegionId,
        as_epoch: u64,
    },
    /// Split the players standing in `chunks` off `region`, which the worker owns
    /// with `epoch`, as the region `part`, and run that with `as_epoch`.
    SplitOff {
        region: RegionId,
        epoch: u64,
        chunks: Vec<ChunkPos>,
        as_epoch: u64,
        part: RegionId,
    },
}

/// The worker `worker` is to be told `order`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReshapeOrder {
    pub worker: String,
    pub order: Order,
}

/// A merge or a split that somebody asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Merge {
        survivor: RegionId,
        absorbed: RegionId,
    },
    Split {
        region: RegionId,
    },
}

/// Why a merge or a split that the coordinator had taken on came to nothing, or why
/// nobody knows what came of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undone {
    /// The worker that was to do it says so, and why.
    Off(Off),
    /// The region to absorb was not released within the lease, and was taken from its
    /// owner.
    NotReleased,
    /// A lease has passed since it was asked without the worker's word. Of a merge,
    /// the world store's list was read and does not show it done. Of a split, nobody
    /// knows: the list shows which regions there are, and only the worker's word says
    /// which of them a split made.
    Overdue,
    /// This region of it lost its owner, or its owner's epoch changed, before the
    /// worker said what came of it. Of a merge whose worker had been told to absorb,
    /// the world store's list was read and does not show it done; of a split, nobody
    /// knows, as for [`Undone::Overdue`].
    Disowned(RegionId),
    /// The world store's list no longer has this region of it.
    Gone(RegionId),
    /// The worker said that the merge was done, and the world store's list still has
    /// the region that was to be absorbed.
    Contradicted,
    /// The reservation of a merge ended without the worker's word when a worker had
    /// been told to absorb, and the world store's list could not be read then: it may
    /// have been done all the same.
    Unread,
    /// Epochs have run out, so the region to absorb could not be opened anew.
    NoEpoch,
}

impl std::fmt::Display for Undone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off(why) => formatter.write_str(&off_in_words(*why)),
            Self::NotReleased => formatter.write_str(
                "the region to absorb was not released within the lease and was taken \
                 from its owner",
            ),
            Self::Overdue => formatter.write_str(
                "the worker did not say within the lease what came of it, and it is not \
                 known to be done",
            ),
            Self::Disowned(region) => write!(
                formatter,
                "region {region} changed hands before the worker said what came of it, \
                 and it is not known to be done"
            ),
            Self::Gone(region) => write!(
                formatter,
                "the world store's list of regions no longer has region {region}"
            ),
            Self::Contradicted => formatter.write_str(
                "the worker said it was done, but the world store's list of regions \
                 still has the region that was to be absorbed",
            ),
            Self::Unread => formatter.write_str(
                "the worker did not say what came of it, and the world store's list of \
                 regions could not be read: it may have been done all the same",
            ),
            Self::NoEpoch => formatter.write_str("epochs have run out"),
        }
    }
}

/// Why a worker says that nothing came of a merge or a split, as a sentence for
/// whoever asked for it.
fn off_in_words(why: Off) -> String {
    let words = match why {
        Off::NotRunning => "the worker does not run the region with the epoch it was named",
        Off::Busy => "the region is in the middle of a release, a merge or a split",
        Off::Nobody => "no player stands in a chunk named that the region holds",
        Off::NothingStays => "nobody would stay, and the region would hold nothing",
        Off::TooLarge => "what the world store would have to be handed is too large",
        Off::Declined(decline) => {
            return format!("the world store declined: {}", decline_in_words(decline));
        }
        Off::StoreLost => "the worker lost the world store on the way",
        Off::Unreadable => "the region to absorb could not be opened or read",
        Off::Refused => "the world store has seen a later owner of the region to absorb",
    };
    words.to_owned()
}

/// Why the world store declined a merge or a split, as the end of a sentence.
fn decline_in_words(decline: Decline) -> String {
    match decline {
        Decline::Uncheckpointed { region } => {
            format!("region {region} has commits that no checkpoint covers")
        }
        Decline::Tick { named } => format!("the tick is not above {named}"),
        Decline::NoSuchRegion => "the region to absorb is no living region".to_owned(),
        Decline::Home => "the home region is never absorbed".to_owned(),
        Decline::OtherDimension => "no merge joins two dimensions".to_owned(),
        Decline::NotOpened { epoch } => match epoch {
            Some(epoch) => format!("the region to absorb has an owner with epoch {epoch}"),
            None => "the region to absorb has no owner".to_owned(),
        },
        Decline::NotHeld { chunk } => {
            let (x, z) = (chunk.x, chunk.z);
            format!("the region does not hold the chunk {x},{z}")
        }
        Decline::Malformed => "the part has no chunks, or no epoch".to_owned(),
        Decline::TooLarge => "the record of it would be too long".to_owned(),
        Decline::NotNext { next } => format!("the next region is {next}, not the one named"),
    }
}

/// How a merge or a split that the coordinator had taken on has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reshaped {
    /// Who asked, as they were named in the call; nobody, if the coordinator asked
    /// itself.
    pub asker: Option<u64>,
    pub asked: Asked,
    /// The region that absorbed the other, or the one that was split off; or why
    /// there is none.
    pub outcome: Result<RegionId, Undone>,
}

/// A move that the coordinator has taken on: `from` is asked to release the region, and
/// `to` is reserved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveBegun {
    pub from: String,
    pub to: String,
}

/// The worker `worker` is to be told to release `region`, which it owns with `epoch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseOrder {
    pub worker: String,
    pub region: RegionId,
    pub epoch: u64,
}

/// How a release that somebody asked for with [`Coordinator::move_region`] has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveOutcome {
    /// Who asked, as they were named in the call.
    pub mover: u64,
    pub region: RegionId,
    /// Who owns the region at the end of the call in which the release ended, and with
    /// which epoch. That need not be the worker the move began for. `None` if the
    /// region is without an owner, because no worker was there to be given it; it is
    /// assigned like any such region later, and the mover hears no more of it.
    pub owner: Option<(String, u64)>,
    /// Whether the old owner let go of the region, by saying so or by registering
    /// without it. If not, it was taken for dead: it did not answer within the lease,
    /// or lost the region for another reason while it was asked.
    pub released: bool,
}

/// What a call changed, so that the service knows whom to tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// The workers whose assignments are not what they were before the call, in
    /// ascending order of their names. A worker that was forgotten is among them if it
    /// owned anything.
    pub workers: Vec<String>,
    /// Whether the routing table is not what it was before the call.
    pub routing: bool,
    /// The workers that are to be told to release a region, for the first time or
    /// again. A worker is told after its new assignments, if it has any.
    pub releases: Vec<ReleaseOrder>,
    /// The moves that have ended, for those who asked for them.
    pub moves: Vec<MoveOutcome>,
    /// The workers that said they are leaving and have been forgotten, because they own
    /// nothing any more, in ascending order. Their connections are to be closed, which
    /// is how they know that they may exit.
    pub gone: Vec<String>,
    /// What workers are to be told about merges and splits, in the order it came up.
    /// A worker is told after its new assignments and after what it is to release. An
    /// order to a worker without a connection is lost: one to absorb is given again
    /// when the worker registers, one to split is not.
    pub orders: Vec<ReshapeOrder>,
    /// The merges and splits that have ended.
    pub reshaped: Vec<Reshaped>,
    /// Whether the world store's list of regions is to be read now, and
    /// [`Coordinator::listed`] or [`Coordinator::unlisted`] called with what came of
    /// it. A reading that was asked for before this call does not do.
    pub read: bool,
}

/// A worker that has registered.
#[derive(Debug, Clone)]
struct Worker {
    /// Host and port at which edges reach it.
    address: String,
    /// When it was last heard from.
    heard: Instant,
    /// Its place among the workers: one that registered earlier has a lower number,
    /// and one that lost or released a region has the highest of all from then on.
    /// Among workers with equally many regions, the one with the lowest is given the
    /// next.
    arrival: u64,
    /// Whether it has a connection, as far as the service has said: it has one when it
    /// registers, and none once [`Coordinator::disconnected`] is called.
    connected: bool,
    /// Whether it has said that it is leaving since it last registered.
    leaving: bool,
    /// When it last failed a region: when one was taken from it because it did not
    /// vouch for it, or because it did not answer when asked to release it. Registering
    /// again does not undo that; see [`Coordinator::FAULT_MEMORY`].
    failed: Option<Instant>,
}

/// A region that its owner has been asked to let go of.
#[derive(Debug, Clone)]
struct Release {
    /// The epoch the owner runs the region with. A release is of one owner and one
    /// epoch, and is dropped when either is no longer the region's.
    epoch: u64,
    /// The owner.
    from: String,
    /// The worker the region is meant for. It is reserved: as long as the release
    /// lasts, it counts as having the region when the worker with the fewest is
    /// looked for. It may run regions already, and be the target of other releases.
    to: String,
    /// When the owner was first asked.
    asked: Instant,
    /// Who asked for the move, if somebody did; nobody did if the owner is leaving,
    /// or if the coordinator evens regions out.
    mover: Option<u64>,
}

/// A merge that is under way: the region `survivor` is to absorb the region the merge
/// is kept under.
#[derive(Debug, Clone)]
struct Merge {
    survivor: RegionId,
    /// The survivor's owner and the epoch it runs it with. A merge is of that owner
    /// and that epoch, as a release is of its.
    owner: String,
    epoch: u64,
    stage: MergeStage,
    /// When it was asked for. All of it has one lease from then.
    asked: Instant,
    /// Who asked, if somebody did.
    asker: Option<u64>,
    /// Whether it is an absorption: the coordinator began it by itself for a region
    /// without players, which goes into another without players. Such a merge stands
    /// nobody still, and ends otherwise for its survivor than a merge by the
    /// distances or one that somebody asked for (ADR-0016, sections 4.4 and 5.5).
    absorption: bool,
}

/// How far a merge has come.
#[derive(Debug, Clone)]
enum MergeStage {
    /// The owner `from` of the region to absorb, which runs it with `epoch`, has been
    /// asked to release it.
    Releasing { from: String, epoch: u64 },
    /// The region to absorb has no owner, and the survivor's owner has been told to
    /// open it with `as_epoch` and absorb it. An order that is given again names the
    /// same epoch, whatever has been heard of the region since.
    Absorbing { as_epoch: u64 },
    /// A worker has said what came of it, and the world store's list is being read
    /// to find out what did.
    Ended { said: Result<(), Off> },
    /// The reservation has run out for the reason given, with the region to absorb
    /// already taken from its owner, and the world store's list is being read before
    /// that region is given to anyone.
    Lapsed { why: Undone },
}

/// What a reading of the world store's list has of a region that was to be absorbed.
#[derive(Debug, Clone, Copy)]
enum Fate {
    /// It went into this region.
    Absorbed(RegionId),
    /// It is a region still.
    Living,
    /// The list has it neither as living nor as absorbed, and is not older than it.
    Gone,
    /// The list is older than the region.
    Unknown,
}

/// A split that is under way, of the region it is kept under.
#[derive(Debug, Clone)]
struct Split {
    /// The region's owner and the epoch it runs it with.
    owner: String,
    epoch: u64,
    /// The epoch the owner was told to run the new region with, which is how its
    /// answer is known. The id it was told to give the region is not kept: the store
    /// can have made the region under another, so only the worker's answer says
    /// which region the split made.
    as_epoch: u64,
    /// The chunks that were named. Whoever stood in one of them is the new
    /// region's, as far as the coordinator has heard (ADR-0016, section 2.4).
    chunks: Vec<ChunkPos>,
    /// When it was asked for.
    asked: Instant,
    /// Who asked, if somebody did.
    asker: Option<u64>,
}

/// What a worker said it had let go of when the coordinator had no list to judge it
/// by: that it released `region`, which it held with `epoch`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word {
    worker: String,
    region: RegionId,
    epoch: u64,
}

/// What a call has to tell the service besides what can be seen of the owners before and
/// after it. It is gathered while the call is made and handed out at its end.
#[derive(Debug, Clone, Default)]
struct Pending {
    /// The releases that have ended, with their regions and whether the owner let go.
    ended: Vec<(RegionId, Release, bool)>,
    /// [`Changes::releases`].
    orders: Vec<ReleaseOrder>,
    /// [`Changes::gone`].
    gone: Vec<String>,
    /// [`Changes::orders`].
    reshapes: Vec<ReshapeOrder>,
    /// [`Changes::reshaped`].
    reshaped: Vec<Reshaped>,
    /// [`Changes::read`].
    read: bool,
    /// Whether the home region or the absorbed pairs of the routing table are not
    /// what they were.
    relisted: bool,
}

/// What the coordinator knows about a region.
#[derive(Debug, Clone, Default)]
struct Region {
    /// Who runs it, if anyone. An owner is always a registered worker.
    owner: Option<Owner>,
    /// The epoch of its owner or, while it has none, of the last one it had. It never
    /// goes down, and it is 0 as long as the region has never had an owner.
    epoch: u64,
    /// Whether the last owner itself said that it let go of the region, and nobody has
    /// run it since. Such a region does not wait for the grace period of a new
    /// coordinator to end: nobody is left who could report that it holds it.
    let_go: bool,
    /// What a coordinator that merges and splits by itself keeps of it; nothing for
    /// one that does not.
    kept: Kept,
    /// The dimension whose chunks it holds, as the store's list says.
    dimension: Dimension,
}

/// The worker that runs a region.
#[derive(Debug, Clone)]
struct Owner {
    /// The name of the worker.
    worker: String,
    /// The entity ids the region hands out while this worker runs it. They are only
    /// passed on: nothing the coordinator decides depends on them, as the world store
    /// issues entity ids from now on, and they go once nothing reads them any more.
    entity_ids: EntityIds,
    /// The latest time at which the region was vouched for: by being assigned or
    /// reported at a registration, or by a heartbeat that names it and counts.
    vouched: Instant,
    /// When the owner began to say that the region waits for the world store, if that
    /// is what it has said since: the earliest heartbeat of the run of
    /// [`Vouch::WaitingForStore`] that no [`Vouch::Committed`] has ended.
    waiting_since: Option<Instant>,
}

impl Owner {
    /// The worker `name` starts to run a region at `now`, which counts as being vouched
    /// for: a new owner has a lease to open and restore it before it has to say more.
    fn new(name: &str, entity_ids: EntityIds, now: Instant) -> Self {
        Self {
            worker: name.to_owned(),
            entity_ids,
            vouched: now,
            waiting_since: None,
        }
    }

    /// The owner says at `now` what it vouches for the region with.
    fn vouch(&mut self, now: Instant, vouch: Vouch) {
        match vouch {
            Vouch::Committed => {
                self.waiting_since = None;
                self.vouched = self.vouched.max(now);
            }
            Vouch::WaitingForStore => {
                // Times need not come in order, so the run began at the earliest of them.
                let since = self.waiting_since.map_or(now, |since| since.min(now));
                self.waiting_since = Some(since);
                if now.saturating_duration_since(since) <= Coordinator::STORE_PATIENCE {
                    self.vouched = self.vouched.max(now);
                }
            }
        }
    }

    /// Why the owner is not to run the region any longer as of `now`, although it is
    /// heard from, if there is a reason.
    fn unvouched(&self, now: Instant, lease: Duration) -> Option<&'static str> {
        if now.saturating_duration_since(self.vouched) > lease {
            return Some("the region was not vouched for within the lease");
        }
        let waited = self
            .waiting_since
            .map(|since| now.saturating_duration_since(since));
        if waited.is_some_and(|waited| waited > Coordinator::STORE_PATIENCE) {
            return Some("the region has waited for the world store for too long");
        }
        None
    }
}

/// The owner of a region as workers and edges get to see it. What a call changed is the
/// difference between these before and after it.
#[derive(Debug, PartialEq, Eq)]
struct Holder {
    worker: String,
    address: String,
    assignment: Assignment,
}

/// What can be seen of the regions from outside, as a call finds it: what the call
/// changed is the difference to what it leaves.
#[derive(Debug)]
struct Seen {
    /// The owner of every region that has one.
    holders: BTreeMap<RegionId, Holder>,
    /// [`RoutingTable::waiting`].
    waiting: u32,
}

/// The entity ids of a region that was split off another: none, as the world store
/// says of such a region (ADR-0011, section 2). Nothing reads them.
const NO_ENTITY_IDS: EntityIds = EntityIds {
    first: EntityId(0),
    end: EntityId(0),
};

impl Holder {
    /// What the worker is told about the region.
    fn to_worker(&self) -> (&str, Assignment) {
        (&self.worker, self.assignment)
    }

    /// What edges are told about the region, and whom it leads them to.
    fn to_edges(&self) -> (&str, &str, u64) {
        (&self.worker, &self.address, self.assignment.epoch)
    }
}

/// Decides which worker runs which region.
///
/// A worker registers and then has to be heard from at least once per lease. It runs
/// as many regions as it is given. A region can be given to any worker that is
/// registered, has a connection and is not leaving, and of those it goes to **the one
/// with the fewest regions**: the regions it owns and those it is the reserved target of
/// (see below) are counted. Of several with equally few it goes to the one that has
/// waited longest: the one that registered first, but a worker that lost or released a
/// region is behind all the others from then on. A worker that failed a region a
/// little while ago comes after every worker that did not, however few regions it has
/// (see below).
///
/// [`Coordinator::tick`] gives the regions without an owner away like that, the lowest
/// first and counting each as it goes, so that they are spread evenly over the workers
/// that are there. A worker that is silent for longer than a lease is forgotten, and
/// what it ran goes to the others, with higher epochs. A worker that registers later is
/// given what has no owner, and regions of workers that have more by and by (see
/// "Evening out" below). The times that are passed in need not be in order; a worker
/// was last heard from at the latest of them.
///
/// Being heard from is not enough to keep a region, though: a worker that is there but
/// cannot get anything of the region made durable shows its players nothing. So each
/// region has to be **vouched for** by its owner. A region is vouched for at the time
///
/// - at which it was assigned, or reported by its owner at a registration: a new owner
///   needs a lease to open and restore it before it can say more;
/// - of a heartbeat in which its owner names it with [`Vouch::Committed`];
/// - of a heartbeat in which its owner names it with [`Vouch::WaitingForStore`], if that
///   is no more than [`Coordinator::STORE_PATIENCE`] after the first heartbeat of the
///   run of such vouches that this one belongs to. Only a `Committed` ends a run.
///
/// At a tick, a region loses its owner if the latest time it was vouched for is more than
/// a lease before, or if its owner has been waiting for the store for longer than
/// `STORE_PATIENCE`: at that point, rather than a lease after the last vouch that
/// counted, so that a worker cut off from the store keeps the region no longer than that.
/// The owner stays registered and goes behind the other workers.
///
/// A worker from which a region was taken like that has **failed** the region, and so
/// has one from which a region was taken because it did not answer a release within
/// the lease. Whatever kept it from vouching or answering may well keep it from running
/// the next region, which would stand still for a lease with it. So for
/// [`Coordinator::FAULT_MEMORY`] leases after it last failed a region, a worker is **at
/// fault**: wherever the worker with the fewest regions is looked for, those that are
/// not at fault come before those that are, whatever they run, and regions are not
/// evened out towards it or away from it. It is still given a region when nobody else
/// can be, and a move that names it is done as asked. Registering again does not end
/// this, as a worker that only lost its connection registers too; time does, and a
/// worker that was forgotten in the meantime is a new one. A worker that released a
/// region, or whose epoch the world store refused, has not failed it.
///
/// Whatever workers report and in whatever order, a region never has two owners and
/// never goes back to an epoch below one it has had, because storage and peers tell the
/// current owner from a replaced one by the epoch alone.
///
/// # Moving a region on purpose
///
/// See `docs/adr/0009-moving-a-region.md`. A region moves when its owner lets go of it
/// and says so with [`Coordinator::released`]: the region is assigned at once, with a
/// new epoch, and nobody waits for a lease. The owner is asked to do that by a
/// **release**, which [`Coordinator::move_region`] notes, or the coordinator itself: for
/// the regions of a worker that said it is [`Coordinator::leaving`], and to even regions
/// out.
///
/// A release needs a **target**: a registered worker that has a connection, is not
/// leaving and is not the region's owner. It may run regions already and be the target
/// of other releases; if nobody names one, it is the one with the fewest regions, as
/// above. The target is **reserved** while the release lasts: it is being given the
/// region just now, and counts as having it. A release is of one owner and one epoch:
/// when the region changes either for another reason, the release is dropped and its
/// target no longer counts as having the region. While it lasts the region is not taken
/// from its owner for want of vouching, as the owner stops ticking on purpose. A release
/// that is not answered within a lease of when it was first asked ends like the owner's
/// death: the region is taken from it and assigned.
///
/// When a release ends, the region goes to its target if that still is one, else to the
/// target with the fewest regions, else it is without an owner until a tick finds a
/// worker to give it to.
///
/// Every call that may change something ends by dropping the releases that no longer
/// hold, forgetting the leaving workers that own nothing, and beginning a release for
/// each region of a leaving worker for which there is a target now.
///
/// # Evening out
///
/// Regions are only given to the worker with the fewest when they have no owner, so a
/// worker that registers after the others, or a moment after the grace period of a new
/// coordinator, would run nothing while another runs everything. So a
/// [`Coordinator::tick`], after everything else it does, begins a release of its own,
/// one at most:
///
/// - not during the grace period of a new coordinator, and not while any release is
///   under way, so that regions move one at a time;
/// - among the workers that are registered, have a connection, are not leaving and are
///   not at fault: a worker without a connection could not be told, and nothing is
///   evened out towards one that leaves or that may not be able to run the region;
/// - if the one of them with the most regions has at least two more than the one with
///   the fewest, which is the one a region without an owner would go to. Of several
///   with the most it is the one that has waited least. A difference of one is left
///   alone, so that nothing goes back and forth;
/// - of the region with the highest number that the former runs, for the latter.
///
/// A coordinator that merges and splits by itself (see "Deciding by itself" below)
/// goes about the last two points otherwise: it moves no region that is left alone,
/// nor one that a merge or a split is wanted of at that tick, and of the others the
/// one with the fewest players by what it last heard, and of several such the one
/// with the highest number.
///
/// From then on it is a release like any other, and the next one is begun by a tick
/// after it has ended. Who brought the difference about makes no difference: a move
/// that somebody asked for and that leaves a worker two regions ahead of another is
/// evened out again.
///
/// # The regions it knows, and the world store's list
///
/// See `docs/adr/0014-merging-and-splitting.md`, section 5, and
/// `docs/adr/0017-the-end-of-the-stripes.md`, section 2.3. **A coordinator knows no
/// region when it is made**: nobody tells it how the world is divided, because it is
/// not. The world store has a list of the regions there are, which the service reads
/// and hands to [`Coordinator::listed`], and until it has been handed one the
/// coordinator knows only what its workers report ([`Coordinator::awaits_the_list`]):
/// a living region of the list that the coordinator does not know is added, without
/// an owner, and assigned like any such region; a region the coordinator knows that
/// the list has as absorbed, or neither has nor leaves room for, is removed, and its
/// owner loses it. A region that a worker reports at a registration and that the
/// coordinator does not know is taken as living, with that worker as its owner,
/// until a reading says otherwise. **The list decides what happened**: a worker's
/// word that a merge or a split is done or off only makes the coordinator ask for
/// the list ([`Changes::read`]).
///
/// # Merging and splitting
///
/// [`Coordinator::merge`] and [`Coordinator::split`] note a merge or a split and say
/// what the workers are to be told ([`Changes::releases`], [`Changes::orders`]). From
/// then until it has ended, **the regions of it are reserved**: neither is moved,
/// released for a leaver or to even out, split, merged with another, or taken from
/// its owner for want of vouching, and nothing is evened out at all, nor within a
/// lease of its end.
///
/// A merge goes through two stages. First the owner of the region to absorb is asked
/// to release it, as for a move, and the survivor's owner to prepare. When it has let
/// go ([`Coordinator::released`], or its registering without the region), the region
/// is taken from it and **not assigned**: it gets a new epoch, with which the
/// survivor's owner is told to open and absorb it. When a worker says what came of
/// that ([`Coordinator::absorb_ended`]), the list is asked for, and
/// [`Coordinator::listed`] ends the merge by what it finds: the region absorbed, and
/// it is gone; or living, and it is assigned at once, as a region that its owner let
/// go of. A list that cannot be read is asked for again at every tick.
///
/// A split is one order to the region's owner, which names the id of the new region,
/// the next one of the list as it was last read, and the epoch to run it with. When
/// the owner says that it is done ([`Coordinator::split_ended`]), the new region is
/// its, with that epoch. The order is not given twice: a split done twice makes two
/// regions.
///
/// A merge and a split are each of the owners and epochs they began with and have
/// **one lease** from when they were asked. When that is up, or an owner or an epoch
/// is no longer what it was, the reservation ends:
///
/// - a merge at its first stage, like a release that was not answered: the region to
///   absorb is taken from its owner and assigned. That owner has failed the region
///   only if the merge's time is up: where the survivor's side ended the reservation,
///   it did nothing wrong;
/// - a merge at its second stage, by asking for the list first. If that shows the
///   region absorbed, the merge was done; if not, or if the list cannot be read, the
///   region is assigned, with an epoch above the one the survivor's owner was told,
///   so that the world store declines an absorb that is still on its way;
/// - a split, as one of which nobody knows what came, and by asking for the list.
///   While a split is reserved, a region the list shows and the coordinator does not
///   know is left out: it is the part of that split, which its worker runs from
///   memory and has yet to say so. The reading after the reservation adds it, as a
///   region nobody runs. Whoever asked is not told by it that the split was done: a
///   region with the id that was ordered can be another split's, and only the
///   worker's word says which region a split made.
///
/// # Alone with its workers
///
/// See `docs/adr/0017-the-end-of-the-stripes.md`, sections 2.3 and 6.6. A coordinator
/// whose workers are in its own process ([`Coordinator::alone`]) waits for nobody and
/// gives nobody up. It has **no grace period**: nobody else can have been running its
/// world, so it gives regions away, evens out and decides from the start. It **takes
/// no region from a worker for having been slow**: no worker is forgotten for being
/// silent and no region is taken for want of vouching, as there is nobody else to
/// give it to, and taking it could only give it back to the same worker with another
/// epoch, which stands its players still for a restore. And it **notes no failure of
/// a worker**: nobody is there to be given regions before one that failed, so the
/// only thing the mark would do is keep the coordinator from beginning anything with
/// that worker's regions. What is overdue still ends as it does anywhere: a release
/// that is not answered within the lease, and a merge whose region to absorb is not
/// released within it, take the region from its owner, and a tick gives it away
/// again.
///
/// # Deciding by itself
///
/// See `docs/adr/0016-when-to-merge-and-split.md`, whose sections are named here. A
/// coordinator that was created with [`CoordinatorConfig::follow`] merges and splits
/// regions without being asked, by where the players are. One that was not does none
/// of what follows, and keeps nothing for it.
///
/// Workers say where the players of their regions are ([`Coordinator::players`]),
/// and of each region the coordinator keeps what it took last, its **sighting**. A
/// sighting is **fresh** while it is of the region's owner and epoch, was taken no
/// more than a second ago, and the region is not part of a merge or a split under
/// way. It is never dropped for being old: the players a region last named are still
/// somewhere near there, and count where they can only hold something back. When a
/// reading of the list shows a region absorbed, its crowds go to the sighting of the
/// region that took it, if that has one; when a split is made, the crowds in the
/// chunks it named go to a sighting of the part (section 2.4).
///
/// [`Coordinator::tick`], between what it settles and evening out, works out what is
/// wanted of the sightings ([`crate::decide`]) and notes since when: a merge by its
/// two regions, and every group of a region's players that is to go by itself.
/// **Nothing is begun on one look**: only what has been wanted without a break for
/// more than a second. Then, in this order, and the first three only while fewer
/// than four merges and splits are under way (section 5.3):
///
/// 1. one split at most, of the region whose group has stood longest, naming the
///    chunks around every group of it that has stood; and none while another split
///    is under way anywhere or the list is being read;
/// 2. the merges that have stood, the one that has waited longest first;
/// 3. the regions that have had no player for three rests and are not pinned to an
///    area, each into a region that has none either (an **absorption**, section 4.4);
/// 4. `Prepare` for the regions that a split is wanted of and that are free, or will
///    be within a second (section 5.6).
///
/// All of it only when the grace period is over, the list was read no more than two
/// leases ago, and every region the coordinator knows has a sighting (section 5.1);
/// and only with regions that are **free**: owned by a worker that has a connection,
/// is not leaving and is not at fault, not reserved, not being released, and not
/// left alone (section 5.2).
///
/// A region is **left alone** until a time that is kept for it
/// ([`Coordinator::alone_until`]): for a rest from when it is given an owner or an
/// epoch that the coordinator did not have for it, from when a merge it survived or
/// a split of it ends well, and from a split that found nobody; and for three rests,
/// doubling up to eight times that, from an attempt that failed. That is noted where
/// a merge or a split ends, for those somebody asked for as for the coordinator's
/// own (section 5.5). What somebody asks for is not held back by it.
///
/// Regions that the list has pinned to an area merge and split by the distances like
/// any others, and a part that is split off among them cannot grow. Whoever pinned
/// them may not have meant that, so the log says it once, at the first reading that
/// shows a pinned region (`docs/adr/0017-the-end-of-the-stripes.md`, section 7, N14).
#[derive(Debug, Clone)]
pub struct Coordinator {
    config: CoordinatorConfig,
    /// When the grace period ends: before this the coordinator gives nothing away
    /// that its owner did not let go of, evens nothing out and begins nothing by
    /// itself. One lease from when it was made; for a coordinator that is alone with
    /// its workers, when it was made, so that it has none.
    grace_until: Instant,
    /// Whether it still waits for its first list: it has been handed none, and was
    /// not made knowing its regions. Set when it is made and cleared for good by the
    /// first [`Coordinator::listed`].
    awaiting: bool,
    /// What workers said they had released of regions the coordinator did not know
    /// while it waited for its first list, in the order they said it and each word
    /// once (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3). The first list
    /// judges them and leaves none; nothing is kept once it has been handed in, nor
    /// by a coordinator that never waited for one.
    words: Vec<Word>,
    /// Whether it gives no worker up: it forgets none for being silent, takes no
    /// region for want of vouching and notes no failure. So for a coordinator that
    /// was made [`Coordinator::alone`], and for no other.
    keeps_its_workers: bool,
    /// The registered workers by name.
    workers: BTreeMap<String, Worker>,
    /// How many workers have registered, not counting those that were registered
    /// already.
    arrivals: u64,
    /// Every region the coordinator knows: those the world store's list showed and
    /// those workers reported, less those a reading of the list took away; and those
    /// it was made knowing, if it was ([`Coordinator::knowing`]).
    regions: BTreeMap<RegionId, Region>,
    /// The highest epoch issued or reported so far.
    last_epoch: u64,
    /// The indices of the entity id blocks issued or reported so far, from which
    /// [`Assignment::entity_ids`] is filled in.
    used_blocks: BTreeSet<u32>,
    /// The version of the routing table.
    version: u64,
    /// The regions that are being released.
    releases: BTreeMap<RegionId, Release>,
    /// The home region, the absorbed pairs and the next region id of the world
    /// store's list as it was last read; nothing until it has been.
    home: Option<RegionId>,
    /// Each dimension's home region, as the same list said.
    homes: Vec<(Dimension, RegionId)>,
    /// The world's spawn, which the store keeps with the world, as the same list said.
    /// Players enter the world in its chunk.
    spawn: Option<BlockPos>,
    absorbed: Vec<(RegionId, RegionId)>,
    next: Option<RegionId>,
    /// The merges under way, each under the region that is to be absorbed.
    merges: BTreeMap<RegionId, Merge>,
    /// The splits under way, each under the region that is split.
    splits: BTreeMap<RegionId, Split>,
    /// When a merge or a split last ended.
    reshaped: Option<Instant>,
    /// Whether the list has been asked for and neither [`Coordinator::listed`] nor
    /// [`Coordinator::unlisted`] has been called since.
    reading: bool,
    /// Whether a split has ended without the worker's word that it was made, and the
    /// list has not been read since, so that the part, if there is one, may be a
    /// region nobody knows of. That is a reservation that ended by itself, and also a
    /// worker that said why there is no part: a worker that lost the world store on
    /// the way does not know whether the store made the split first, and the reading
    /// that follows such a word fails as a rule, the store being away. The list is
    /// asked for at every tick until it has been read.
    owed: bool,
    /// What a coordinator that merges and splits by itself keeps besides what it
    /// keeps of each region; nothing for one that does not.
    noted: Noted,
    /// What the call that is being made has to tell the service; empty between calls.
    pending: Pending,
}

impl Coordinator {
    /// How long a region counts as vouched for while its owner says that it waits for
    /// the world store. Moving a region would not help while nobody can reach the store;
    /// after this long the store is likely fine and the owner cut off from it.
    pub const STORE_PATIENCE: Duration = Duration::from_secs(30);

    /// For how many leases a worker that failed a region is at fault; see
    /// [`Coordinator`]. Long enough for a worker that is given a region and fails it
    /// again to be found out before the fault is forgotten, short enough for a worker
    /// that had a bad moment to be of use again soon.
    pub const FAULT_MEMORY: u32 = 6;

    /// How often [`Coordinator::tick`] is to be called when the coordinator decides by
    /// itself, which is as often as a worker says where its players are. See
    /// `docs/adr/0016-when-to-merge-and-split.md`, section 3.
    pub const LOOK: Duration = Duration::from_millis(250);

    /// A coordinator that knows of no worker and of no region yet.
    ///
    /// Which regions there are it learns from the world store's list, and until it
    /// has been handed one ([`Coordinator::awaits_the_list`]) from what its workers
    /// report: its routing table has no route, counts no region as waiting and names
    /// no home region, and a worker that registers holding nothing is told that it
    /// runs nothing (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3).
    ///
    /// Every epoch it issues is above `first_epoch`, and the version of its routing
    /// table starts there. The service passes the wall-clock time, so that a coordinator
    /// that takes over from another, of which it knows nothing, carries on above it in
    /// both.
    ///
    /// For one lease from `now` it assigns nothing new: workers that kept running while
    /// there was no coordinator must get the chance to report what they hold before any
    /// of it is given away. A region whose owner says that it let go of it does not wait
    /// for that ([`Coordinator::released`]).
    pub fn new(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self {
        Self {
            grace_until: follow::after(now, config.lease),
            awaiting: true,
            words: Vec::new(),
            keeps_its_workers: false,
            config,
            workers: BTreeMap::new(),
            arrivals: 0,
            regions: BTreeMap::new(),
            last_epoch: first_epoch,
            used_blocks: BTreeSet::new(),
            version: first_epoch,
            releases: BTreeMap::new(),
            home: None,
            homes: Vec::new(),
            spawn: None,
            absorbed: Vec::new(),
            next: None,
            merges: BTreeMap::new(),
            splits: BTreeMap::new(),
            reshaped: None,
            reading: false,
            owed: false,
            noted: Noted::default(),
            pending: Pending::default(),
        }
    }

    /// A coordinator that knows `regions` from the start, without owners and without
    /// a home region, and does not wait for a first list
    /// ([`Coordinator::awaits_the_list`] is false from the start). In everything else
    /// it is [`Coordinator::new`].
    ///
    /// For tests, and no process makes one: most tests of the coordinator are about
    /// what it does with regions it knows, and handing each a list instead would
    /// change the version of its routing table, its home region and what it may be
    /// asked (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3).
    #[doc(hidden)]
    pub fn knowing(
        config: CoordinatorConfig,
        now: Instant,
        first_epoch: u64,
        regions: &[RegionId],
    ) -> Self {
        Self {
            regions: regions.iter().map(|id| (*id, Region::default())).collect(),
            awaiting: false,
            ..Self::new(config, now, first_epoch)
        }
    }

    /// A coordinator whose workers are in its own process: [`Coordinator::new`] with
    /// two differences (`docs/adr/0017-the-end-of-the-stripes.md`, sections 2.3 and
    /// 6.6; "Alone with its workers" at [`Coordinator`]).
    ///
    /// It has no grace period, so it assigns, evens out and decides from `now` on:
    /// nobody else can have been running its world. And it gives no worker up
    /// ([`Coordinator::keeps_its_workers`]): it forgets none for being silent, takes
    /// no region for want of vouching and notes no failure of a worker. A process
    /// that is held up for longer than the lease, as a laptop that sleeps is, wakes
    /// with its worker registered and its regions running.
    ///
    /// Only the single process makes one. Like one made with `new` it knows no region
    /// until it has been handed a list.
    pub fn alone(config: CoordinatorConfig, now: Instant, first_epoch: u64) -> Self {
        Self {
            grace_until: now,
            keeps_its_workers: true,
            ..Self::new(config, now, first_epoch)
        }
    }
    /// What the coordinator was created with.
    pub fn config(&self) -> &CoordinatorConfig {
        &self.config
    }

    /// The home region of the world store's list as it was last handed in; none
    /// until one has been.
    pub fn home(&self) -> Option<RegionId> {
        self.home
    }

    /// Whether it has been handed no list and was not made knowing its regions.
    ///
    /// Until then the list says nothing of which regions there are, and whoever
    /// serves the coordinator has it read until it has been read once, whichever way
    /// the coordinator reshapes (`docs/adr/0017-the-end-of-the-stripes.md`, section
    /// 2.3). A reading that fails ([`Coordinator::unlisted`]) leaves it waiting.
    pub fn awaits_the_list(&self) -> bool {
        self.awaiting
    }

    /// Whether it gives no worker up for silence (it was made
    /// [`Coordinator::alone`]). Whoever serves it closes no worker's connection for
    /// silence either.
    pub fn keeps_its_workers(&self) -> bool {
        self.keeps_its_workers
    }

    /// Whether `now` is within the grace period, in which nothing is given away that
    /// its owner did not let go of, nothing is evened out and nothing is begun by
    /// the coordinator itself.
    fn in_grace(&self, now: Instant) -> bool {
        now < self.grace_until
    }

    /// A worker offers to run regions, or is back after losing its connection.
    ///
    /// Nobody is refused. The worker is registered under `name`. If a worker of that
    /// name is registered already, this is taken to be the same worker: edges are sent
    /// to `address` from now on, and what it owns stays.
    ///
    /// `holding` is what the worker runs already. It goes on running such a region, with
    /// the epoch and the entity ids it reports, unless
    ///
    /// - the world store's list, as it was last read, has the region as absorbed,
    /// - another worker owns the region: whoever reports a region first keeps it,
    ///   whatever the epochs, or
    /// - the region has had an owner with a higher epoch, so this one was replaced.
    ///
    /// A region the coordinator does not know is taken as living on the worker's word:
    /// it is one that was split off another, of which an earlier coordinator heard, or
    /// nobody; or any region at all, as long as the coordinator has been handed no
    /// list. The next reading of the list says whether it is.
    ///
    /// A region the worker reports and goes on running counts as vouched for at `now`.
    /// That does not end a run of [`Vouch::WaitingForStore`]: a worker that lost its
    /// connection while it waited for the store is waiting still.
    ///
    /// The worker's assignments after the call tell it what became of its holdings: it
    /// has to stop running whatever is not among them.
    ///
    /// A worker that registers has a connection and is not leaving, whatever a worker
    /// of that name said before: a replaced process comes back under its name.
    ///
    /// If a region the worker owns is being released, the worker may have missed being
    /// asked, or its answer may have been lost, with the connection it had. So if
    /// `holding` names the region with the epoch of the release, the worker is to be
    /// asked again ([`Changes::releases`]); the time it has to answer still counts from
    /// when it was first asked. If `holding` does not, the worker has let go of the
    /// region, and that is taken as its [`Coordinator::released`]. The same holds of a
    /// region it was asked to release for a merge. And if a region it owns is to
    /// absorb another, and it has been told so, it is told again
    /// ([`Changes::orders`]): the worker can take that order twice.
    pub fn register(
        &mut self,
        now: Instant,
        name: &str,
        address: &str,
        holding: &[Assignment],
    ) -> Changes {
        let before = self.seen();
        match self.workers.get_mut(name) {
            Some(worker) => {
                worker.heard = worker.heard.max(now);
                worker.connected = true;
                worker.leaving = false;
                if worker.address != address {
                    info!(worker = name, address, "a worker has a new address");
                    worker.address = address.to_owned();
                }
            }
            None => {
                info!(worker = name, address, "a worker registered");
                let worker = Worker {
                    address: address.to_owned(),
                    heard: now,
                    arrival: self.arrivals,
                    connected: true,
                    leaving: false,
                    failed: None,
                };
                self.workers.insert(name.to_owned(), worker);
                self.arrivals += 1;
            }
        }
        for holding in holding {
            self.report(now, name, holding);
        }

        let asked: Vec<(RegionId, u64)> = self
            .releases
            .iter()
            .filter(|(_, release)| release.from == name)
            .map(|(region, release)| (*region, release.epoch))
            .collect();
        for (region, epoch) in asked {
            // What the worker reported may have moved the region on to another epoch.
            // Such a release is dropped when the call ends.
            if !self.holds(region, name, epoch) {
                continue;
            }
            let held = |held: &Assignment| held.region == region && held.epoch == epoch;
            if holding.iter().any(held) {
                self.pending.orders.push(ReleaseOrder {
                    worker: name.to_owned(),
                    region,
                    epoch,
                });
            } else {
                info!(
                    worker = name,
                    %region,
                    epoch,
                    "a worker registered without a region it was asked to release"
                );
                self.hand_over(now, region, true);
            }
        }

        // The same for what it was asked to release for a merge.
        let asked: Vec<(RegionId, u64)> = self
            .merges
            .iter()
            .filter_map(|(absorbed, merge)| match &merge.stage {
                MergeStage::Releasing { from, epoch } if from == name => Some((*absorbed, *epoch)),
                _ => None,
            })
            .collect();
        for (absorbed, epoch) in asked {
            // As above: what it reported may have moved either region on, and such a
            // merge ends with the call.
            if self.broken_merge(absorbed).is_some() {
                continue;
            }
            let held = |held: &Assignment| held.region == absorbed && held.epoch == epoch;
            if holding.iter().any(held) {
                self.pending.orders.push(ReleaseOrder {
                    worker: name.to_owned(),
                    region: absorbed,
                    epoch,
                });
            } else {
                info!(
                    worker = name,
                    region = %absorbed,
                    epoch,
                    "a worker registered without a region it was asked to release for a merge"
                );
                self.absorb_released(now, absorbed);
            }
        }
        // An order to absorb may have been lost with the connection the worker had.
        // One that was given a moment ago, in this very call, is not given twice.
        let ordered = |orders: &[ReshapeOrder], absorbed: RegionId| {
            orders.iter().any(|told| {
                matches!(told.order, Order::Absorb { absorbed: named, .. } if named == absorbed)
            })
        };
        let again: Vec<RegionId> = self
            .merges
            .iter()
            .filter(|(_, merge)| merge.owner == name)
            .filter(|(_, merge)| matches!(merge.stage, MergeStage::Absorbing { .. }))
            .map(|(absorbed, _)| *absorbed)
            .filter(|absorbed| !ordered(&self.pending.reshapes, *absorbed))
            .collect();
        for absorbed in again {
            if self.broken_merge(absorbed).is_none() {
                self.order_absorb(absorbed);
            }
        }
        self.finish(&before, now)
    }

    /// A worker says that it is still there, and vouches for the regions it names, in
    /// order; see [`Coordinator`] for what counts. Returns whether the coordinator knows
    /// the worker: one it does not know, because it was silent for too long or because
    /// this is a new coordinator, has to register again.
    ///
    /// A region the worker does not own is passed over, as is every region if the
    /// worker is not known. Nothing changes owner here; that is left to the next tick.
    pub fn heartbeat(&mut self, now: Instant, name: &str, regions: &[(RegionId, Vouch)]) -> bool {
        let Some(worker) = self.workers.get_mut(name) else {
            return false;
        };
        worker.heard = worker.heard.max(now);
        for (id, vouch) in regions {
            let owner = self
                .regions
                .get_mut(id)
                .and_then(|region| region.owner.as_mut())
                .filter(|owner| owner.worker == name);
            if let Some(owner) = owner {
                owner.vouch(now, *vouch);
            }
        }
        true
    }

    /// A worker says where the players of the regions it runs are; see
    /// `docs/adr/0016-when-to-merge-and-split.md`, section 2.3. Returns whether the
    /// coordinator knows the worker, as [`Coordinator::heartbeat`] does: one it does
    /// not know has to register again.
    ///
    /// To say it is to be heard from, so a worker that says nothing else is not
    /// forgotten for being silent. It vouches for nothing: a region whose owner says
    /// only this loses that owner a lease after it was last vouched for. Like a
    /// heartbeat it changes no owner here; that is left to the next tick.
    ///
    /// A coordinator that decides nothing by itself keeps nothing of it. One that
    /// does takes or passes over each entry: it is passed over unless the worker owns
    /// the region with the epoch it names, while the region is part of a merge or a
    /// split under way, and if what was last taken of the region is of this owner
    /// and epoch and has that tick or a later one. What is taken is the region's
    /// **sighting** from then on; see "Deciding by itself" at [`Coordinator`].
    pub fn players(&mut self, now: Instant, name: &str, regions: &[PlayersOf]) -> bool {
        let Some(worker) = self.workers.get_mut(name) else {
            return false;
        };
        worker.heard = worker.heard.max(now);
        self.take_players(now, name, regions);
        true
    }

    /// The world store refused to let the worker `name` open `region`, because it has
    /// seen an owner of the region with the epoch `seen`.
    ///
    /// No epoch issued from now on is at or below `seen`: epochs come from the clock of
    /// the coordinator when it starts, and the store's record of them may be ahead of
    /// it. If `name` owns the region with an epoch below `seen`, the worker has dropped
    /// it, as it cannot run a region the store does not let it open. The region is
    /// without an owner then, and no holding with an epoch below `seen` is honoured for
    /// it any more. A worker that owns the region with an epoch at or above `seen` was
    /// refused under an earlier assignment, and keeps it. Nor does the word of one
    /// worker take a region from another.
    ///
    /// To be refused is to be heard from, if the worker is registered. The call ends
    /// like a [`Coordinator::tick`] at `now`, so that a region the worker dropped is
    /// given away at once, with an epoch above `seen`, and edges see it change hands in
    /// one new routing table. It may go to the same worker: being refused says nothing
    /// against it, and it has one region fewer now.
    pub fn epoch_refused(
        &mut self,
        now: Instant,
        name: &str,
        region: RegionId,
        seen: u64,
    ) -> Changes {
        // Whoever reports it, the store has seen the epoch, and issuing it again would
        // make the region impossible to open.
        self.last_epoch = self.last_epoch.max(seen);
        if let Some(worker) = self.workers.get_mut(name) {
            worker.heard = worker.heard.max(now);
        }
        let before = self.seen();
        if let Some(state) = self.regions.get_mut(&region) {
            let epoch = state.epoch;
            let dropped = |owner: &mut Owner| owner.worker == name && epoch < seen;
            if let Some(owner) = state.owner.take_if(dropped) {
                warn!(
                    %region,
                    worker = %owner.worker,
                    epoch,
                    seen,
                    "the world store refused the epoch of a region's owner"
                );
            }
            if state.owner.is_none() {
                state.epoch = state.epoch.max(seen);
            }
        }
        self.settle(now);
        self.finish(&before, now)
    }

    /// The service says that the connection of the worker `name` has ended: the one it
    /// last registered over, not an earlier one that a registration replaced.
    ///
    /// Nothing is taken from a worker for that; it may be back before its lease is out.
    /// It is no target of a release until it registers again, though: it would not hear
    /// that it was given the region.
    ///
    /// A worker that said it is leaving is not expected back. It is forgotten at once,
    /// and the call ends like a [`Coordinator::tick`] at `now`, so that what it owned
    /// goes to the other workers, if there are any and the coordinator is not new. The
    /// world store fences the worker if it lives.
    pub fn disconnected(&mut self, now: Instant, name: &str) -> Changes {
        let before = self.seen();
        match self.workers.get_mut(name) {
            Some(worker) if worker.leaving => {
                info!(worker = name, "a leaving worker's connection ended");
                self.workers.remove(name);
                self.settle(now);
            }
            Some(worker) => worker.connected = false,
            None => {}
        }
        self.finish(&before, now)
    }

    /// The worker `name` has been told to stop. It is given nothing new from now on, and
    /// is no target of a release. For each region it owns a release is begun as soon as
    /// there is a target, which is looked for at the end of this and every later call;
    /// several of its regions may be meant for the same target. Once it owns nothing,
    /// it is forgotten and named in [`Changes::gone`]; that is at once if it owns
    /// nothing now.
    ///
    /// To say so is to be heard from. Leaving belongs to one registration: a worker that
    /// registers again is not leaving until it says so again. A worker that is not
    /// registered is named in [`Changes::gone`] as well: there is nothing for it to
    /// wait for.
    pub fn leaving(&mut self, now: Instant, name: &str) -> Changes {
        let before = self.seen();
        match self.workers.get_mut(name) {
            Some(worker) => {
                worker.heard = worker.heard.max(now);
                if !worker.leaving {
                    info!(worker = name, "a worker is leaving");
                }
                worker.leaving = true;
            }
            None => self.pending.gone.push(name.to_owned()),
        }
        self.finish(&before, now)
    }

    /// Somebody wants `region` moved: to the worker `to`, which may run regions
    /// already, or to the target with the fewest. `mover` names whoever asked, and comes back in the [`MoveOutcome`] of a later call (or of
    /// none, if the coordinator is replaced before the release ends).
    ///
    /// The move is refused, and nothing changes, unless the region has an owner, is not
    /// part of a merge or a split under way, is not being released already, and there
    /// is a target for it, which `to` has to be if it is given; see [`Coordinator`].
    /// Otherwise a release is noted as asked at `now`, the target is reserved, and the
    /// owner is to be told ([`Changes::releases`]).
    pub fn move_region(
        &mut self,
        now: Instant,
        region: RegionId,
        to: Option<&str>,
        mover: u64,
    ) -> Result<(MoveBegun, Changes), MoveRefusal> {
        let state = self
            .regions
            .get(&region)
            .ok_or(MoveRefusal::NoSuchRegion(region))?;
        // Before the owner is looked for: a region that waits to be absorbed has none,
        // and this is why.
        if self.reserved(region) {
            return Err(MoveRefusal::Reserved(region));
        }
        let owner = state.owner.as_ref().ok_or(MoveRefusal::NoOwner(region))?;
        if let Some(release) = self.releases.get(&region) {
            return Err(MoveRefusal::BeingReleased {
                region,
                from: release.from.clone(),
                to: release.to.clone(),
            });
        }
        let (from, epoch) = (owner.worker.clone(), state.epoch);
        let to = match to {
            Some(named) => match self.unfit(named, &from) {
                None => named.to_owned(),
                Some(why) => {
                    return Err(MoveRefusal::NotATarget {
                        region,
                        worker: named.to_owned(),
                        why,
                    });
                }
            },
            None => self
                .first_target(now, &from)
                .ok_or(MoveRefusal::NoTarget(region))?,
        };

        let before = self.seen();
        self.note_release(now, region, epoch, &from, &to, Some(mover));
        let begun = MoveBegun { from, to };
        Ok((begun, self.finish(&before, now)))
    }

    /// The worker `name` says that it has let go of `region`, which it held with
    /// `epoch`: because it was asked to, or by itself.
    ///
    /// If it owns the region with that epoch, the region is taken from it and assigned
    /// at once, with a new epoch: to the target of the release if there was one and it
    /// still is a target, else to the target with the fewest regions. That holds
    /// whether or not a release was noted, and during the grace period of a new
    /// coordinator too: the owner itself says that the region is free. With no target,
    /// which is when no other worker is there, the region is without an owner until a
    /// tick gives it away, to this worker again if it is still the only one. The worker
    /// itself goes behind all the others.
    ///
    /// A region that nobody owns, released by a registered worker with an epoch that is
    /// not below the last one the coordinator knows of it, is free as well. That is
    /// what a new coordinator hears when the release was done before the worker found
    /// it: the worker registers holding nothing and says what it let go of. The region
    /// is given away at once like any without an owner, and the worker that says this
    /// is behind all the others for it.
    /// The coordinator cannot check that the worker was the owner; if it was not, the
    /// world store fences whoever still runs the region, as after any crash.
    ///
    /// If the region was to be released for a merge, it is taken from the worker and
    /// **not assigned**: the survivor's owner is told to absorb it
    /// ([`Changes::orders`]), with a new epoch to open it with, which is the region's
    /// from then on.
    ///
    /// **A coordinator that still waits for its first list keeps the word** of a
    /// registered worker about a region it does not know, behind the words it has
    /// kept before and each word once, and changes nothing else by the call
    /// (`docs/adr/0017-the-end-of-the-stripes.md`, section 2.3). It cannot say yet
    /// whether the store has such a region, nor with which epoch it was last run, so
    /// the first list judges the word ([`Coordinator::listed`]). Without that a
    /// region that was let go while there was no coordinator would wait out the grace
    /// period of the new one, as its worker says so once.
    ///
    /// From any other worker, or with any other epoch, this changes nothing. To say it
    /// is to be heard from, if the worker is registered.
    pub fn released(&mut self, now: Instant, name: &str, region: RegionId, epoch: u64) -> Changes {
        let before = self.seen();
        if let Some(worker) = self.workers.get_mut(name) {
            worker.heard = worker.heard.max(now);
        }
        let for_a_merge = self
            .merges
            .get(&region)
            .is_some_and(|merge| matches!(merge.stage, MergeStage::Releasing { .. }));
        if self.holds(region, name, epoch) && for_a_merge {
            info!(worker = name, %region, epoch, "a worker released a region to be absorbed");
            self.absorb_released(now, region);
        } else if self.holds(region, name, epoch) {
            info!(worker = name, %region, epoch, "a worker released a region");
            self.hand_over(now, region, true);
        } else if self.without_owner_since(region, name, epoch) {
            self.free_on_a_word(name, region, epoch);
            self.assign(now);
        } else if self.awaiting
            && !self.regions.contains_key(&region)
            && self.workers.contains_key(name)
        {
            // ADR-0017, section 2.3. Whether the store has such a region, and with
            // which epoch it was last run, only the list can say.
            info!(
                worker = name,
                %region,
                epoch,
                "a worker released a region this coordinator does not know yet; \
                 the first list will say"
            );
            let word = Word {
                worker: name.to_owned(),
                region,
                epoch,
            };
            if !self.words.contains(&word) {
                self.words.push(word);
            }
        } else {
            info!(
                worker = name,
                %region,
                epoch,
                "a worker released a region it does not own with that epoch"
            );
        }
        self.finish(&before, now)
    }

    /// Lets go of `region` on the word of the worker `name` that it released it with
    /// `epoch`, when [`Coordinator::without_owner_since`] holds of the three: the
    /// region has had that epoch, no epoch issued from now on is at or below it, the
    /// region is given away whatever the grace period says by the next
    /// [`Coordinator::assign`], and the worker goes behind all the others.
    fn free_on_a_word(&mut self, name: &str, region: RegionId, epoch: u64) {
        info!(
            worker = name,
            %region,
            epoch,
            "a worker released a region before this coordinator knew of it"
        );
        self.last_epoch = self.last_epoch.max(epoch);
        let state = self
            .regions
            .get_mut(&region)
            .expect("a region without an owner is one the coordinator knows");
        state.epoch = epoch;
        state.let_go = true;
        let arrivals = self.arrivals;
        let worker = self
            .workers
            .get_mut(name)
            .expect("the worker was found to be registered");
        worker.arrival = arrivals;
        self.arrivals += 1;
    }

    /// Forgets the workers whose lease has run out and hands out regions; to be called
    /// regularly.
    ///
    /// Leases are only looked at here: a worker that is heard from late, but before the
    /// tick that would have forgotten it, keeps what it has.
    ///
    /// Regions without an owner are given away, the lowest region first, each to the
    /// worker that has the fewest regions when its turn comes; see [`Coordinator`].
    /// Such an assignment has an epoch above every epoch issued or reported so far, and
    /// entity ids that were never issued or reported. A region stays without an owner if
    /// no worker is there that could be given it, or if epochs have run out.
    ///
    /// A release that was first asked more than a lease ago ends here: the region is
    /// taken from its owner, which goes behind all the other workers, and assigned as
    /// in [`Coordinator::released`].
    ///
    /// A merge or a split that was asked for more than a lease ago ends here as well;
    /// see [`Coordinator`]. And the world store's list is asked for again
    /// ([`Changes::read`]) if a merge waits for it, or a split ended without the
    /// worker's word that it was made, and the reading that was to follow failed.
    /// A coordinator that merges and splits by itself also asks for it when no
    /// reading has been answered for a lease, or none ever: it goes by the list for
    /// which region is home, which are pinned and what became of a merge, and nobody
    /// else tells it of a change that no worker reported
    /// (`docs/adr/0016-when-to-merge-and-split.md`, section 7). One that does not
    /// reads the list on events only.
    ///
    /// A coordinator that merges and splits by itself decides here, and nowhere
    /// else, what it begins: after what is said above and before evening out. See
    /// "Deciding by itself" at [`Coordinator`]. What it begins is in
    /// [`Changes::releases`] and [`Changes::orders`] like what somebody asked for.
    ///
    /// Last of all, and only here, a release is begun to even regions out, if a worker
    /// runs two regions more than another, no release is under way, and no merge or
    /// split is or was within the last lease; see [`Coordinator`]. The worker that is
    /// to release is named in [`Changes::releases`].
    pub fn tick(&mut self, now: Instant) -> Changes {
        let before = self.seen();
        self.settle(now);
        // What a coordinator that decides by itself begins goes by which regions
        // rest, so those that were given an owner a moment ago are noted first.
        self.note_owners(now);
        self.decide_and_begin(now);
        // Not part of what other calls end like a tick with: those give away what
        // has no owner, and this takes a region from one that runs it. After what
        // was begun above, so that a region which is wanted for a merge and free is
        // reserved before it could be picked.
        self.even_out(now);
        let said = |merge: &Merge| matches!(merge.stage, MergeStage::Ended { .. });
        let waited_for = self.owed || self.merges.values().any(said);
        // Never while a reading is asked for: the service throws a reading away for
        // the next one that is asked for, so whatever asks at every tick has to wait
        // for its answer first.
        if !self.reading && (waited_for || self.list_is_due(now)) {
            self.ask_for_the_list();
        }
        self.finish(&before, now)
    }

    /// The world store's list of regions as the service has just read it. A reading
    /// has to have been asked for after every earlier call that said
    /// [`Changes::read`], and readings have to be handed in in the order they were
    /// asked for: the list decides what happened, so an older one must not be taken
    /// for a newer.
    ///
    /// - A region the coordinator knows that the list has among the absorbed, or that
    ///   is below the list's next id and neither living nor absorbed, is removed. Its
    ///   owner, if it has one, loses it, and a merge or a split that names it ends. A
    ///   region at or above the next id is left alone: the reading is older than the
    ///   split that made it.
    /// - A living region of the list that the coordinator does not know is added,
    ///   without an owner, and assigned like any such region: at once, unless the
    ///   coordinator is new. **Not while a split is reserved**; see [`Coordinator`].
    /// - The home region and the absorbed pairs are noted for the routing table, and
    ///   the next id for the next split.
    /// - A merge of which a worker has said what came, or whose reservation has run
    ///   out at its second stage, ends by what the list has of the region to absorb.
    ///   Nothing is said of a split by it: whoever asked for one whose reservation
    ///   ended without the worker's word has been told so then.
    ///
    /// No epoch issued from now on is at or below one the list has, and a region
    /// without an owner has had the epoch the list has for it: a holding below that
    /// is not honoured, as the store would not let its worker open the region.
    ///
    /// **The first list judges the words that were kept** while there was none
    /// ([`Coordinator::released`]; `docs/adr/0017-the-end-of-the-stripes.md`, section
    /// 2.3), in the order they were said: when the living regions of the list have
    /// been added and the epochs raised, and before regions are given away. Each is
    /// judged as `released` judges such a word of a region the coordinator knows. If
    /// the region is known by then, has no owner, is no part of a merge and has no
    /// epoch above the word's, which now says that the list's is not above it, and
    /// if the worker is still registered, the region is let go with the word's epoch
    /// and given away by this call, whatever the grace period says, and the worker
    /// goes behind all the others. Any other word is dropped: the store does not have
    /// the region living, or somebody has run it since, or a worker registered
    /// holding it meanwhile, or the worker is no longer registered. None is kept
    /// after that.
    pub fn listed(&mut self, now: Instant, list: &RegionList) -> Changes {
        let before = self.seen();
        let living: BTreeMap<RegionId, u64> = list
            .regions
            .iter()
            .map(|info| (info.region, info.epoch))
            .collect();
        let absorbed: BTreeMap<RegionId, RegionId> = list.absorbed.iter().copied().collect();
        if let Some(highest) = living.values().max() {
            self.last_epoch = self.last_epoch.max(*highest);
        }

        let gone: Vec<RegionId> = self
            .regions
            .keys()
            .filter(|id| !living.contains_key(id))
            .filter(|id| absorbed.contains_key(id) || **id < list.next)
            .copied()
            .collect();
        // The living region that one which was absorbed went into in the end: a
        // region that absorbed another may have been absorbed since. No more steps
        // than there are pairs, should a list ever lead round in a circle.
        let went_into = |region: RegionId| {
            let mut into = region;
            for _ in 0..absorbed.len() {
                into = *absorbed.get(&into)?;
                if living.contains_key(&into) {
                    return Some(into);
                }
            }
            None
        };
        for id in gone {
            let Some(state) = self.regions.remove(&id) else {
                continue;
            };
            let owner = state.owner.map(|owner| owner.worker);
            info!(
                region = %id,
                into = ?absorbed.get(&id),
                ?owner,
                "the world store has a region no longer"
            );
            self.note_gone(id, state.kept, went_into(id));
        }

        // What the list has of a region, for the merge that is to absorb it.
        let fate = |region: RegionId| match absorbed.get(&region) {
            Some(into) => Fate::Absorbed(*into),
            None if living.contains_key(&region) => Fate::Living,
            None if region < list.next => Fate::Gone,
            None => Fate::Unknown,
        };
        // The merges first, then the reservations that this reading itself has
        // broken by taking a region away, and then the merges once more: those that
        // broke are dealt with by this reading and not by the next.
        for _ in 0..2 {
            let noted: Vec<RegionId> = self.merges.keys().copied().collect();
            for region in noted {
                self.merge_listed(now, region, fate(region));
            }
            self.end_broken_reshapes(now);
        }
        // Every reservation that waited for a reading has had this one, and so has
        // a split that ended without the worker's word: what it made, if anything,
        // is added below like any region nobody is known to run.
        self.reading = false;
        self.owed = false;
        self.pending.read = false;

        if self.splits.is_empty() {
            for (id, epoch) in &living {
                if self.regions.contains_key(id) {
                    continue;
                }
                info!(region = %id, epoch, "the world store has a region nobody is known to run");
                let state = Region {
                    epoch: *epoch,
                    ..Region::default()
                };
                self.regions.insert(*id, state);
            }
        }
        for (id, epoch) in &living {
            let unowned = self.regions.get_mut(id);
            if let Some(state) = unowned.filter(|state| state.owner.is_none()) {
                state.epoch = state.epoch.max(*epoch);
            }
        }
        // From here on the list has said which regions there are, and with which
        // epochs, so the words that were kept for it are judged, and none is kept any
        // more (ADR-0017, section 2.3). Here and nowhere else: the regions of the list
        // are known and have the store's epochs to compare with, everything that was
        // registered and reported meanwhile is in the state, and nothing has been
        // given away yet.
        self.awaiting = false;
        for word in std::mem::take(&mut self.words) {
            let Word {
                worker,
                region,
                epoch,
            } = word;
            if self.without_owner_since(region, &worker, epoch) {
                self.free_on_a_word(&worker, region, epoch);
            } else {
                info!(
                    %worker,
                    %region,
                    epoch,
                    "a worker released a region it does not own with that epoch"
                );
            }
        }

        for info in &list.regions {
            if let Some(region) = self.regions.get_mut(&info.region) {
                region.dimension = info.dimension;
            }
        }
        let home = Some(list.home);
        self.pending.relisted |=
            self.home != home || self.homes != list.homes || self.absorbed != list.absorbed;
        self.home = home;
        self.homes.clone_from(&list.homes);
        self.spawn = Some(list.spawn);
        self.absorbed.clone_from(&list.absorbed);
        self.next = Some(list.next);
        self.note_answered(now);
        self.note_listed(now, list);

        self.assign(now);
        self.finish(&before, now)
    }

    /// The world store's list of regions could not be read. What is said of readings
    /// in [`Coordinator::listed`] holds of this one too. A coordinator that waits for
    /// its first list goes on waiting, with the words it has kept for it.
    ///
    /// A merge whose reservation has run out at its second stage ends all the same,
    /// with the region to absorb assigned: should it have been absorbed, the worker
    /// that is given it is refused by the world store and says so. After a split
    /// that ended without the worker's word that it was made, the list is asked for
    /// at every tick until it has been read, as the part may be a region that nobody
    /// runs. A merge of which a worker has said what came waits on, and the list is
    /// asked for again at the next tick.
    pub fn unlisted(&mut self, now: Instant) -> Changes {
        let before = self.seen();
        self.reading = false;
        self.note_answered(now);
        let lapsed: Vec<RegionId> = self
            .merges
            .iter()
            .filter(|(_, merge)| matches!(merge.stage, MergeStage::Lapsed { .. }))
            .map(|(absorbed, _)| *absorbed)
            .collect();
        for absorbed in lapsed {
            warn!(
                region = %absorbed,
                "a merge has run out and the list cannot be read; the region is assigned"
            );
            self.let_go(absorbed);
            self.end_merge(now, absorbed, Err(Undone::Unread));
        }
        self.assign(now);
        self.finish(&before, now)
    }

    /// Somebody wants the region `absorbed` merged into `survivor`. `asker` names
    /// whoever asked, if anybody did, and comes back in the [`Reshaped`] of a later
    /// call (or of none, if the coordinator is replaced before the merge ends).
    ///
    /// The merge is refused, and nothing changes, for the first reason of
    /// [`ReshapeRefusal`] that holds:
    ///
    /// - `NoSuchRegion`: the coordinator does not know the region;
    /// - `Same`: the two are one region;
    /// - `Home`: `absorbed` is the home region of the list as it was last read;
    /// - `Reserved`: the region is part of a merge or a split under way;
    /// - `BeingReleased`: the region is being released, for a move, a leaver or to
    ///   even out;
    /// - `NoOwner`: the region has no owner;
    /// - `Unfit`: the survivor's owner has no connection or is leaving, or the owner
    ///   of the region to absorb has no connection;
    /// - `NoEpoch`: no epoch is left to open the absorbed region with.
    ///
    /// Otherwise the merge is noted as asked at `now`, both regions are reserved, the
    /// owner of `absorbed` is to be told to release it ([`Changes::releases`]) and the
    /// survivor's owner to prepare ([`Changes::orders`]); see [`Coordinator`].
    pub fn merge(
        &mut self,
        now: Instant,
        survivor: RegionId,
        absorbed: RegionId,
        asker: Option<u64>,
    ) -> Result<Changes, ReshapeRefusal> {
        // Taken before the merge is checked and noted, neither of which changes what
        // can be seen of the regions.
        let before = self.seen();
        self.begin_merge(now, survivor, absorbed, asker, false)?;
        Ok(self.finish(&before, now))
    }

    /// What [`Coordinator::merge`] checks and notes, without what a call ends with:
    /// a tick that begins a merge by itself does this between what it found and
    /// what it ends with, like everything else it does. `absorption` says whether
    /// it is one; see [`Merge::absorption`].
    fn begin_merge(
        &mut self,
        now: Instant,
        survivor: RegionId,
        absorbed: RegionId,
        asker: Option<u64>,
        absorption: bool,
    ) -> Result<(), ReshapeRefusal> {
        let both = [survivor, absorbed];
        if let Some(unknown) = both.iter().find(|id| !self.regions.contains_key(id)) {
            return Err(ReshapeRefusal::NoSuchRegion(*unknown));
        }
        if survivor == absorbed {
            return Err(ReshapeRefusal::Same);
        }
        if self.home == Some(absorbed) {
            return Err(ReshapeRefusal::Home);
        }
        if let Some(reserved) = both.iter().find(|id| self.reserved(**id)) {
            return Err(ReshapeRefusal::Reserved(*reserved));
        }
        if let Some(released) = both.iter().find(|id| self.releases.contains_key(id)) {
            return Err(ReshapeRefusal::BeingReleased(*released));
        }
        let owner_of = |region| self.owner_of(region).ok_or(ReshapeRefusal::NoOwner(region));
        let (owner, epoch) = owner_of(survivor)?;
        let (from, absorbed_epoch) = owner_of(absorbed)?;
        // Of the survivor's owner it is asked that it can be told and will be there.
        // The other only has to hear that it is to release.
        for (name, leaver) in [(&owner, true), (&from, false)] {
            let worker = &self.workers[name.as_str()];
            let why = if !worker.connected {
                "has no connection to the coordinator"
            } else if leaver && worker.leaving {
                "is leaving"
            } else {
                continue;
            };
            let worker = name.clone();
            return Err(ReshapeRefusal::Unfit { worker, why });
        }
        if self.last_epoch == u64::MAX {
            return Err(ReshapeRefusal::NoEpoch);
        }

        info!(%survivor, %absorbed, %owner, epoch, %from, "a region is to absorb another");
        self.pending.orders.push(ReleaseOrder {
            worker: from.clone(),
            region: absorbed,
            epoch: absorbed_epoch,
        });
        self.pending.reshapes.push(ReshapeOrder {
            worker: owner.clone(),
            order: Order::Prepare {
                region: survivor,
                epoch,
            },
        });
        let merge = Merge {
            survivor,
            owner,
            epoch,
            stage: MergeStage::Releasing {
                from,
                epoch: absorbed_epoch,
            },
            asked: now,
            asker,
            absorption,
        };
        self.merges.insert(absorbed, merge);
        self.note_begun(&both);
        Ok(())
    }

    /// Somebody wants the players standing in `chunks` split off `region` as a region
    /// of its own. `asker` is as for [`Coordinator::merge`]. The list has to have
    /// been read: the new region is to have the next id of the list as it was last
    /// handed in, so whoever asks hands in a reading first.
    ///
    /// The split is refused, and nothing changes, for the first reason of
    /// [`ReshapeRefusal`] that holds: `NoSuchRegion`, `Reserved`, `BeingReleased`,
    /// `NoOwner`, `Unfit` (the owner has no connection or is leaving), `NoChunks`,
    /// `Unlisted`, `NoEpoch`.
    ///
    /// Otherwise the split is noted as asked at `now`, the region is reserved, and its
    /// owner is to be told to split it ([`Changes::orders`]), with an epoch for the
    /// new region that is above every epoch issued or reported so far.
    pub fn split(
        &mut self,
        now: Instant,
        region: RegionId,
        chunks: &[ChunkPos],
        asker: Option<u64>,
    ) -> Result<Changes, ReshapeRefusal> {
        // As for a merge: taken before the split is checked and noted.
        let before = self.seen();
        self.begin_split(now, region, chunks, asker)?;
        Ok(self.finish(&before, now))
    }

    /// What [`Coordinator::split`] checks and notes, without what a call ends with;
    /// see [`Coordinator::begin_merge`].
    fn begin_split(
        &mut self,
        now: Instant,
        region: RegionId,
        chunks: &[ChunkPos],
        asker: Option<u64>,
    ) -> Result<(), ReshapeRefusal> {
        if !self.regions.contains_key(&region) {
            return Err(ReshapeRefusal::NoSuchRegion(region));
        }
        if self.reserved(region) {
            return Err(ReshapeRefusal::Reserved(region));
        }
        if self.releases.contains_key(&region) {
            return Err(ReshapeRefusal::BeingReleased(region));
        }
        let owner = self.owner_of(region);
        let (owner, epoch) = owner.ok_or(ReshapeRefusal::NoOwner(region))?;
        let worker = &self.workers[owner.as_str()];
        let unfit = if !worker.connected {
            Some("has no connection to the coordinator")
        } else if worker.leaving {
            Some("is leaving")
        } else {
            None
        };
        if let Some(why) = unfit {
            return Err(ReshapeRefusal::Unfit { worker: owner, why });
        }
        if chunks.is_empty() {
            return Err(ReshapeRefusal::NoChunks);
        }
        let part = self.next.ok_or(ReshapeRefusal::Unlisted)?;
        let as_epoch = self
            .last_epoch
            .checked_add(1)
            .ok_or(ReshapeRefusal::NoEpoch)?;

        self.last_epoch = as_epoch;
        info!(%region, %owner, epoch, %part, as_epoch, "a region is to be split");
        self.pending.reshapes.push(ReshapeOrder {
            worker: owner.clone(),
            order: Order::SplitOff {
                region,
                epoch,
                chunks: chunks.to_vec(),
                as_epoch,
                part,
            },
        });
        let split = Split {
            owner,
            epoch,
            as_epoch,
            chunks: chunks.to_vec(),
            asked: now,
            asker,
        };
        self.splits.insert(region, split);
        self.note_begun(&[region]);
        Ok(())
    }

    /// The worker `name` says what came of having `region` absorb `absorbed`: that it
    /// is done, or why not. Either only makes the coordinator ask for the list
    /// ([`Changes::read`]), which decides what happened; a merge the coordinator has
    /// noted, and told a worker to carry out, waits for that reading from now on and
    /// no longer for the worker. Whichever worker says it, and whether or not the
    /// coordinator has such a merge noted: a worker also says this when the world
    /// store refuses to let it open a region because it was absorbed.
    ///
    /// To say it is to be heard from, if the worker is registered.
    pub fn absorb_ended(
        &mut self,
        now: Instant,
        name: &str,
        region: RegionId,
        absorbed: RegionId,
        outcome: Result<(), Off>,
    ) -> Changes {
        let before = self.seen();
        if let Some(worker) = self.workers.get_mut(name) {
            worker.heard = worker.heard.max(now);
        }
        info!(worker = name, %region, %absorbed, ?outcome, "a worker says what came of a merge");
        if let Some(merge) = self.merges.get_mut(&absorbed)
            && merge.survivor == region
            && matches!(merge.stage, MergeStage::Absorbing { .. })
        {
            merge.stage = MergeStage::Ended { said: outcome };
        }
        self.ask_for_the_list();
        self.finish(&before, now)
    }

    /// The worker `name` says what came of the order to split `region` that named
    /// `as_epoch`: the new region, which it runs with that epoch, or why there is
    /// none.
    ///
    /// If that is the split the coordinator has noted, of that worker, the reservation
    /// ends. With a new region, the region is the worker's, with `as_epoch`, and
    /// counts as vouched for at `now` like a new assignment, unless something speaks
    /// against it as against a holding at a registration ([`Coordinator::register`]).
    ///
    /// Without such a split noted, a new region is taken as one the worker reports to
    /// run: it is the worker's unless the coordinator knows another owner of it, or
    /// a higher epoch.
    ///
    /// Either way the list is asked for ([`Changes::read`]), and without a new region
    /// until it has been read: a worker that lost the world store on the way cannot
    /// know whether the store made the split, and a part it made is a region nobody
    /// runs. To say it is to be heard from, if the worker is registered.
    pub fn split_ended(
        &mut self,
        now: Instant,
        name: &str,
        region: RegionId,
        as_epoch: u64,
        outcome: Result<RegionId, Off>,
    ) -> Changes {
        let before = self.seen();
        if let Some(worker) = self.workers.get_mut(name) {
            worker.heard = worker.heard.max(now);
        }
        info!(worker = name, %region, as_epoch, ?outcome, "a worker says what came of a split");
        if let Ok(part) = outcome
            && self.workers.contains_key(name)
        {
            let held = Assignment {
                region: part,
                epoch: as_epoch,
                entity_ids: NO_ENTITY_IDS,
            };
            self.report(now, name, &held);
        }
        let noted = self.splits.get(&region);
        if noted.is_some_and(|split| split.as_epoch == as_epoch && split.owner == name) {
            let split = self.splits.remove(&region).expect("the split was found");
            self.reshape_ended(now, &[region]);
            let outcome = outcome.map_err(Undone::Off);
            self.note_split_ended(now, region, &split, &outcome);
            self.pending.reshaped.push(Reshaped {
                asker: split.asker,
                asked: Asked::Split { region },
                outcome,
            });
        }
        // Whatever reason the worker gives: the list costs one reading where the
        // store is there, and where it is not, that is the reason.
        self.owed |= outcome.is_err();
        self.ask_for_the_list();
        self.finish(&before, now)
    }

    /// Notes that the world store's list is to be read.
    fn ask_for_the_list(&mut self) {
        self.pending.read = true;
        self.reading = true;
    }

    /// Whether `region` is part of a merge or a split under way.
    fn reserved(&self, region: RegionId) -> bool {
        let mut survivors = self.merges.values().map(|merge| merge.survivor);
        self.merges.contains_key(&region)
            || self.splits.contains_key(&region)
            || survivors.any(|survivor| survivor == region)
    }

    /// The owner of `region` and the epoch it runs it with, if the region has one.
    fn owner_of(&self, region: RegionId) -> Option<(String, u64)> {
        let state = self.regions.get(&region)?;
        let owner = state.owner.as_ref()?;
        Some((owner.worker.clone(), state.epoch))
    }

    /// What no longer holds of the merge that is to absorb `absorbed`, if anything:
    /// the region of it that is not its owner's with the epoch the merge began with,
    /// or, once the region to absorb was taken from its owner, has an owner again.
    fn broken_merge(&self, absorbed: RegionId) -> Option<Undone> {
        let merge = self.merges.get(&absorbed)?;
        if !self.holds(merge.survivor, &merge.owner, merge.epoch) {
            return Some(self.lost(merge.survivor));
        }
        let intact = match &merge.stage {
            MergeStage::Releasing { from, epoch } => self.holds(absorbed, from, *epoch),
            _ => {
                let state = self.regions.get(&absorbed);
                state.is_some_and(|state| state.owner.is_none())
            }
        };
        (!intact).then(|| self.lost(absorbed))
    }

    /// Why a merge or a split cannot go on with `region`, which is not its owner's
    /// with the epoch it had any more: a reading of the world store's list took the
    /// region away, or it lost its owner or changed hands.
    fn lost(&self, region: RegionId) -> Undone {
        if self.regions.contains_key(&region) {
            Undone::Disowned(region)
        } else {
            Undone::Gone(region)
        }
    }

    /// Ends the reservations that no longer hold: a merge whose survivor is not its
    /// owner's with the epoch it had, or whose region to absorb changed hands
    /// otherwise than by being released for it; a split whose region is not its
    /// owner's with that epoch. A merge at its first stage ends like a release that
    /// was not answered, and one at its second waits for the list, which is asked
    /// for. A split ends at once, and the list is asked for as well.
    fn end_broken_reshapes(&mut self, now: Instant) {
        let merges: Vec<RegionId> = self.merges.keys().copied().collect();
        for absorbed in merges {
            let Some(why) = self.broken_merge(absorbed) else {
                continue;
            };
            let stage = &self.merges[&absorbed].stage;
            if matches!(stage, MergeStage::Lapsed { .. }) {
                continue;
            }
            info!(region = %absorbed, ?why, "a merge no longer holds");
            self.lapse_merge(now, absorbed, why);
        }
        let broken: Vec<RegionId> = self
            .splits
            .iter()
            .filter(|(region, split)| !self.holds(**region, &split.owner, split.epoch))
            .map(|(region, _)| *region)
            .collect();
        for region in broken {
            info!(%region, "a split no longer holds, as the region is not that owner's");
            let why = self.lost(region);
            self.lapse_split(now, region, why);
        }
    }

    /// Ends the reservations of the merges and splits that were asked for more than a
    /// lease before `now`. None of them may be broken.
    fn end_overdue_reshapes(&mut self, now: Instant) {
        let lease = self.config.lease;
        let late = |asked: Instant| now.saturating_duration_since(asked) > lease;
        let merges: Vec<(RegionId, bool)> = self
            .merges
            .iter()
            .filter(|(_, merge)| late(merge.asked))
            .filter(|(_, merge)| !matches!(merge.stage, MergeStage::Lapsed { .. }))
            .map(|(absorbed, merge)| {
                let first = matches!(merge.stage, MergeStage::Releasing { .. });
                (*absorbed, first)
            })
            .collect();
        for (absorbed, first) in merges {
            warn!(region = %absorbed, "a merge was not done within the lease");
            let why = if first {
                Undone::NotReleased
            } else {
                Undone::Overdue
            };
            self.lapse_merge(now, absorbed, why);
        }
        let splits: Vec<RegionId> = self
            .splits
            .iter()
            .filter(|(_, split)| late(split.asked))
            .map(|(region, _)| *region)
            .collect();
        for region in splits {
            warn!(%region, "a worker did not say within the lease what came of a split");
            self.lapse_split(now, region, Undone::Overdue);
        }
    }

    /// The reservation of the merge that is to absorb `absorbed` ends for `why`,
    /// without a worker's word of what came of it.
    ///
    /// At the first stage the region to absorb, if it is still its owner's, is taken
    /// from it like one that was not released in time (ADR-0009), and assigned. Its
    /// owner has failed it only if that is so, and the merge's time is up
    /// ([`Undone::NotReleased`]): where the reservation ends because the survivor is
    /// no longer its owner's, or no more, the owner of the other region was asked to
    /// let go and did nothing wrong, and is not passed over for it. At the second
    /// stage the region has no owner and may have been absorbed, so the list is asked
    /// for and the merge waits for it, with both regions held back until then.
    fn lapse_merge(&mut self, now: Instant, absorbed: RegionId, why: Undone) {
        let merge = self
            .merges
            .get_mut(&absorbed)
            .expect("only a merge that is noted lapses");
        let MergeStage::Releasing { from, epoch } = &merge.stage else {
            merge.stage = MergeStage::Lapsed { why };
            self.ask_for_the_list();
            return;
        };
        let (from, epoch) = (from.clone(), *epoch);
        self.end_merge(now, absorbed, Err(why));
        if self.holds(absorbed, &from, epoch) {
            if why == Undone::NotReleased {
                self.note_failure(now, &from);
            }
            self.hand_over(now, absorbed, false);
        }
    }

    /// The reservation of the split of `region` ends for `why`, without the worker's
    /// word of what came of it. Whoever asked is told so at once, whatever the list
    /// will show: a region with the id that was ordered can be another split's, the
    /// store having made this one's under the next id, so nothing but the worker's
    /// word says that the split was done. The list is asked for all the same: what
    /// the split made, if anything, is a region nobody may be running.
    fn lapse_split(&mut self, now: Instant, region: RegionId, why: Undone) {
        let split = self
            .splits
            .remove(&region)
            .expect("only a split that is noted lapses");
        self.reshape_ended(now, &[region]);
        let outcome = Err(why);
        self.note_split_ended(now, region, &split, &outcome);
        self.pending.reshaped.push(Reshaped {
            asker: split.asker,
            asked: Asked::Split { region },
            outcome,
        });
        self.owed = true;
        self.ask_for_the_list();
    }

    /// What a reading of the list makes of the merge that is to absorb `absorbed`,
    /// given what the list has of that region.
    fn merge_listed(&mut self, now: Instant, absorbed: RegionId, fate: Fate) {
        let merge = &self.merges[&absorbed];
        let outcome = match (fate, &merge.stage) {
            (Fate::Absorbed(into), _) if into == merge.survivor => Ok(merge.survivor),
            // It went into another region, or the store has forgotten what became of
            // it: this merge did not do that.
            (Fate::Absorbed(_) | Fate::Gone, _) => Err(Undone::Gone(absorbed)),
            (Fate::Living, MergeStage::Ended { said: Err(why) }) => Err(Undone::Off(*why)),
            (Fate::Living, MergeStage::Ended { said: Ok(()) }) => Err(Undone::Contradicted),
            // The reservation has run out, so the region is not held back for a
            // reading that knows more.
            (Fate::Living | Fate::Unknown, MergeStage::Lapsed { why }) => Err(*why),
            // Under way, or a reading that is older than the region: the next one
            // says.
            (Fate::Living | Fate::Unknown, _) => return,
        };
        if matches!(fate, Fate::Living | Fate::Unknown) {
            // The worker has let go of it, or never opened it: it is nobody's.
            self.let_go(absorbed);
        }
        self.end_merge(now, absorbed, outcome);
    }

    /// Marks `region` as one that its owner let go of, if it has no owner: it is
    /// assigned at once, whatever the grace period of a new coordinator says.
    fn let_go(&mut self, region: RegionId) {
        let state = self.regions.get_mut(&region);
        if let Some(state) = state.filter(|state| state.owner.is_none()) {
            state.let_go = true;
        }
    }

    /// The merge that is to absorb `absorbed` has ended at `now`, with `outcome` for
    /// whoever asked.
    fn end_merge(&mut self, now: Instant, absorbed: RegionId, outcome: Result<RegionId, Undone>) {
        let merge = self
            .merges
            .remove(&absorbed)
            .expect("only a merge that is noted ends");
        let survivor = merge.survivor;
        info!(%survivor, %absorbed, ?outcome, "a merge has ended");
        self.reshape_ended(now, &[survivor, absorbed]);
        self.note_merge_ended(now, survivor, absorbed, merge.absorption, outcome.is_ok());
        self.pending.reshaped.push(Reshaped {
            asker: merge.asker,
            asked: Asked::Merge { survivor, absorbed },
            outcome,
        });
    }

    /// The reservation of `regions` has ended at `now`. Those of them that have an
    /// owner count as vouched for at this moment: their owner had stopped them on
    /// purpose, as the coordinator had told it to.
    fn reshape_ended(&mut self, now: Instant, regions: &[RegionId]) {
        self.reshaped = Some(self.reshaped.map_or(now, |ended| ended.max(now)));
        for region in regions {
            let state = self.regions.get_mut(region);
            if let Some(owner) = state.and_then(|state| state.owner.as_mut()) {
                owner.vouched = owner.vouched.max(now);
            }
        }
    }

    /// The region `absorbed`, which its owner has let go of for a merge, is taken from
    /// it and not assigned. It gets a new epoch, with which the survivor's owner is
    /// told to open and absorb it. The owner that let go goes behind all the other
    /// workers, as after any release.
    fn absorb_released(&mut self, now: Instant, absorbed: RegionId) {
        let state = self
            .regions
            .get_mut(&absorbed)
            .expect("a region that was released is one the coordinator knows");
        let owner = state
            .owner
            .take()
            .expect("a region that was released has an owner");
        let worker = self
            .workers
            .get_mut(&owner.worker)
            .expect("the owner of a region is a registered worker");
        worker.arrival = self.arrivals;
        self.arrivals += 1;

        let Some(as_epoch) = self.last_epoch.checked_add(1) else {
            // Nor can the region be assigned: it is without an owner for good, like
            // every region that loses its owner from now on.
            self.end_merge(now, absorbed, Err(Undone::NoEpoch));
            return;
        };
        self.last_epoch = as_epoch;
        let state = self.regions.get_mut(&absorbed);
        state.expect("the region was found above").epoch = as_epoch;
        let merge = self
            .merges
            .get_mut(&absorbed)
            .expect("the region was released for a merge that is noted");
        merge.stage = MergeStage::Absorbing { as_epoch };
        self.order_absorb(absorbed);
    }

    /// Notes that the survivor's owner is to be told to absorb `absorbed`, which has
    /// been taken from its owner for that, if the merge is at that stage.
    fn order_absorb(&mut self, absorbed: RegionId) {
        let merge = &self.merges[&absorbed];
        let MergeStage::Absorbing { as_epoch } = merge.stage else {
            return;
        };
        let order = Order::Absorb {
            region: merge.survivor,
            epoch: merge.epoch,
            absorbed,
            as_epoch,
        };
        let worker = merge.owner.clone();
        self.pending.reshapes.push(ReshapeOrder { worker, order });
    }

    /// Begins a release, as of `now`, of one region of the worker with the most regions
    /// for the one with the fewest, if the difference is two or more, no release is
    /// under way, no merge or split is or was within the last lease, and the
    /// coordinator is not new; see [`Coordinator`].
    ///
    /// A coordinator that merges and splits by itself does not wait for a lease
    /// after a merge or a split in the world, which where regions merge and split
    /// all the time would let nothing be evened out. It leaves each region alone
    /// while that one rests, and those that a merge or a split is wanted of at this
    /// tick; and of what is left it moves the region with the fewest players, as a
    /// move stands its players still (`docs/adr/0016-when-to-merge-and-split.md`,
    /// section 6).
    fn even_out(&mut self, now: Instant) {
        let lease = self.config.lease;
        let by_itself = self.config.follow.is_some();
        if self.in_grace(now) || !self.releases.is_empty() {
            return;
        }
        // A part is on the worker that made it, which has one region more for that;
        // moving it in the same breath would stand its players still twice.
        let reshaping = !self.merges.is_empty() || !self.splits.is_empty();
        let ended = self.reshaped;
        let lately = ended.is_some_and(|ended| now.saturating_duration_since(ended) < lease);
        if reshaping || (lately && !by_itself) {
            return;
        }
        // The lightest is one that is not at fault, if there is such a worker at all.
        let Some(light) = self.lightest(now, None) else {
            return;
        };
        // No release is under way, so these are the regions the workers own.
        let loads = self.loads();
        let load = |name: &str| loads.get(name).copied().unwrap_or(0);
        let heavy = self
            .workers
            .iter()
            .filter(|(_, worker)| worker.connected && !worker.leaving)
            // A region is not taken from a worker at fault either: the workers that
            // are in order are evened out among themselves.
            .filter(|(_, worker)| !self.at_fault(worker, now))
            .max_by_key(|(name, worker)| (load(name), worker.arrival))
            .map(|(name, _)| name.clone());
        let Some(heavy) = heavy else {
            return;
        };
        // With one more, the two would only change places.
        if load(&heavy) < load(&light) + 2 {
            return;
        }
        let (region, epoch) = if by_itself {
            // Every region of that worker may rest or be wanted for something, and
            // then nothing is evened out at this tick.
            let Some(moved) = self.region_to_even_out(&heavy, now) else {
                return;
            };
            moved
        } else {
            self.regions
                .iter()
                .rev()
                .find(|(_, state)| {
                    let owner = state.owner.as_ref();
                    owner.is_some_and(|owner| owner.worker == heavy)
                })
                .map(|(region, state)| (*region, state.epoch))
                .expect("a worker that has two regions more than another owns one")
        };
        info!(%region, from = %heavy, to = %light, "a region is moved to even regions out");
        self.note_release(now, region, epoch, &heavy, &light, None);
    }

    /// What a tick at `now` does.
    fn settle(&mut self, now: Instant) {
        self.forget_silent(now);
        // Before the overdue ones are ended, so that none of them takes a region from
        // an owner it was not asked of, and so that the targets of these are free.
        self.drop_stale_releases();
        self.end_overdue_releases(now);
        // In the same order, and before regions are given away: a region that was
        // held back for a merge that ends here is given away by this very call.
        self.end_broken_reshapes(now);
        self.end_overdue_reshapes(now);
        self.take_unvouched(now);
        self.assign(now);
    }

    /// Whether `region` has no owner, has not been run with an epoch above `epoch`, and
    /// `name` is a registered worker: then `name` may have been its last owner.
    fn without_owner_since(&self, region: RegionId, name: &str, epoch: u64) -> bool {
        self.workers.contains_key(name)
            // A region that waits to be absorbed has no owner on purpose, and its
            // epoch is the one it is to be opened with for that.
            && !self.merges.contains_key(&region)
            && self
                .regions
                .get(&region)
                .is_some_and(|state| state.owner.is_none() && state.epoch <= epoch)
    }

    /// Whether the worker `name` owns `region` with `epoch`.
    fn holds(&self, region: RegionId, name: &str, epoch: u64) -> bool {
        self.regions.get(&region).is_some_and(|state| {
            state.epoch == epoch
                && state
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.worker == name)
        })
    }

    /// What keeps the worker `name` from being the target of a release by `owner`, as
    /// the end of a sentence about it; `None` if it is a target. How many regions it
    /// runs does not come into it.
    fn unfit(&self, name: &str, owner: &str) -> Option<&'static str> {
        let Some(worker) = self.workers.get(name) else {
            return Some("is not registered");
        };
        if name == owner {
            return Some("owns the region");
        }
        if !worker.connected {
            return Some("has no connection to the coordinator");
        }
        if worker.leaving {
            return Some("is leaving");
        }
        None
    }

    /// How many regions each worker has that has any: those it owns and those it is
    /// the reserved target of, which it is being given just now.
    fn loads(&self) -> BTreeMap<&str, usize> {
        let owned = self
            .regions
            .values()
            .filter_map(|state| state.owner.as_ref())
            .map(|owner| owner.worker.as_str());
        let reserved = self.releases.values().map(|release| release.to.as_str());
        let mut loads = BTreeMap::new();
        for name in owned.chain(reserved) {
            *loads.entry(name).or_insert(0) += 1;
        }
        loads
    }

    /// Whether `worker` failed a region so short a time before `now` that the others
    /// are given regions before it.
    fn at_fault(&self, worker: &Worker, now: Instant) -> bool {
        let memory = self.config.lease * Self::FAULT_MEMORY;
        worker
            .failed
            .is_some_and(|failed| now.saturating_duration_since(failed) < memory)
    }

    /// Notes that the worker `name` failed a region at `now`. Not for a coordinator
    /// that is alone with its workers: it has nobody to give regions to before a
    /// worker that failed, and the mark would only keep it from beginning anything
    /// with the regions of the one it has (ADR-0017, section 2.3).
    fn note_failure(&mut self, now: Instant, name: &str) {
        if self.keeps_its_workers {
            return;
        }
        let worker = self
            .workers
            .get_mut(name)
            .expect("the owner of a region is a registered worker");
        worker.failed = Some(worker.failed.map_or(now, |failed| failed.max(now)));
    }

    /// The worker that is given the next region as of `now`: of those that are
    /// registered, have a connection and are not leaving, one that is not at fault if
    /// there is one; of those the one with the fewest regions, and of several such the
    /// one that has waited longest. `except` is left out, if it is given.
    fn lightest(&self, now: Instant, except: Option<&str>) -> Option<String> {
        let loads = self.loads();
        self.workers
            .iter()
            // One without a connection may be dead, which only its lease running out
            // would show: the region would stand still for that long. If it lives, it
            // registers again within a second.
            .filter(|(_, worker)| worker.connected && !worker.leaving)
            .filter(|(name, _)| except != Some(name.as_str()))
            .min_by_key(|(name, worker)| {
                let load = loads.get(name.as_str()).copied().unwrap_or(0);
                (self.at_fault(worker, now), load, worker.arrival)
            })
            .map(|(name, _)| name.clone())
    }

    /// The target of a release by `owner` as of `now` if nobody names one: of the
    /// workers that could be one, the one [`Coordinator::lightest`] picks.
    fn first_target(&self, now: Instant, owner: &str) -> Option<String> {
        self.lightest(now, Some(owner))
    }

    /// Notes that `from`, which owns `region` with `epoch`, is asked at `now` to release
    /// it for `to`, and that it is to be told so.
    fn note_release(
        &mut self,
        now: Instant,
        region: RegionId,
        epoch: u64,
        from: &str,
        to: &str,
        mover: Option<u64>,
    ) {
        info!(%region, epoch, from, to, "a region is to be released");
        let release = Release {
            epoch,
            from: from.to_owned(),
            to: to.to_owned(),
            asked: now,
            mover,
        };
        self.releases.insert(region, release);
        self.pending.orders.push(ReleaseOrder {
            worker: from.to_owned(),
            region,
            epoch,
        });
    }

    /// Takes `region` from its owner, which goes behind all the other workers, and
    /// assigns it at `now` to the target of its release if it has one that still is a
    /// target, else to the target with the fewest regions, else to nobody. A release of the region ends with
    /// that; `released` says whether the owner let go by itself.
    fn hand_over(&mut self, now: Instant, region: RegionId, released: bool) {
        let state = self
            .regions
            .get_mut(&region)
            .expect("only a region the coordinator knows is handed over");
        let owner = state
            .owner
            .take()
            .expect("only a region that has an owner is handed over");
        let worker = self
            .workers
            .get_mut(&owner.worker)
            .expect("the owner of a region is a registered worker");
        worker.arrival = self.arrivals;
        self.arrivals += 1;

        // Taken out first, so that its target is not counted as having the region
        // already if another target has to be found.
        let release = self.releases.remove(&region);
        let meant = release.as_ref().map(|release| release.to.clone());
        if let Some(release) = release {
            self.pending.ended.push((region, release, released));
        }
        let next = meant
            .filter(|to| self.unfit(to, &owner.worker).is_none())
            .or_else(|| self.first_target(now, &owner.worker));
        match next {
            Some(next) => {
                self.grant(now, region, &next);
            }
            None => {
                info!(%region, "nobody is there to be given a region that was let go");
                if released {
                    self.regions
                        .get_mut(&region)
                        .expect("the region was found above")
                        .let_go = true;
                }
            }
        }
    }

    /// Drops the releases whose region is no longer its owner's with the epoch it was
    /// asked to release. Their owners did not let go by themselves.
    fn drop_stale_releases(&mut self) {
        let stale: Vec<RegionId> = self
            .releases
            .iter()
            .filter(|(region, release)| !self.holds(**region, &release.from, release.epoch))
            .map(|(region, _)| *region)
            .collect();
        for region in stale {
            let release = self
                .releases
                .remove(&region)
                .expect("the release was there a moment ago");
            info!(
                %region,
                from = %release.from,
                epoch = release.epoch,
                "a release was dropped, as the region is no longer that owner's"
            );
            self.pending.ended.push((region, release, false));
        }
    }

    /// Ends the releases that were first asked more than a lease before `now`: their
    /// owners are taken for dead. None of the releases may be stale.
    fn end_overdue_releases(&mut self, now: Instant) {
        let lease = self.config.lease;
        let overdue: Vec<RegionId> = self
            .releases
            .iter()
            .filter(|(_, release)| now.saturating_duration_since(release.asked) > lease)
            .map(|(region, _)| *region)
            .collect();
        for region in overdue {
            if let Some(release) = self.releases.get(&region) {
                warn!(
                    %region,
                    worker = %release.from,
                    epoch = release.epoch,
                    "a region was not released within the lease and is taken from its owner"
                );
                // A worker that does not answer may not be able to run a region either.
                let from = release.from.clone();
                self.note_failure(now, &from);
            }
            self.hand_over(now, region, false);
        }
    }

    /// Forgets the leaving workers that own nothing; they are named to the service.
    fn forget_leavers(&mut self) {
        let owners: BTreeSet<&str> = self
            .regions
            .values()
            .filter_map(|region| region.owner.as_ref())
            .map(|owner| owner.worker.as_str())
            .collect();
        let done: Vec<String> = self
            .workers
            .iter()
            .filter(|(name, worker)| worker.leaving && !owners.contains(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        for name in done {
            info!(worker = %name, "a leaving worker owns nothing any more and is forgotten");
            self.workers.remove(&name);
            self.pending.gone.push(name);
        }
    }

    /// Begins a release, as of `now`, for each region of a leaving worker that is not
    /// being released and for which there is a target, the lowest region first. A
    /// region that is part of a merge or a split waits until that has ended.
    fn release_for_leavers(&mut self, now: Instant) {
        let wanted: Vec<(RegionId, String, u64)> = self
            .regions
            .iter()
            .filter(|(region, _)| !self.releases.contains_key(region))
            .filter(|(region, _)| !self.reserved(**region))
            .filter_map(|(region, state)| {
                let owner = state.owner.as_ref()?;
                let leaving = self.workers.get(&owner.worker)?.leaving;
                leaving.then(|| (*region, owner.worker.clone(), state.epoch))
            })
            .collect();
        for (region, from, epoch) in wanted {
            // No target owns any of these, as their owners are leaving. So a target for
            // one of them is a target for all of them, and without one there is none.
            // Each release counts for its target when the next one is looked for.
            let Some(to) = self.first_target(now, &from) else {
                break;
            };
            self.note_release(now, region, epoch, &from, &to, None);
        }
    }

    /// What every call that may have changed something ends with; see [`Coordinator`].
    /// Returns what is different from `before`, which is what the call found, and what
    /// else the service has to act on.
    fn finish(&mut self, before: &Seen, now: Instant) -> Changes {
        self.drop_stale_releases();
        self.end_broken_reshapes(now);
        self.forget_leavers();
        self.release_for_leavers(now);
        // Last of what can change an owner: a region that was given one rests.
        self.note_owners(now);

        let mut changes = self.changes_since(before);
        let pending = std::mem::take(&mut self.pending);
        changes.releases = pending.orders;
        changes.orders = pending.reshapes;
        changes.reshaped = pending.reshaped;
        changes.read = pending.read;
        changes.gone = pending.gone;
        changes.gone.sort_unstable();
        changes.gone.dedup();
        for (region, release, released) in pending.ended {
            let Some(mover) = release.mover else {
                continue;
            };
            let state = self.regions.get(&region);
            let owner = state.and_then(|state| {
                let owner = state.owner.as_ref()?;
                Some((owner.worker.clone(), state.epoch))
            });
            changes.moves.push(MoveOutcome {
                mover,
                region,
                owner,
                released,
            });
        }
        changes
    }

    /// What `name` is to run, in ascending order of the regions. Nothing, if no such
    /// worker is registered.
    pub fn assignments(&self, name: &str) -> Vec<Assignment> {
        self.regions
            .iter()
            .filter_map(|(id, region)| {
                let owner = region.owner.as_ref().filter(|owner| owner.worker == name)?;
                Some(Assignment {
                    region: *id,
                    epoch: region.epoch,
                    entity_ids: owner.entity_ids,
                })
            })
            .collect()
    }

    /// Where edges reach the owner of each region that has one, how many regions have
    /// none, and the home region and the absorbed pairs of the world store's list as
    /// it was last read. The version goes up by one with every call that changes any
    /// of it: the owner of a region, its address or its epoch, how many regions wait
    /// for an owner, the home region or the pairs.
    pub fn routing_table(&self) -> RoutingTable {
        let routes = self
            .holders()
            .into_iter()
            .map(|(region, holder)| RegionRoute {
                region,
                dimension: self
                    .regions
                    .get(&region)
                    .map_or(Dimension::Overworld, |state| state.dimension),
                epoch: holder.assignment.epoch,
                address: holder.address,
            })
            .collect();
        RoutingTable {
            home: self.home,
            homes: self.homes.clone(),
            spawn: self.spawn,
            absorbed: self.absorbed.clone(),
            waiting: self.waiting_count(),
            version: self.version,
            routes,
        }
    }

    /// The merges and splits under way, whoever asked for them: the merges in the
    /// order of the regions that are to be absorbed, then the splits in the order of
    /// their regions.
    pub fn under_way(&self) -> Vec<Asked> {
        let merges = self.merges.iter().map(|(absorbed, merge)| Asked::Merge {
            survivor: merge.survivor,
            absorbed: *absorbed,
        });
        let splits = self.splits.keys();
        let splits = splits.map(|region| Asked::Split { region: *region });
        merges.chain(splits).collect()
    }

    /// Before when the coordinator begins nothing with `region` by itself, if it has
    /// noted such a time: the end of the region's rest, or of the time it is left
    /// alone for after an attempt that failed. A coordinator that decides nothing by
    /// itself notes none. For the logs and for the tests.
    pub fn alone_until(&self, region: RegionId) -> Option<Instant> {
        self.regions.get(&region)?.kept.alone_until
    }

    /// The regions the coordinator knows that have no owner, in ascending order. A
    /// region that waits to be absorbed is among them.
    pub fn waiting(&self) -> Vec<RegionId> {
        let unowned = self.regions.iter();
        let unowned = unowned.filter(|(_, state)| state.owner.is_none());
        unowned.map(|(id, _)| *id).collect()
    }

    /// [`RoutingTable::waiting`].
    fn waiting_count(&self) -> u32 {
        let unowned = self.regions.values();
        let unowned = unowned.filter(|state| state.owner.is_none()).count();
        // There are not that many region ids.
        u32::try_from(unowned).unwrap_or(u32::MAX)
    }

    /// Takes note of a region that `name` says it runs, and lets the worker go on
    /// running it if nothing speaks against that.
    fn report(&mut self, now: Instant, name: &str, holding: &Assignment) {
        // Some coordinator issued this. Whether or not it still counts, nothing that is
        // issued from now on may be mistaken for it.
        self.last_epoch = self.last_epoch.max(holding.epoch);
        // Only so that the entity ids filled in from now on stay clear of these.
        self.used_blocks.extend(block_indices(holding.entity_ids));

        if let Some(objection) = self.objection(name, holding) {
            warn!(
                worker = name,
                region = %holding.region,
                epoch = holding.epoch,
                objection,
                "a worker may not go on running a region"
            );
            return;
        }
        // A region the coordinator does not know is one on the worker's word: it was
        // split off another, and whoever was told so is no more. A split under way
        // whose new region this is gives it the dimension of the region split, which
        // the next list says too: a region never changes dimension.
        let parent_dimension = self
            .splits
            .iter()
            .find(|(_, split)| split.as_epoch == holding.epoch)
            .and_then(|(parent, _)| self.regions.get(parent))
            .map(|parent| parent.dimension);
        let known = self.regions.contains_key(&holding.region);
        let region = self.regions.entry(holding.region).or_default();
        if !known && let Some(dimension) = parent_dimension {
            region.dimension = dimension;
        }
        region.epoch = holding.epoch;
        region.let_go = false;
        match &mut region.owner {
            // The worker's own region, so a run of waiting for the store goes on.
            Some(owner) => {
                owner.entity_ids = holding.entity_ids;
                owner.vouched = owner.vouched.max(now);
            }
            None => region.owner = Some(Owner::new(name, holding.entity_ids, now)),
        }
    }

    /// What speaks against `name` going on to run `holding`, if anything does.
    fn objection(&self, name: &str, holding: &Assignment) -> Option<&'static str> {
        let Some(region) = self.regions.get(&holding.region) else {
            // Nothing else is known against a region the coordinator does not know;
            // the next reading of the list says whether there is such a region.
            let mut absorbed = self.absorbed.iter().map(|(absorbed, _)| absorbed);
            let gone = absorbed.any(|absorbed| *absorbed == holding.region);
            return gone.then_some("the region was absorbed by another");
        };
        if region
            .owner
            .as_ref()
            .is_some_and(|owner| owner.worker != name)
        {
            return Some("another worker owns the region");
        }
        // Storage and peers have seen the higher epoch and turn this one away.
        if holding.epoch < region.epoch {
            return Some("the region has had an owner with a higher epoch");
        }
        None
    }

    /// Forgets the workers that have been silent for longer than a lease as of `now`.
    /// What they owned, and what is owned by a worker that was forgotten otherwise, is
    /// without an owner again.
    ///
    /// A coordinator that is alone with its workers forgets none for being silent
    /// (ADR-0017, section 2.3). What a worker owned that was forgotten otherwise, one
    /// that said it leaves and lost its connection, is without an owner all the same.
    fn forget_silent(&mut self, now: Instant) {
        let lease = self.config.lease;
        let keeps = self.keeps_its_workers;
        self.workers.retain(|name, worker| {
            let silent = now.saturating_duration_since(worker.heard);
            if keeps || silent <= lease {
                return true;
            }
            warn!(worker = %name, ?silent, "the lease of a worker ran out");
            false
        });
        for (id, region) in &mut self.regions {
            let forgotten = |owner: &mut Owner| !self.workers.contains_key(&owner.worker);
            if let Some(owner) = region.owner.take_if(forgotten) {
                info!(
                    region = %id,
                    worker = %owner.worker,
                    epoch = region.epoch,
                    "a region lost its owner"
                );
            }
        }
    }

    /// Takes regions from owners that no longer vouch for them as of `now`. Those
    /// regions are without an owner again. A region that is being released is left
    /// alone, and so is one that is part of a merge or a split: its owner has stopped
    /// ticking on purpose.
    ///
    /// A coordinator that is alone with its workers takes none: it could only give
    /// the region back to the worker it took it from (ADR-0017, section 2.3).
    fn take_unvouched(&mut self, now: Instant) {
        if self.keeps_its_workers {
            return;
        }
        let lease = self.config.lease;
        let survivors = self.merges.values().map(|merge| merge.survivor);
        let reserved: BTreeSet<RegionId> = survivors
            .chain(self.merges.keys().copied())
            .chain(self.splits.keys().copied())
            .collect();
        for (id, region) in &mut self.regions {
            if self.releases.contains_key(id) || reserved.contains(id) {
                continue;
            }
            let reason = region
                .owner
                .as_ref()
                .and_then(|owner| owner.unvouched(now, lease));
            let Some(reason) = reason else {
                continue;
            };
            let owner = region.owner.take().expect("only an owner can be unvouched");
            warn!(
                region = %id,
                worker = %owner.worker,
                epoch = region.epoch,
                reason,
                "a region was taken from its owner"
            );
            // Whatever keeps the worker from vouching may well keep it from running the
            // region again, so a worker that has not failed gets it first.
            let worker = self
                .workers
                .get_mut(&owner.worker)
                .expect("the owner of a region is a registered worker");
            worker.arrival = self.arrivals;
            worker.failed = Some(worker.failed.map_or(now, |failed| failed.max(now)));
            self.arrivals += 1;
        }
    }

    /// Gives each region without an owner to the worker that has the fewest regions
    /// when its turn comes, as of `now`, the lowest region first. A worker that is
    /// leaving or has no connection is given nothing. During the grace period only the
    /// regions that their owners let go of are given. A region that waits to be
    /// absorbed is not given to anyone.
    fn assign(&mut self, now: Instant) {
        let grace = self.in_grace(now);
        let unowned: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, region)| region.owner.is_none() && (region.let_go || !grace))
            .filter(|(id, _)| !self.merges.contains_key(id))
            .map(|(id, _)| *id)
            .collect();
        for id in unowned {
            // Who can be given a region does not change as these are given.
            let Some(name) = self.lightest(now, None) else {
                break;
            };
            // Epochs do not come back once they have run out, so there is nothing to
            // assign the other regions with either.
            if !self.grant(now, id, &name) {
                break;
            }
        }
    }

    /// Makes the worker `name` the owner of `region` at `now`, with an epoch above
    /// every epoch issued or reported so far. Returns whether there was such an epoch;
    /// if not, nothing changes.
    fn grant(&mut self, now: Instant, id: RegionId, name: &str) -> bool {
        let Some(epoch) = self.last_epoch.checked_add(1) else {
            return false;
        };
        self.last_epoch = epoch;
        let entity_ids = self.fill_entity_ids();
        let region = self
            .regions
            .get_mut(&id)
            .expect("the region is one the coordinator knows");
        region.epoch = epoch;
        region.owner = Some(Owner::new(name, entity_ids, now));
        region.let_go = false;
        info!(region = %id, worker = %name, epoch, "a region was assigned");
        true
    }

    /// The entity ids of a new assignment: the first block that was never issued or
    /// reported, as before the world store issued them. Once those have run out, the
    /// first block that no owner has, and after that none at all. Entity ids never stop
    /// a region from being assigned.
    fn fill_entity_ids(&mut self) -> EntityIds {
        if let Some((index, entity_ids)) = unused_block(&self.used_blocks) {
            self.used_blocks.insert(index);
            return entity_ids;
        }
        // The lowest block no owner's ids reach into. A worker may report any range, so
        // the ranges are walked rather than spread out into their blocks.
        let mut held: Vec<Range<u32>> = self
            .regions
            .values()
            .filter_map(|region| region.owner.as_ref())
            .map(|owner| block_indices(owner.entity_ids))
            .filter(|indices| !indices.is_empty())
            .collect();
        held.sort_by_key(|indices| indices.start);
        let mut lowest = 0;
        for indices in held {
            if indices.start > lowest {
                break;
            }
            lowest = lowest.max(indices.end);
        }
        let none = EntityIds {
            first: EntityId(1),
            end: EntityId(1),
        };
        EntityIds::block(lowest).unwrap_or(none)
    }

    /// The owner of every region that has one.
    fn holders(&self) -> BTreeMap<RegionId, Holder> {
        self.regions
            .iter()
            .filter_map(|(id, region)| {
                let owner = region.owner.as_ref()?;
                let worker = self
                    .workers
                    .get(&owner.worker)
                    .expect("the owner of a region is a registered worker");
                let holder = Holder {
                    worker: owner.worker.clone(),
                    address: worker.address.clone(),
                    assignment: Assignment {
                        region: *id,
                        epoch: region.epoch,
                        entity_ids: owner.entity_ids,
                    },
                };
                Some((*id, holder))
            })
            .collect()
    }

    /// What can be seen of the regions now.
    fn seen(&self) -> Seen {
        Seen {
            holders: self.holders(),
            waiting: self.waiting_count(),
        }
    }

    /// What is different from `before`, which is what the call found. Moves the
    /// routing table on to its next version if it is among that.
    fn changes_since(&mut self, before: &Seen) -> Changes {
        let after = self.holders();
        let mut workers = BTreeSet::new();
        // What the list changed of the table shows in no owner.
        let mut routing = self.pending.relisted || before.waiting != self.waiting_count();
        let before = &before.holders;
        // A region may have come or gone with the call.
        let regions: BTreeSet<&RegionId> = before.keys().chain(after.keys()).collect();
        for region in regions {
            let (was, is) = (before.get(region), after.get(region));
            // The old owner no longer has this assignment and the new one did not have
            // it, even if they are the same worker.
            if was.map(Holder::to_worker) != is.map(Holder::to_worker) {
                workers.extend(was.map(|holder| holder.worker.clone()));
                workers.extend(is.map(|holder| holder.worker.clone()));
            }
            routing |= was.map(Holder::to_edges) != is.map(Holder::to_edges);
        }
        if routing {
            // A version cannot get this high by counting; it only must not wrap.
            self.version = self.version.saturating_add(1);
        }
        Changes {
            workers: workers.into_iter().collect(),
            routing,
            ..Changes::default()
        }
    }
}

/// The indices of the blocks [`EntityIds::block`] makes that share an id with `ids`.
/// For a block made by it that is its own index; a worker may report anything, though.
fn block_indices(ids: EntityIds) -> Range<u32> {
    let (first, end) = (ids.first.0, ids.end.0);
    if first >= end {
        return 0..0;
    }
    let index = |id: u64| (id / EntityIds::BLOCK_SIZE) as u32;
    index(first)..index(end - 1).saturating_add(1)
}

/// The entity id block with the lowest index that is not in `used`, if any is left.
fn unused_block(used: &BTreeSet<u32>) -> Option<(u32, EntityIds)> {
    let index = (0..EntityIds::BLOCK_COUNT).find(|index| !used.contains(index))?;
    Some((index, EntityIds::block(index)?))
}

#[cfg(test)]
mod tests;
