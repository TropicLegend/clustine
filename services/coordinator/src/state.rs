//! Which worker runs which region: the coordinator's decisions as a state machine.
//!
//! Nothing here does I/O or reads a clock. Whoever drives the coordinator passes the
//! time in, so the same calls always lead to the same decisions and each of them can be
//! tested. The service turns messages into calls and [`Changes`] into messages.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use clustine_region::{Layout, RegionId, RegionRoute, RoutingTable};
use clustine_rpc::{Assignment, Decline, Off, PlayersOf, RegionList, Vouch};
use clustine_world::{ChunkPos, EntityId, EntityIds, Vec3};
use tracing::{info, warn};

use crate::policy::Policy;

/// What a coordinator is created with.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordinatorConfig {
    /// How the world is divided into regions.
    pub layout: Layout,
    /// Where players enter the world.
    pub spawn: Vec3,
    /// How long a worker may be silent before it loses its regions, and how long a
    /// region may go without being vouched for before it loses its owner. A new
    /// coordinator also waits this long before it gives any region away; see
    /// [`Coordinator::new`].
    pub lease: Duration,
    /// What the coordinator goes by when it merges and splits regions by itself, or
    /// `None` for one that does so only when somebody asks. See
    /// `docs/adr/0016-when-to-merge-and-split.md`, section 8. Nothing reads it yet:
    /// with either, regions are merged and split by hand.
    pub follow: Option<Policy>,
}

/// Why a worker's registration is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(
        "the worker's layout has the fingerprint {reported:#018x}, \
         but the world is divided by one with {expected:#018x}"
    )]
    Layout { reported: u64, expected: u64 },
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
    /// When it was asked for.
    asked: Instant,
    /// Who asked, if somebody did.
    asker: Option<u64>,
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
/// From then on it is a release like any other, and the next one is begun by a tick
/// after it has ended. Who brought the difference about makes no difference: a move
/// that somebody asked for and that leaves a worker two regions ahead of another is
/// evened out again.
///
/// # The regions it knows, and the world store's list
///
/// See `docs/adr/0014-merging-and-splitting.md`, section 5. The regions of the layout
/// are known from the start. Since regions merge and split, the world store has a list
/// of those there are, which the service reads and hands to [`Coordinator::listed`]:
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
#[derive(Debug, Clone)]
pub struct Coordinator {
    config: CoordinatorConfig,
    /// [`Layout::fingerprint`] of the layout.
    fingerprint: u64,
    /// When the coordinator was created.
    started: Instant,
    /// The registered workers by name.
    workers: BTreeMap<String, Worker>,
    /// How many workers have registered, not counting those that were registered
    /// already.
    arrivals: u64,
    /// Every region the coordinator knows: those of the layout, those the world
    /// store's list showed, and those workers reported, less those a reading of the
    /// list took away.
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

    /// A coordinator that knows of no worker yet.
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
        let regions = config
            .layout
            .regions()
            .map(|(id, _)| (id, Region::default()))
            .collect();
        Self {
            fingerprint: config.layout.fingerprint(),
            config,
            started: now,
            workers: BTreeMap::new(),
            arrivals: 0,
            regions,
            last_epoch: first_epoch,
            used_blocks: BTreeSet::new(),
            version: first_epoch,
            releases: BTreeMap::new(),
            home: None,
            absorbed: Vec::new(),
            next: None,
            merges: BTreeMap::new(),
            splits: BTreeMap::new(),
            reshaped: None,
            reading: false,
            owed: false,
            pending: Pending::default(),
        }
    }

    /// What the coordinator was created with.
    pub fn config(&self) -> &CoordinatorConfig {
        &self.config
    }

    /// A worker offers to run regions, or is back after losing its connection.
    ///
    /// It is refused if `layout`, the fingerprint of the layout it works with, is not
    /// that of the coordinator's layout; nothing changes then. Otherwise it is
    /// registered under `name`. If a worker of that name is registered already, this is
    /// taken to be the same worker: edges are sent to `address` from now on, and what it
    /// owns stays.
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
    /// nobody. The next reading of the list says whether it is.
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
        layout: Option<u64>,
    ) -> Result<Changes, Refusal> {
        if let Some(reported) = layout.filter(|reported| *reported != self.fingerprint) {
            let expected = self.fingerprint;
            warn!(
                worker = name,
                reported, expected, "refused a worker that divides the world differently"
            );
            return Err(Refusal::Layout { reported, expected });
        }

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
        Ok(self.finish(&before, now))
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
    /// A coordinator that decides nothing by itself keeps nothing of it, and none
    /// decides by itself yet.
    pub fn players(&mut self, now: Instant, name: &str, regions: &[PlayersOf]) -> bool {
        let Some(worker) = self.workers.get_mut(name) else {
            return false;
        };
        worker.heard = worker.heard.max(now);
        // Nothing is kept of where the players are.
        let _ = regions;
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
            self.assign(now);
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
    ///
    /// Last of all, and only here, a release is begun to even regions out, if a worker
    /// runs two regions more than another, no release is under way, and no merge or
    /// split is or was within the last lease; see [`Coordinator`]. The worker that is
    /// to release is named in [`Changes::releases`].
    pub fn tick(&mut self, now: Instant) -> Changes {
        let before = self.seen();
        self.settle(now);
        // Not part of what other calls end like a tick with: those give away what
        // has no owner, and this takes a region from one that runs it.
        self.even_out(now);
        let said = |merge: &Merge| matches!(merge.stage, MergeStage::Ended { .. });
        if !self.reading && (self.owed || self.merges.values().any(said)) {
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
        for id in gone {
            let state = self.regions.remove(&id);
            let owner = state.and_then(|state| state.owner);
            let owner = owner.map(|owner| owner.worker);
            info!(
                region = %id,
                into = ?absorbed.get(&id),
                ?owner,
                "the world store has a region no longer"
            );
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

        let home = Some(list.home);
        self.pending.relisted |= self.home != home || self.absorbed != list.absorbed;
        self.home = home;
        self.absorbed.clone_from(&list.absorbed);
        self.next = Some(list.next);

        self.assign(now);
        self.finish(&before, now)
    }

    /// The world store's list of regions could not be read. What is said of readings
    /// in [`Coordinator::listed`] holds of this one too.
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
        self.begin_merge(now, survivor, absorbed, asker)?;
        Ok(self.finish(&before, now))
    }

    /// What [`Coordinator::merge`] checks and notes, without what a call ends with:
    /// a tick that begins a merge by itself does this between what it found and
    /// what it ends with, like everything else it does.
    fn begin_merge(
        &mut self,
        now: Instant,
        survivor: RegionId,
        absorbed: RegionId,
        asker: Option<u64>,
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
        };
        self.merges.insert(absorbed, merge);
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
            asked: now,
            asker,
        };
        self.splits.insert(region, split);
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
            self.pending.reshaped.push(Reshaped {
                asker: split.asker,
                asked: Asked::Split { region },
                outcome: outcome.map_err(Undone::Off),
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
        self.pending.reshaped.push(Reshaped {
            asker: split.asker,
            asked: Asked::Split { region },
            outcome: Err(why),
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
    fn even_out(&mut self, now: Instant) {
        let lease = self.config.lease;
        let grace = now.saturating_duration_since(self.started) < lease;
        if grace || !self.releases.is_empty() {
            return;
        }
        // A part is on the worker that made it, which has one region more for that;
        // moving it in the same breath would stand its players still twice.
        let reshaping = !self.merges.is_empty() || !self.splits.is_empty();
        let ended = self.reshaped;
        let lately = ended.is_some_and(|ended| now.saturating_duration_since(ended) < lease);
        if reshaping || lately {
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
        let (region, epoch) = self
            .regions
            .iter()
            .rev()
            .find(|(_, state)| {
                let owner = state.owner.as_ref();
                owner.is_some_and(|owner| owner.worker == heavy)
            })
            .map(|(region, state)| (*region, state.epoch))
            .expect("a worker that has two regions more than another owns one");
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

    /// Notes that the worker `name` failed a region at `now`.
    fn note_failure(&mut self, now: Instant, name: &str) {
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
                epoch: holder.assignment.epoch,
                address: holder.address,
            })
            .collect();
        RoutingTable {
            home: self.home,
            absorbed: self.absorbed.clone(),
            waiting: self.waiting_count(),
            version: self.version,
            layout: self.config.layout.clone(),
            spawn: self.config.spawn,
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
        // split off another, and whoever was told so is no more.
        let region = self.regions.entry(holding.region).or_default();
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
    fn forget_silent(&mut self, now: Instant) {
        let lease = self.config.lease;
        self.workers.retain(|name, worker| {
            let silent = now.saturating_duration_since(worker.heard);
            if silent > lease {
                warn!(worker = %name, ?silent, "the lease of a worker ran out");
            }
            silent <= lease
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
    fn take_unvouched(&mut self, now: Instant) {
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
        let grace = now.saturating_duration_since(self.started) < self.config.lease;
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
        let held: BTreeSet<u32> = self
            .regions
            .values()
            .filter_map(|region| region.owner.as_ref())
            .flat_map(|owner| block_indices(owner.entity_ids))
            .collect();
        let none = EntityIds {
            first: EntityId(1),
            end: EntityId(1),
        };
        unused_block(&held).map_or(none, |(_, entity_ids)| entity_ids)
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
    // No block has a negative id.
    let first = ids.first.0.max(0);
    let end = ids.end.0;
    if first >= end {
        return 0..0;
    }
    let index = |id: i32| (id / EntityIds::BLOCK_SIZE).unsigned_abs();
    index(first)..index(end - 1) + 1
}

/// The entity id block with the lowest index that is not in `used`, if any is left.
fn unused_block(used: &BTreeSet<u32>) -> Option<(u32, EntityIds)> {
    let index = (0..EntityIds::BLOCK_COUNT).find(|index| !used.contains(index))?;
    Some((index, EntityIds::block(index)?))
}

#[cfg(test)]
mod tests {
    use clustine_rpc::RegionInfo;
    use clustine_world::EntityId;

    use super::*;

    /// The lease of the coordinators in these tests, in milliseconds. The tests give
    /// every time in milliseconds since the first coordinator was created.
    const LEASE: u64 = 10_000;

    /// What a coordinator is given as its first epoch unless a test says otherwise.
    const FIRST_EPOCH: u64 = 1000;

    const SPAWN: Vec3 = Vec3::new(0.5, -60.0, 0.5);

    /// What can be seen of a coordinator from outside.
    #[derive(Debug, Clone, PartialEq)]
    struct View {
        /// The assignments of every worker that has ever registered.
        assignments: BTreeMap<String, Vec<Assignment>>,
        table: RoutingTable,
    }

    impl View {
        fn assignments(&self, name: &str) -> &[Assignment] {
            self.assignments.get(name).map_or(&[], Vec::as_slice)
        }
    }

    /// A coordinator and what the workers told it. Every call goes through here and is
    /// checked: what the coordinator says it changed must be exactly what can be seen to
    /// have changed, and what can be seen must be in order.
    struct Cluster {
        coordinator: Coordinator,
        layout: Layout,
        start: Instant,
        /// The address each worker gave when it last registered.
        addresses: BTreeMap<String, String>,
        /// What the last call that may change something said it changed.
        last: Option<Changes>,
    }

    impl Cluster {
        /// A new coordinator for a world with region boundaries at these chunk x
        /// coordinates.
        fn new(boundaries: &[i32]) -> Self {
            Self::with_first_epoch(boundaries, FIRST_EPOCH)
        }

        fn with_first_epoch(boundaries: &[i32], first_epoch: u64) -> Self {
            let layout = Layout::new(boundaries.to_vec()).unwrap();
            let start = Instant::now();
            Self {
                coordinator: coordinator(&layout, start, first_epoch),
                layout,
                start,
                addresses: BTreeMap::new(),
                last: None,
            }
        }

        /// Replaces the coordinator by a new one, which knows nothing of what was.
        fn restart(&mut self, at: u64, first_epoch: u64) {
            self.coordinator = coordinator(&self.layout, self.at(at), first_epoch);
        }

        fn at(&self, milliseconds: u64) -> Instant {
            self.start + Duration::from_millis(milliseconds)
        }

        /// Registers a worker that has the coordinator's layout.
        fn register(
            &mut self,
            at: u64,
            name: &str,
            address: &str,
            holding: &[Assignment],
        ) -> Changes {
            let layout = Some(self.layout.fingerprint());
            self.register_as(at, name, address, holding, layout)
                .unwrap()
        }

        fn register_as(
            &mut self,
            at: u64,
            name: &str,
            address: &str,
            holding: &[Assignment],
            layout: Option<u64>,
        ) -> Result<Changes, Refusal> {
            let before = self.view();
            let now = self.at(at);
            let mut releases = self.coordinator.releases.values();
            let asked = releases.any(|release| release.from == name);
            // Or it has a part in a merge or a split, which what it reports can end.
            let reshaping = self.coordinator.merges.values().any(|merge| {
                let releasing =
                    matches!(&merge.stage, MergeStage::Releasing { from, .. } if from == name);
                releasing || merge.owner == name
            }) || self
                .coordinator
                .splits
                .values()
                .any(|split| split.owner == name);
            let result = self
                .coordinator
                .register(now, name, address, holding, layout);
            match &result {
                Ok(changes) => {
                    self.addresses.insert(name.to_owned(), address.to_owned());
                    self.verify(&before, changes);
                    // A registration is nobody else's business, unless the worker
                    // was asked to release a region and comes back without it.
                    let own = changes.workers.iter().all(|worker| worker == name);
                    assert!(asked || reshaping || own);
                }
                Err(_) => assert_eq!(self.view(), before, "a refusal changed something"),
            }
            result
        }

        /// A heartbeat that vouches for everything the worker owns as committed, which
        /// is what a worker that is in order says.
        fn heartbeat(&mut self, at: u64, name: &str) -> bool {
            let regions: Vec<(RegionId, Vouch)> = self
                .assignments(name)
                .iter()
                .map(|held| (held.region, Vouch::Committed))
                .collect();
            self.heartbeat_with(at, name, &regions)
        }

        fn heartbeat_with(&mut self, at: u64, name: &str, regions: &[(RegionId, Vouch)]) -> bool {
            let before = self.view();
            let known = self.coordinator.heartbeat(self.at(at), name, regions);
            assert_eq!(self.view(), before, "a heartbeat changed something");
            known
        }

        /// The worker says where the players of `regions` are.
        fn players(&mut self, at: u64, name: &str, regions: &[PlayersOf]) -> bool {
            let before = self.view();
            let known = self.coordinator.players(self.at(at), name, regions);
            assert_eq!(self.view(), before, "word of players changed something");
            known
        }

        fn epoch_refused(&mut self, at: u64, name: &str, region: u32, seen: u64) -> Changes {
            let before = self.view();
            let now = self.at(at);
            let changes = self
                .coordinator
                .epoch_refused(now, name, RegionId(region), seen);
            self.verify(&before, &changes);
            changes
        }

        fn tick(&mut self, at: u64) -> Changes {
            let before = self.view();
            let changes = self.coordinator.tick(self.at(at));
            self.verify(&before, &changes);
            changes
        }

        fn move_region(
            &mut self,
            at: u64,
            region: u32,
            to: Option<&str>,
            mover: u64,
        ) -> Result<(MoveBegun, Changes), MoveRefusal> {
            let before = self.view();
            let releases = self.coordinator.releases.len();
            let now = self.at(at);
            let result = self
                .coordinator
                .move_region(now, RegionId(region), to, mover);
            // Whether or not it begins, asking for a move changes no owner.
            assert_eq!(self.view(), before, "asking for a move changed something");
            match &result {
                Ok((begun, changes)) => {
                    self.verify(&before, changes);
                    let order = ReleaseOrder {
                        worker: begun.from.clone(),
                        region: RegionId(region),
                        epoch: self.coordinator.releases[&RegionId(region)].epoch,
                    };
                    assert_eq!(changes.releases, [order]);
                    assert!(changes.moves.is_empty() && changes.gone.is_empty());
                }
                Err(_) => assert_eq!(self.coordinator.releases.len(), releases),
            }
            result
        }

        fn released(&mut self, at: u64, name: &str, region: u32, epoch: u64) -> Changes {
            let before = self.view();
            let now = self.at(at);
            let changes = self
                .coordinator
                .released(now, name, RegionId(region), epoch);
            self.verify(&before, &changes);
            changes
        }

        fn leaving(&mut self, at: u64, name: &str) -> Changes {
            let before = self.view();
            let changes = self.coordinator.leaving(self.at(at), name);
            self.verify(&before, &changes);
            // To say that one leaves takes no region from anybody.
            assert_eq!(self.view(), before, "leaving changed an owner");
            changes
        }

        fn disconnected(&mut self, at: u64, name: &str) -> Changes {
            let before = self.view();
            let changes = self.coordinator.disconnected(self.at(at), name);
            self.verify(&before, &changes);
            changes
        }

        fn listed(&mut self, at: u64, list: &RegionList) -> Changes {
            let before = self.view();
            let changes = self.coordinator.listed(self.at(at), list);
            self.verify(&before, &changes);
            // The table has what the list had, and every reservation that waited for
            // a reading has had it.
            let table = self.table();
            assert_eq!(table.home, Some(list.home));
            assert_eq!(table.absorbed, list.absorbed);
            assert!(!changes.read, "{changes:?}");
            changes
        }

        fn unlisted(&mut self, at: u64) -> Changes {
            let before = self.view();
            let changes = self.coordinator.unlisted(self.at(at));
            self.verify(&before, &changes);
            assert!(!changes.read, "{changes:?}");
            changes
        }

        fn merge(
            &mut self,
            at: u64,
            survivor: u32,
            absorbed: u32,
            asker: Option<u64>,
        ) -> Result<Changes, ReshapeRefusal> {
            let before = self.view();
            let reserved = self.reservations();
            let (survivor, absorbed) = (RegionId(survivor), RegionId(absorbed));
            let now = self.at(at);
            let result = self.coordinator.merge(now, survivor, absorbed, asker);
            // Whether or not it begins, asking for a merge changes no owner.
            assert_eq!(self.view(), before, "asking for a merge changed something");
            match &result {
                Ok(changes) => self.verify(&before, changes),
                Err(_) => assert_eq!(self.reservations(), reserved),
            }
            result
        }

        fn split(
            &mut self,
            at: u64,
            region: u32,
            chunks: &[ChunkPos],
            asker: Option<u64>,
        ) -> Result<Changes, ReshapeRefusal> {
            let before = self.view();
            let reserved = self.reservations();
            let now = self.at(at);
            let result = self.coordinator.split(now, RegionId(region), chunks, asker);
            assert_eq!(self.view(), before, "asking for a split changed something");
            match &result {
                Ok(changes) => self.verify(&before, changes),
                Err(_) => assert_eq!(self.reservations(), reserved),
            }
            result
        }

        fn absorb_ended(
            &mut self,
            at: u64,
            name: &str,
            region: u32,
            absorbed: u32,
            outcome: Result<(), Off>,
        ) -> Changes {
            let before = self.view();
            let (region, absorbed) = (RegionId(region), RegionId(absorbed));
            let now = self.at(at);
            let changes = self
                .coordinator
                .absorb_ended(now, name, region, absorbed, outcome);
            self.verify(&before, &changes);
            // It decides nothing: the list does.
            assert_eq!(
                self.view(),
                before,
                "a worker's word of a merge changed something"
            );
            assert!(changes.read && changes.reshaped.is_empty(), "{changes:?}");
            changes
        }

        fn split_ended(
            &mut self,
            at: u64,
            name: &str,
            region: u32,
            as_epoch: u64,
            outcome: Result<u32, Off>,
        ) -> Changes {
            let before = self.view();
            let now = self.at(at);
            let outcome = outcome.map(RegionId);
            let changes =
                self.coordinator
                    .split_ended(now, name, RegionId(region), as_epoch, outcome);
            self.verify(&before, &changes);
            assert!(changes.read, "{changes:?}");
            changes
        }

        /// The regions that are part of a merge or a split under way.
        fn reservations(&self) -> Vec<RegionId> {
            let known = self.coordinator.regions.keys();
            let reserved = known.filter(|region| self.coordinator.reserved(**region));
            reserved.copied().collect()
        }

        fn assignments(&self, name: &str) -> Vec<Assignment> {
            self.coordinator.assignments(name)
        }

        fn table(&self) -> RoutingTable {
            self.coordinator.routing_table()
        }

        fn view(&self) -> View {
            let assignments = self
                .addresses
                .keys()
                .map(|name| (name.clone(), self.coordinator.assignments(name)))
                .collect();
            View {
                assignments,
                table: self.coordinator.routing_table(),
            }
        }

        /// Checks that `changes` is exactly what a call changed, given what could be
        /// seen before it, and that what can be seen now is in order.
        fn verify(&mut self, before: &View, changes: &Changes) {
            self.last = Some(changes.clone());
            let now = self.view();
            let changed: Vec<String> = now
                .assignments
                .keys()
                .filter(|name| before.assignments(name) != now.assignments(name))
                .cloned()
                .collect();
            assert_eq!(changes.workers, changed);
            // The table has a new version exactly when anything else of it is new.
            let unchanged = RoutingTable {
                version: now.table.version,
                ..before.table.clone()
            };
            assert_eq!(changes.routing, unchanged != now.table);
            assert_eq!(
                now.table.version,
                before.table.version + u64::from(changes.routing)
            );

            // The table sends edges to the owner of each region that has one.
            let mut routes: Vec<RegionRoute> = Vec::new();
            for (name, assignments) in &now.assignments {
                routes.extend(assignments.iter().map(|assignment| RegionRoute {
                    region: assignment.region,
                    epoch: assignment.epoch,
                    address: self.addresses[name].clone(),
                }));
            }
            routes.sort_by_key(|route| route.region);
            assert_eq!(now.table.routes, routes);
            assert_eq!(now.table.layout, self.layout);
            assert_eq!(now.table.spawn, SPAWN);
            // And counts the regions that have none.
            let known = &self.coordinator.regions;
            assert_eq!(now.table.waiting as usize, known.len() - routes.len());
            assert_eq!(self.coordinator.waiting().len(), known.len() - routes.len());
            assert_eq!(now.table.is_complete(), known.len() == routes.len());

            let live: Vec<Assignment> = now.assignments.values().flatten().copied().collect();
            for (index, one) in live.iter().enumerate() {
                assert!(known.contains_key(&one.region), "{one:?}");
                for other in &live[index + 1..] {
                    assert_ne!(one.region, other.region, "a region has two owners");
                }
            }

            // A merge or a split never outlives the owners and epochs it began with,
            // unless the list is being read to end it; a region that waits to be
            // absorbed has no owner; and no region of one is being released as well.
            let coordinator = &self.coordinator;
            for (absorbed, merge) in &coordinator.merges {
                let lapsed = matches!(merge.stage, MergeStage::Lapsed { .. });
                assert!(
                    lapsed || coordinator.broken_merge(*absorbed).is_none(),
                    "{merge:?}"
                );
                assert!(!lapsed || coordinator.reading, "{merge:?}");
                if matches!(
                    merge.stage,
                    MergeStage::Absorbing { .. } | MergeStage::Ended { .. }
                ) {
                    assert!(now.table.route(*absorbed).is_none(), "{merge:?}");
                }
                for region in [absorbed, &merge.survivor] {
                    assert!(!coordinator.releases.contains_key(region), "{merge:?}");
                    assert!(!coordinator.splits.contains_key(region), "{merge:?}");
                }
                let others = coordinator
                    .merges
                    .iter()
                    .filter(|(other, _)| *other != absorbed);
                for (other, second) in others {
                    assert_ne!(merge.survivor, second.survivor, "{merge:?}");
                    assert_ne!(merge.survivor, *other, "{merge:?}");
                }
            }
            for (region, split) in &coordinator.splits {
                let held = coordinator.holds(*region, &split.owner, split.epoch);
                assert!(held, "{split:?} outlived its owner");
                assert!(!coordinator.releases.contains_key(region), "{split:?}");
            }
            // Whoever asked for a merge or a split hears of it once, when it is over.
            for reshaped in &changes.reshaped {
                let region = match reshaped.asked {
                    Asked::Merge { absorbed, .. } => absorbed,
                    Asked::Split { region } => region,
                };
                assert!(!coordinator.merges.contains_key(&region), "{reshaped:?}");
                assert!(!coordinator.splits.contains_key(&region), "{reshaped:?}");
            }
            // An order names the owner and the epoch of the region it is about.
            for order in &changes.orders {
                let (Order::Prepare { region, epoch }
                | Order::Absorb { region, epoch, .. }
                | Order::SplitOff { region, epoch, .. }) = &order.order;
                assert!(
                    coordinator.holds(*region, &order.worker, *epoch),
                    "{order:?}"
                );
            }

            // A release never outlives its owner or its epoch, and no worker is the
            // target of its own.
            for (region, release) in &self.coordinator.releases {
                let held = Assignment {
                    region: *region,
                    epoch: release.epoch,
                    entity_ids: ids(0),
                };
                let owned = self.coordinator.assignments(&release.from);
                let same =
                    |owned: &Assignment| (owned.region, owned.epoch) == (held.region, held.epoch);
                assert!(owned.iter().any(same), "{release:?} outlived its owner");
                assert_ne!(release.from, release.to, "{release:?}");
            }
            // Nothing is left over for the next call to hand out.
            let Pending {
                ended,
                orders,
                gone,
                reshapes,
                reshaped,
                read,
                relisted,
            } = &self.coordinator.pending;
            assert!(ended.is_empty() && orders.is_empty() && gone.is_empty());
            assert!(reshapes.is_empty() && reshaped.is_empty() && !read && !relisted);
            // A worker that left is forgotten, and one that is told to release owns
            // what it is told to release: for a move, or for a merge.
            for name in &changes.gone {
                assert!(!self.coordinator.workers.contains_key(name), "{name}");
            }
            for order in &changes.releases {
                let asked = match self.coordinator.releases.get(&order.region) {
                    Some(release) => (&release.from, release.epoch),
                    None => match &self.coordinator.merges[&order.region].stage {
                        MergeStage::Releasing { from, epoch } => (from, *epoch),
                        other => panic!("{order:?} for a merge that is {other:?}"),
                    },
                };
                assert_eq!((&order.worker, order.epoch), asked);
            }
            // Whoever asked for a move is told who has the region now.
            for outcome in &changes.moves {
                let route = now.table.route(outcome.region);
                let owner = outcome.owner.as_ref();
                assert_eq!(
                    owner.map(|(_, epoch)| *epoch),
                    route.map(|route| route.epoch)
                );
                if let Some((name, epoch)) = owner {
                    let has = now.assignments(name).iter();
                    let mut has = has.map(|held| (held.region, held.epoch));
                    assert!(
                        has.any(|held| held == (outcome.region, *epoch)),
                        "{outcome:?}"
                    );
                }
            }
        }
    }

    /// A worker that is to be told to release a region.
    fn order(worker: &str, region: u32, epoch: u64) -> ReleaseOrder {
        ReleaseOrder {
            worker: worker.to_owned(),
            region: RegionId(region),
            epoch,
        }
    }

    /// How the move that `mover` asked for ended.
    fn outcome(mover: u64, region: u32, owner: Option<(&str, u64)>, released: bool) -> MoveOutcome {
        MoveOutcome {
            mover,
            region: RegionId(region),
            owner: owner.map(|(name, epoch)| (name.to_owned(), epoch)),
            released,
        }
    }

    fn begun(from: &str, to: &str) -> MoveBegun {
        MoveBegun {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    /// What a call says that asks nobody but `orders` to do anything.
    fn asks(orders: &[ReleaseOrder]) -> Changes {
        Changes {
            releases: orders.to_vec(),
            ..Changes::default()
        }
    }

    fn coordinator(layout: &Layout, now: Instant, first_epoch: u64) -> Coordinator {
        let config = CoordinatorConfig {
            layout: layout.clone(),
            spawn: SPAWN,
            lease: Duration::from_millis(LEASE),
            follow: None,
        };
        Coordinator::new(config, now, first_epoch)
    }

    fn ids(block: u32) -> EntityIds {
        EntityIds::block(block).unwrap()
    }

    fn assignment(region: u32, epoch: u64, block: u32) -> Assignment {
        Assignment {
            region: RegionId(region),
            epoch,
            entity_ids: ids(block),
        }
    }

    fn route(region: u32, epoch: u64, address: &str) -> RegionRoute {
        RegionRoute {
            region: RegionId(region),
            epoch,
            address: address.to_owned(),
        }
    }

    fn changes(workers: &[&str], routing: bool) -> Changes {
        Changes {
            workers: workers.iter().map(|name| (*name).to_owned()).collect(),
            routing,
            ..Changes::default()
        }
    }

    /// The world store's list of a world that is divided into `count` stripes, none
    /// of which has been opened, merged or split: the first is the home region.
    fn stripes(count: u32) -> RegionList {
        let regions = (0..count).map(|region| living(region, 0)).collect();
        RegionList {
            home: RegionId(0),
            regions,
            absorbed: Vec::new(),
            next: RegionId(count),
        }
    }

    /// What the list has of a living region that was last opened with `epoch`. Where
    /// it lies does not concern the coordinator yet.
    fn living(region: u32, epoch: u64) -> RegionInfo {
        RegionInfo {
            region: RegionId(region),
            epoch,
            bounds: None,
            pinned: Vec::new(),
        }
    }

    #[test]
    fn two_workers_are_given_the_two_regions_in_the_order_they_registered() {
        let mut cluster = Cluster::new(&[0]);
        // Not in the order of their names.
        assert_eq!(cluster.register(1, "b", "b:25601", &[]), Changes::default());
        assert_eq!(cluster.register(2, "a", "a:25601", &[]), Changes::default());
        let table = cluster.table();
        assert_eq!(table.version, FIRST_EPOCH);
        assert!(table.routes.is_empty());
        assert!(!table.is_complete());

        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert_eq!(
            cluster.assignments("a"),
            [assignment(1, FIRST_EPOCH + 2, 1)]
        );
        let table = cluster.table();
        assert!(table.is_complete());
        assert_eq!(table.version, FIRST_EPOCH + 1);
        assert_eq!(
            table.routes,
            [
                route(0, FIRST_EPOCH + 1, "b:25601"),
                route(1, FIRST_EPOCH + 2, "a:25601"),
            ]
        );
        assert_eq!(table.layout, Layout::new(vec![0]).unwrap());
        assert_eq!(table.spawn, SPAWN);

        // There is nothing left to do.
        assert_eq!(cluster.tick(LEASE + 1), Changes::default());
        assert_eq!(cluster.table(), table);
    }

    #[test]
    fn the_table_fills_up_with_a_new_version_for_each_call_that_adds_to_it() {
        let mut cluster = Cluster::new(&[0]);
        // Nobody is there to run anything.
        assert_eq!(cluster.tick(1), Changes::default());

        // A worker that ran one of the regions under the coordinator before this one.
        let held = assignment(0, 7, 0);
        assert_eq!(
            cluster.register(1, "a", "a:25601", &[held]),
            changes(&["a"], true)
        );
        let table = cluster.table();
        assert_eq!(table.version, FIRST_EPOCH + 1);
        assert_eq!(table.routes, [route(0, 7, "a:25601")]);
        assert!(!table.is_complete());

        // The other region goes to the worker that has none.
        cluster.register(2, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        let table = cluster.table();
        assert_eq!(table.version, FIRST_EPOCH + 2);
        assert_eq!(
            table.routes,
            [route(0, 7, "a:25601"), route(1, FIRST_EPOCH + 1, "b:25601"),]
        );
        assert!(table.is_complete());
    }

    /// The layout of a world with five regions.
    const FIVE: [i32; 4] = [-16, -8, 0, 8];

    /// The numbers of the regions that the worker `name` runs.
    fn regions_of(cluster: &Cluster, name: &str) -> Vec<u32> {
        let held = cluster.assignments(name);
        held.iter().map(|held| held.region.0).collect()
    }

    #[test]
    fn fewer_workers_than_regions_are_given_every_region_spread_evenly() {
        let mut cluster = Cluster::new(&FIVE);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        // The lowest region first, each to the worker with the fewest so far, and of
        // two with equally many to the one that registered first.
        assert_eq!(regions_of(&cluster, "a"), [0, 2, 4]);
        assert_eq!(regions_of(&cluster, "b"), [1, 3]);
        let table = cluster.table();
        assert!(table.is_complete());
        // One call, one new version, and an epoch of its own for each region.
        assert_eq!(table.version, FIRST_EPOCH + 1);
        let epochs: Vec<u64> = table.routes.iter().map(|route| route.epoch).collect();
        assert_eq!(epochs, Vec::from_iter(FIRST_EPOCH + 1..=FIRST_EPOCH + 5));
        assert_eq!(cluster.tick(LEASE), Changes::default());

        // A single worker runs the whole world.
        let mut cluster = Cluster::new(&FIVE);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn the_regions_of_a_worker_whose_lease_ran_out_are_spread_over_those_that_remain() {
        let mut cluster = Cluster::new(&[-16, -8, 0, 8, 16]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
        assert_eq!(regions_of(&cluster, "c"), [2, 5]);

        for name in ["a", "b"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b", "c"], true));
        // One each, and not both to the worker that comes first.
        assert_eq!(regions_of(&cluster, "a"), [0, 2, 3]);
        assert_eq!(regions_of(&cluster, "b"), [1, 4, 5]);
        assert!(cluster.table().is_complete());
        assert!(!cluster.heartbeat(2 * LEASE + 1, "c"));

        // With one worker more than the other, the next region to lose its owner does
        // not go to the one that has more, although that one registered first.
        let mut cluster = Cluster::new(&FIVE);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 3]);
        assert_eq!(regions_of(&cluster, "b"), [1, 4]);
        assert_eq!(regions_of(&cluster, "c"), [2]);
        for name in ["a", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 3, 4]);
        assert_eq!(regions_of(&cluster, "c"), [1, 2]);
    }

    /// The worker each region that is being released is meant for.
    fn targets(cluster: &Cluster) -> Vec<(u32, &str)> {
        let releases = cluster.coordinator.releases.iter();
        Vec::from_iter(releases.map(|(region, release)| (region.0, release.to.as_str())))
    }

    /// Lets `leases` leases go by from `from` on, a quarter of a lease at a time, with
    /// every worker heard from and vouching for what it runs: no tick changes or
    /// begins anything.
    fn nothing_happens(cluster: &mut Cluster, from: u64, leases: u64, workers: &[&str]) {
        for quarter in 1..=4 * leases {
            let now = from + quarter * LEASE / 4;
            for name in workers {
                assert!(cluster.heartbeat(now, name));
            }
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
    }

    #[test]
    fn a_second_worker_is_given_one_of_the_two_regions_of_the_first_at_the_next_tick() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert!(cluster.heartbeat(LEASE, "a"));
        let table = cluster.table();

        // A worker that registers is given nothing that has an owner. The next tick
        // asks the first for one of its regions, the one with the highest number.
        assert_eq!(
            cluster.register(LEASE, "b", "b:25601", &[]),
            Changes::default()
        );
        assert!(cluster.coordinator.releases.is_empty());
        // Nor does a call that ends like a tick begin anything: only a tick does.
        assert_eq!(
            cluster.epoch_refused(LEASE, "b", 0, FIRST_EPOCH),
            Changes::default()
        );
        assert_eq!(
            cluster.tick(LEASE + 1),
            asks(&[order("a", 1, FIRST_EPOCH + 2)])
        );
        assert_eq!(targets(&cluster), [(1, "b")]);
        // Nothing has changed hands yet, and the first is asked only once.
        assert_eq!(cluster.table(), table);
        assert_eq!(cluster.tick(LEASE + 2), Changes::default());

        // It is a release like any other: nobody hears how it ended.
        assert_eq!(
            cluster.released(LEASE + 3, "a", 1, FIRST_EPOCH + 2),
            changes(&["a", "b"], true)
        );
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert_eq!(
            cluster.assignments("b"),
            [assignment(1, FIRST_EPOCH + 3, 2)]
        );
        // One each, and that is how it stays.
        nothing_happens(&mut cluster, LEASE + 3, 3, &["a", "b"]);
        assert!(cluster.coordinator.releases.is_empty());
    }

    #[test]
    fn three_regions_on_one_worker_and_a_newcomer_end_as_two_and_one_and_stay_so() {
        let mut cluster = Cluster::new(&[-8, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert!(cluster.heartbeat(LEASE, "a"));
        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(
            cluster.tick(LEASE + 1),
            asks(&[order("a", 2, FIRST_EPOCH + 3)])
        );
        assert_eq!(
            cluster.released(LEASE + 2, "a", 2, FIRST_EPOCH + 3),
            changes(&["a", "b"], true)
        );
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert_eq!(regions_of(&cluster, "b"), [2]);

        // One move, and no more: with another the two would only change places.
        nothing_happens(&mut cluster, LEASE + 2, 3, &["a", "b"]);
        assert!(cluster.coordinator.releases.is_empty());
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
    }

    #[test]
    fn a_difference_of_one_region_is_left_alone() {
        let mut cluster = Cluster::new(&[-8, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);
        assert_eq!(regions_of(&cluster, "b"), [1]);
        nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);

        // Nor is a worker that runs nothing given the only region of another.
        let mut cluster = a_runs_and_b_waits();
        nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
        assert!(cluster.coordinator.releases.is_empty());
    }

    #[test]
    fn of_the_workers_with_the_most_regions_the_one_that_waited_least_gives_up_its_highest() {
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "b"), [1, 3]);
        let last = cluster.assignments("b")[1].epoch;
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(LEASE, name));
        }

        cluster.register(LEASE, "c", "c:25601", &[]);
        assert_eq!(cluster.tick(LEASE + 1), asks(&[order("b", 3, last)]));
        assert_eq!(targets(&cluster), [(3, "c")]);
        assert_eq!(
            cluster.released(LEASE + 2, "b", 3, last),
            changes(&["b", "c"], true)
        );
        // Two, one and one: nothing more is to be done.
        nothing_happens(&mut cluster, LEASE + 2, 2, &["a", "b", "c"]);
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);
        assert_eq!(regions_of(&cluster, "c"), [3]);
    }

    /// For how long a worker that failed a region is at fault, in the milliseconds the
    /// tests give times in.
    const MEMORY: u64 = Coordinator::FAULT_MEMORY as u64 * LEASE;

    /// When `a` fails its region in [`a_fails_and_b_has_both`].
    const FAILED: u64 = 2 * LEASE + 1;

    /// A world of two regions, of which `a` and `b` ran one each until `a`, which is
    /// still there, did not vouch for its own any longer: at `FAILED` it was taken
    /// from it and given to `b`.
    fn a_fails_and_b_has_both() -> Cluster {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
        assert!(cluster.heartbeat(2 * LEASE, "b"));
        // Not back to the worker that failed it, although that one runs nothing now
        // and the other runs a region.
        assert_eq!(cluster.tick(FAILED), changes(&["a", "b"], true));
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(
            cluster.assignments("b"),
            [
                assignment(0, FIRST_EPOCH + 3, 2),
                assignment(1, FIRST_EPOCH + 2, 1)
            ]
        );
        cluster
    }

    #[test]
    fn a_worker_that_did_not_vouch_is_passed_over_for_six_leases_and_then_counts_again() {
        let mut cluster = a_fails_and_b_has_both();
        // Registering again makes no difference: a worker that lost its connection
        // does that too.
        assert_eq!(
            cluster.register(FAILED, "a", "a:25601", &[]),
            Changes::default()
        );
        // Nothing is evened out towards it either, with two regions against none.
        nothing_happens(&mut cluster, FAILED, 5, &["a", "b"]);
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
        }
        assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());

        // Six leases after it failed, it is a worker like any other.
        assert_eq!(MEMORY, 6 * LEASE);
        assert_eq!(
            cluster.tick(FAILED + MEMORY),
            asks(&[order("b", 1, FIRST_EPOCH + 2)])
        );
        assert_eq!(targets(&cluster), [(1, "a")]);
    }

    #[test]
    fn a_worker_at_fault_is_given_the_region_back_when_nobody_else_can_take_it() {
        // The other worker has no connection.
        let mut cluster = a_runs_and_b_waits();
        assert_eq!(cluster.disconnected(LEASE, "b"), Changes::default());
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(2 * LEASE, name, &[]));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a"], true));
        assert_eq!(cluster.assignments("a"), [assignment(0, NEXT_OWNER, 1)]);

        // Both are at fault, one after the other: the region goes to the one of them
        // that has waited longer since.
        let mut cluster = a_runs_and_b_waits();
        let mut now = LEASE;
        for (lost, next) in [("a", "b"), ("b", "a"), ("a", "b")] {
            now += LEASE + 1;
            for name in ["a", "b"] {
                assert!(cluster.heartbeat_with(now, name, &[]));
            }
            assert_eq!(cluster.tick(now), changes(&["a", "b"], true), "{lost}");
            assert_eq!(cluster.assignments(next).len(), 1, "{next}");
        }
    }

    #[test]
    fn a_worker_that_did_not_answer_a_release_is_passed_over_as_well() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE + 1, name));
        }
        // The owner is there and vouches, but does not answer.
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 3)), false)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 2), expected);

        // The worker that was given its region dies. The region goes to the worker
        // that runs one, not to the one that runs nothing and has waited longer.
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(3 * LEASE + 2, name));
        }
        assert_eq!(cluster.tick(3 * LEASE + 2), changes(&["b", "c"], true));
        assert_eq!(regions_of(&cluster, "b"), [0, 1]);
        assert!(cluster.assignments("a").is_empty());

        // A worker that releases when it is asked has failed nothing: it is given
        // the next region that loses its owner.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
        cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1);
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(2 * LEASE + 1, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0]);
    }

    #[test]
    fn a_worker_whose_epoch_the_store_refused_is_not_at_fault() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(LEASE, name));
        }
        // What the store has seen is news to the coordinator, not a failure of the
        // worker: it is given the region again, having fewer than the other.
        assert_eq!(
            cluster.epoch_refused(LEASE + 1, "a", 0, FIRST_EPOCH + 50),
            changes(&["a"], true)
        );
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 51, 2)]
        );
    }

    #[test]
    fn workers_that_are_in_order_are_evened_out_while_another_is_at_fault() {
        let mut cluster = Cluster::new(&[-8, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);
        // One of them stops vouching, and the other has all three regions.
        assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
        assert!(cluster.heartbeat(2 * LEASE, "b"));
        assert_eq!(cluster.tick(FAILED), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "b"), [0, 1, 2]);
        let held = cluster.assignments("b");

        // A third worker comes. A region is moved to it, and not to the one at
        // fault, which has waited longer and runs as little.
        cluster.register(FAILED, "c", "c:25601", &[]);
        assert_eq!(
            cluster.tick(FAILED + 1),
            asks(&[order("b", 2, held[2].epoch)])
        );
        assert_eq!(targets(&cluster), [(2, "c")]);
        assert_eq!(
            cluster.released(FAILED + 2, "b", 2, held[2].epoch),
            changes(&["b", "c"], true)
        );
        // Two and one among those that are in order: the third does not count.
        nothing_happens(&mut cluster, FAILED + 2, 5, &["a", "b", "c"]);
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
        }
        assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());
        // Until its fault is forgotten.
        assert_eq!(
            cluster.tick(FAILED + MEMORY),
            asks(&[order("b", 1, held[1].epoch)])
        );
        assert_eq!(targets(&cluster), [(1, "a")]);
    }

    #[test]
    fn nothing_is_evened_out_from_a_worker_at_fault() {
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        // It vouches for three of its four regions. The fourth is taken from it
        // and given back, as nobody else is there.
        let three: Vec<(RegionId, Vouch)> =
            Vec::from_iter((0..3).map(|region| (RegionId(region), Vouch::Committed)));
        assert!(cluster.heartbeat_with(2 * LEASE, "a", &three));
        assert_eq!(cluster.tick(FAILED), changes(&["a"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3]);
        let last = cluster.assignments("a")[3].epoch;

        // A worker that comes now is not handed any of them as long as the fault is
        // remembered: the workers at fault are left out of evening out altogether.
        cluster.register(FAILED, "b", "b:25601", &[]);
        nothing_happens(&mut cluster, FAILED, 5, &["a", "b"]);
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(FAILED + MEMORY - 1, name));
        }
        assert_eq!(cluster.tick(FAILED + MEMORY - 1), Changes::default());
        assert_eq!(cluster.tick(FAILED + MEMORY), asks(&[order("a", 3, last)]));
    }

    #[test]
    fn a_move_by_name_to_a_worker_at_fault_is_done_and_one_by_choice_passes_it_over() {
        let mut cluster = a_fails_and_b_has_both();
        cluster.register(FAILED, "c", "c:25601", &[]);
        // By choice the region goes to the worker that has not failed one, although
        // the other has waited longer.
        let (moving, _) = cluster.move_region(FAILED, 1, None, 7).unwrap();
        assert_eq!(moving, begun("b", "c"));
        // Whoever names the worker at fault knows better.
        let (moving, _) = cluster.move_region(FAILED, 0, Some("a"), 8).unwrap();
        assert_eq!(moving, begun("b", "a"));
        let expected = Changes {
            moves: vec![outcome(8, 0, Some(("a", FIRST_EPOCH + 4)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(
            cluster.released(FAILED + 1, "b", 0, FIRST_EPOCH + 3),
            expected
        );
        assert_eq!(regions_of(&cluster, "a"), [0]);
    }

    #[test]
    fn nothing_is_evened_out_during_the_grace_period() {
        let mut cluster = Cluster::new(&[0]);
        // One worker ran both regions under the coordinator before this one, and
        // another ran nothing.
        let held = [assignment(0, 5, 0), assignment(1, 6, 1)];
        cluster.register(0, "a", "a:25601", &held);
        cluster.register(0, "b", "b:25601", &[]);
        for now in [0, 1, LEASE / 2, LEASE - 1] {
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
            assert!(cluster.coordinator.releases.is_empty());
        }
        // Workers that have not found the new coordinator yet may be among those
        // with the most regions. After a lease they have had their chance.
        assert_eq!(cluster.tick(LEASE), asks(&[order("a", 1, 6)]));
        assert_eq!(targets(&cluster), [(1, "b")]);
    }

    #[test]
    fn nothing_is_evened_out_while_another_release_is_under_way() {
        let mut cluster = Cluster::new(&[-8, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        assert!(cluster.heartbeat(LEASE, "a"));
        cluster.register(LEASE, "b", "b:25601", &[]);
        cluster.register(LEASE, "c", "c:25601", &[]);

        // Somebody asks for a move before the next tick. While that lasts, the
        // coordinator begins none of its own, although one worker has three regions
        // and another neither has one nor is being given one.
        let (moving, _) = cluster.move_region(LEASE + 1, 0, Some("b"), 1).unwrap();
        assert_eq!(moving, begun("a", "b"));
        for now in [LEASE + 2, LEASE + 3] {
            assert_eq!(cluster.tick(now), Changes::default());
        }
        assert_eq!(targets(&cluster), [(0, "b")]);

        // When it has ended, the next tick begins one, and the tick after that
        // begins no second one beside it.
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("b", FIRST_EPOCH + 4)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 4, "a", 0, held[0].epoch), expected);
        assert_eq!(
            cluster.tick(LEASE + 5),
            asks(&[order("a", 2, held[2].epoch)])
        );
        assert_eq!(targets(&cluster), [(2, "c")]);
        assert_eq!(cluster.tick(LEASE + 6), Changes::default());
        assert_eq!(
            cluster.released(LEASE + 7, "a", 2, held[2].epoch),
            changes(&["a", "c"], true)
        );
        nothing_happens(&mut cluster, LEASE + 7, 2, &["a", "b", "c"]);
        for (name, region) in [("a", 1), ("b", 0), ("c", 2)] {
            assert_eq!(regions_of(&cluster, name), [region], "{name}");
        }
    }

    #[test]
    fn nothing_is_evened_out_from_or_to_a_worker_without_a_connection() {
        // The worker that has both regions could not be told to release one.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        assert_eq!(cluster.disconnected(LEASE, "a"), Changes::default());
        cluster.register(LEASE, "b", "b:25601", &[]);
        nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
        assert!(cluster.coordinator.releases.is_empty());
        // When it is back, it is.
        assert_eq!(
            cluster.register(3 * LEASE, "a", "a:25601", &held),
            Changes::default()
        );
        assert_eq!(
            cluster.tick(3 * LEASE),
            asks(&[order("a", 1, held[1].epoch)])
        );

        // The worker that has nothing would not hear that it was given a region.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.disconnected(LEASE, "b"), Changes::default());
        nothing_happens(&mut cluster, LEASE, 2, &["a", "b"]);
        assert!(cluster.coordinator.releases.is_empty());
    }

    #[test]
    fn nothing_is_evened_out_towards_a_leaving_worker() {
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        // One worker ran three of the regions under the coordinator before this one,
        // and another the fourth.
        let three = [
            assignment(0, 5, 0),
            assignment(1, 6, 1),
            assignment(2, 7, 2),
        ];
        cluster.register(0, "a", "a:25601", &three);
        cluster.register(0, "b", "b:25601", &[assignment(3, 8, 3)]);
        // The one with fewer leaves. Its region is to go to the other one, not the
        // other way round.
        assert_eq!(cluster.leaving(1, "b"), asks(&[order("b", 3, 8)]));
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(LEASE, name));
        }
        assert_eq!(cluster.tick(LEASE), Changes::default());
        assert_eq!(targets(&cluster), [(3, "a")]);

        // A leaving worker that owns something is being asked to release it whenever
        // another worker is there, so that alone holds the coordinator back. With
        // those releases out of the way it still begins nothing: neither towards the
        // worker that leaves, nor from it when it is the one with more regions.
        let now = cluster.at(LEASE);
        cluster.coordinator.releases.clear();
        cluster.coordinator.even_out(now);
        assert!(cluster.coordinator.releases.is_empty());
        assert!(cluster.coordinator.pending.orders.is_empty());

        let mut cluster = Cluster::new(&[-8, 0, 8]);
        cluster.register(0, "a", "a:25601", &three);
        cluster.register(0, "b", "b:25601", &[]);
        let asked = Vec::from_iter(
            three
                .iter()
                .map(|held| order("a", held.region.0, held.epoch)),
        );
        assert_eq!(cluster.leaving(1, "a"), asks(&asked));
        let now = cluster.at(LEASE);
        cluster.coordinator.releases.clear();
        cluster.coordinator.even_out(now);
        assert!(cluster.coordinator.releases.is_empty());
        assert!(cluster.coordinator.pending.orders.is_empty());
    }

    #[test]
    fn a_single_region_goes_to_the_first_worker_and_the_others_wait() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert!(cluster.assignments("b").is_empty());
        let table = cluster.table();
        assert!(table.is_complete());
        assert_eq!(table.layout, Layout::single());
        assert_eq!(table.routes, [route(0, FIRST_EPOCH + 1, "a:25601")]);

        assert!(cluster.heartbeat(LEASE, "a"));
        assert!(cluster.heartbeat(LEASE, "b"));
        assert_eq!(cluster.tick(2 * LEASE), Changes::default());
    }

    /// A waiting worker whose connection is gone may be dead, and only its lease running
    /// out would show. A region given to it would stand still for that long, so it is
    /// given to one that is there, or kept until the worker is back.
    #[test]
    fn a_region_is_not_given_to_a_waiting_worker_that_has_no_connection() {
        let mut cluster = Cluster::new(&[]);
        // The first to arrive would be the first to be given the region.
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.disconnected(0, "a"), Changes::default());
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert!(cluster.assignments("a").is_empty());

        // With nobody else there, the region waits for the worker to be back.
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert!(cluster.heartbeat(LEASE, "a"));
        assert_eq!(cluster.disconnected(LEASE, "a"), Changes::default());
        assert_eq!(cluster.tick(LEASE), Changes::default());
        assert!(cluster.table().routes.is_empty());
        cluster.register(LEASE + 1, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE + 1), changes(&["a"], true));
    }

    #[test]
    fn a_waiting_worker_takes_over_when_the_lease_of_an_owner_runs_out() {
        let mut cluster = Cluster::new(&[0]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert!(cluster.assignments("c").is_empty());

        // All three are heard from once more, then only two of them.
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(LEASE, name));
        }
        for name in ["b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        // Silence for exactly one lease is not too long.
        assert_eq!(cluster.tick(2 * LEASE), Changes::default());
        let version = cluster.table().version;

        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "c"], true));
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(
            cluster.assignments("b"),
            [assignment(1, FIRST_EPOCH + 2, 1)]
        );
        // A higher epoch than any before, and entity ids nobody has had.
        assert_eq!(
            cluster.assignments("c"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
        let table = cluster.table();
        assert_eq!(table.version, version + 1);
        assert_eq!(
            table.routes,
            [
                route(0, FIRST_EPOCH + 3, "c:25601"),
                route(1, FIRST_EPOCH + 2, "b:25601"),
            ]
        );
        // The worker that was forgotten has to register again.
        assert!(!cluster.heartbeat(2 * LEASE + 1, "a"));
    }

    #[test]
    fn heartbeats_keep_a_lease() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");

        // One heartbeat per lease is just enough.
        let mut now = 0;
        for _ in 0..50 {
            now += LEASE;
            assert!(cluster.heartbeat(now, "a"));
            assert!(cluster.heartbeat(now, "b"));
            assert_eq!(cluster.tick(now), Changes::default());
        }
        assert_eq!(cluster.assignments("a"), held);

        // Without them the region goes to the worker that waited.
        assert!(cluster.heartbeat(now + LEASE, "b"));
        assert_eq!(cluster.tick(now + LEASE + 1), changes(&["a", "b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 2, 1)]
        );
    }

    /// The store waits of the tests, in the milliseconds the tests give times in.
    const PATIENCE: u64 = Coordinator::STORE_PATIENCE.as_millis() as u64;

    const COMMITTED: [(RegionId, Vouch); 1] = [(RegionId(0), Vouch::Committed)];
    const WAITING: [(RegionId, Vouch); 1] = [(RegionId(0), Vouch::WaitingForStore)];

    /// A world of one region, which `a` runs from `LEASE` on with the epoch after the
    /// first, and `b`, which waits. Both are heard from at `LEASE`.
    fn a_runs_and_b_waits() -> Cluster {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(LEASE, name, &[]));
        }
        cluster
    }

    #[test]
    fn a_region_not_vouched_for_loses_its_owner_a_lease_after_the_last_vouch() {
        let mut cluster = a_runs_and_b_waits();
        assert!(cluster.heartbeat_with(LEASE + 3000, "a", &COMMITTED));

        // Both go on being heard from, but `a` no longer names the region.
        let mut now = LEASE + 3000;
        while now < 2 * LEASE + 3000 {
            now += 1000;
            for name in ["a", "b"] {
                assert!(cluster.heartbeat_with(now, name, &[]));
            }
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        // A lease after the vouch is not too long; a moment more is.
        assert_eq!(cluster.tick(2 * LEASE + 3001), changes(&["a", "b"], true));
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 2, 1)]
        );
        // The worker that lost it is still registered, and waits.
        assert!(cluster.heartbeat_with(2 * LEASE + 3001, "a", &COMMITTED));
        assert!(cluster.assignments("a").is_empty());
    }

    #[test]
    fn a_freshly_assigned_region_is_not_taken_away_within_its_first_lease() {
        let mut cluster = a_runs_and_b_waits();
        // The new owner is opening and restoring the region, and says nothing of it.
        for now in [LEASE + 1, LEASE + LEASE / 2, 2 * LEASE - 1, 2 * LEASE] {
            for name in ["a", "b"] {
                assert!(cluster.heartbeat_with(now, name, &[]));
            }
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
    }

    #[test]
    fn a_region_reported_at_a_registration_counts_as_vouched_for_then() {
        let mut cluster = Cluster::new(&[]);
        let held = assignment(0, 7, 0);
        cluster.register(0, "a", "a:25601", &[held]);
        cluster.register(3000, "a", "a:25601", &[held]);
        assert!(cluster.heartbeat_with(LEASE + 3000, "a", &[]));
        assert_eq!(cluster.tick(LEASE + 3000), Changes::default());
        assert_eq!(cluster.tick(LEASE + 3001), changes(&["a"], true));
    }

    #[test]
    fn waiting_for_the_store_keeps_a_region_for_thirty_seconds_and_no_longer() {
        let mut cluster = a_runs_and_b_waits();
        // The run begins with the first heartbeat that says so, and every one of them
        // until thirty seconds after it counts, however long that is in leases.
        let start = LEASE + 2000;
        let mut now = start;
        while now <= start + PATIENCE {
            assert!(cluster.heartbeat_with(now, "a", &WAITING));
            assert!(cluster.heartbeat_with(now, "b", &[]));
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
            now += LEASE / 2;
        }
        assert!(cluster.heartbeat_with(start + PATIENCE, "a", &WAITING));
        assert_eq!(cluster.tick(start + PATIENCE), Changes::default());

        // After that the region goes, although `a` still says it waits.
        assert!(cluster.heartbeat_with(start + PATIENCE + 1, "a", &WAITING));
        assert!(cluster.heartbeat_with(start + PATIENCE + 1, "b", &[]));
        assert_eq!(
            cluster.tick(start + PATIENCE + 1),
            changes(&["a", "b"], true)
        );
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 2, 1)]
        );
    }

    #[test]
    fn a_committed_vouch_ends_a_run_of_waiting_for_the_store() {
        let mut cluster = a_runs_and_b_waits();
        let start = LEASE + 2000;
        let mut now = start;
        let beat = |cluster: &mut Cluster, now: u64, vouches: &[(RegionId, Vouch)]| {
            assert!(cluster.heartbeat_with(now, "a", vouches));
            assert!(cluster.heartbeat_with(now, "b", &[]));
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        };
        while now < start + PATIENCE - 5000 {
            beat(&mut cluster, now, &WAITING);
            now += 5000;
        }
        // The store answers for a moment, and then the worker waits again.
        beat(&mut cluster, now, &COMMITTED);
        let again = now + 5000;
        now = again;
        while now <= again + PATIENCE {
            beat(&mut cluster, now, &WAITING);
            now += 5000;
        }
        assert!(cluster.heartbeat_with(again + PATIENCE + 1, "b", &[]));
        assert_eq!(
            cluster.tick(again + PATIENCE + 1),
            changes(&["a", "b"], true)
        );
    }

    #[test]
    fn registering_again_does_not_end_a_run_of_waiting_for_the_store() {
        let mut cluster = a_runs_and_b_waits();
        let held = cluster.assignments("a");
        let start = LEASE + 2000;
        assert!(cluster.heartbeat_with(start, "a", &WAITING));
        // The worker lost its connection while it waited, and is back.
        cluster.register(start + PATIENCE - 1, "a", "a:25601", &held);
        assert!(cluster.heartbeat_with(start + PATIENCE, "a", &WAITING));
        assert!(cluster.heartbeat_with(start + PATIENCE, "b", &[]));
        assert_eq!(cluster.tick(start + PATIENCE), Changes::default());
        assert_eq!(
            cluster.tick(start + PATIENCE + 1),
            changes(&["a", "b"], true)
        );
    }

    #[test]
    fn a_silent_worker_loses_its_regions_after_a_lease_however_it_vouched() {
        for vouches in [COMMITTED, WAITING] {
            let mut cluster = a_runs_and_b_waits();
            assert!(cluster.heartbeat_with(LEASE + 1000, "a", &vouches));
            assert!(cluster.heartbeat_with(2 * LEASE + 1000, "b", &[]));
            assert_eq!(cluster.tick(2 * LEASE + 1000), Changes::default());
            assert_eq!(cluster.tick(2 * LEASE + 1001), changes(&["a", "b"], true));
            // It is forgotten, not just left waiting.
            assert!(!cluster.heartbeat_with(2 * LEASE + 1001, "a", &vouches));
        }
    }

    #[test]
    fn a_vouch_for_a_region_of_another_worker_does_nothing() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        // `b` runs region 1 and names region 0 as well, as does a worker of a world
        // with another layout.
        let both = [
            (RegionId(0), Vouch::Committed),
            (RegionId(1), Vouch::Committed),
            (RegionId(9), Vouch::Committed),
        ];
        assert!(cluster.heartbeat_with(2 * LEASE, "a", &[]));
        assert!(cluster.heartbeat_with(2 * LEASE, "b", &both));
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
        // It goes to the worker that has not failed it, which keeps its own.
        assert_eq!(
            cluster.assignments("b"),
            [
                assignment(0, FIRST_EPOCH + 3, 2),
                assignment(1, FIRST_EPOCH + 2, 1)
            ]
        );
        assert!(cluster.assignments("a").is_empty());
    }

    #[test]
    fn a_worker_that_lost_a_region_for_not_vouching_waits_behind_the_others() {
        let mut cluster = Cluster::new(&[]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        // Each owner in turn stops vouching, and the region goes to the worker that
        // has waited longest without failing: `b` and `c` before `a` gets it again.
        let mut now = LEASE;
        for (lost, next) in [("a", "b"), ("b", "c"), ("c", "a")] {
            now += LEASE + 1;
            for name in ["a", "b", "c"] {
                assert!(cluster.heartbeat_with(now, name, &[]));
            }
            let mut told = [lost, next];
            told.sort_unstable();
            assert_eq!(cluster.tick(now), changes(&told, true));
            assert_eq!(cluster.assignments(next).len(), 1, "{next}");
        }
    }

    #[test]
    fn a_refused_epoch_raises_later_epochs_above_it_and_the_region_is_reassigned() {
        let mut cluster = a_runs_and_b_waits();
        let stale = cluster.assignments("a");
        let version = cluster.table().version;
        let seen = FIRST_EPOCH + 50;

        // The worker has dropped the region, which goes at once to the worker that has
        // waited longest, with an epoch above the one the store has seen. Being refused
        // says nothing against a worker, so that is the same one, as it registered
        // first. Edges see one new table.
        assert_eq!(
            cluster.epoch_refused(LEASE + 100, "a", 0, seen),
            changes(&["a"], true)
        );
        let again = assignment(0, seen + 1, 1);
        assert_eq!(cluster.assignments("a"), [again]);
        assert!(cluster.assignments("b").is_empty());
        assert_eq!(cluster.table().version, version + 1);
        assert_eq!(cluster.table().routes, [route(0, seen + 1, "a:25601")]);
        // What it held before is not honoured again: the store has seen a later owner.
        assert_eq!(
            cluster.register(LEASE + 100, "a", "a:25601", &stale),
            Changes::default()
        );
        assert_eq!(cluster.assignments("a"), [again]);

        // Again, with the epoch the store has seen in the meantime.
        assert_eq!(
            cluster.epoch_refused(LEASE + 300, "a", 0, seen + 9),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), [assignment(0, seen + 10, 2)]);
        // The new owner has its first lease to open the region.
        assert!(cluster.heartbeat_with(2 * LEASE + 300, "a", &[]));
        assert!(cluster.heartbeat_with(2 * LEASE + 300, "b", &[]));
        assert_eq!(cluster.tick(2 * LEASE + 300), Changes::default());
    }

    #[test]
    fn a_region_dropped_for_a_refused_epoch_waits_for_the_grace_period_to_end() {
        let mut cluster = Cluster::new(&[]);
        let held = assignment(0, 5, 0);
        cluster.register(0, "a", "a:25601", &[held]);
        assert_eq!(cluster.epoch_refused(1, "a", 0, 77), changes(&["a"], true));
        assert!(cluster.table().routes.is_empty());
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 1)]
        );
    }

    #[test]
    fn a_refusal_takes_nothing_from_an_owner_it_does_not_concern_but_raises_epochs() {
        let mut cluster = Cluster::new(&[0]);
        // The coordinator is new, so the region that this worker does not report
        // stays without an owner for a lease.
        let epoch = FIRST_EPOCH + 1;
        let held = [assignment(0, epoch, 0)];
        cluster.register(0, "a", "a:25601", &held);

        // A refusal under an earlier assignment of the same worker, of a region that
        // another worker runs or nobody does, from a worker that is not registered,
        // and of a region the layout does not have.
        let refusals = [
            ("a", 0, epoch),
            ("a", 0, epoch - 1),
            ("b", 0, epoch + 5),
            ("a", 1, epoch + 20),
            ("nobody", 1, epoch + 30),
            ("a", 7, epoch + 40),
        ];
        for (name, region, seen) in refusals {
            assert_eq!(
                cluster.epoch_refused(1, name, region, seen),
                Changes::default(),
                "{name} {region} {seen}"
            );
            assert_eq!(cluster.assignments("a"), held);
        }
        // Every one of them counts for what is issued from now on.
        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        assert_eq!(cluster.assignments("b")[0].epoch, epoch + 41);

        // A region nobody runs does not go back below what the store has seen.
        let mut cluster = Cluster::new(&[]);
        assert_eq!(cluster.epoch_refused(0, "a", 0, 77), Changes::default());
        assert_eq!(
            cluster.register(0, "a", "a:25601", &[assignment(0, 76, 0)]),
            Changes::default()
        );
    }

    #[test]
    fn a_late_heartbeat_counts_if_it_comes_before_the_tick() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

        // Both were silent for three leases, but nobody looked.
        assert!(cluster.heartbeat(3 * LEASE, "a"));
        assert!(cluster.heartbeat(3 * LEASE, "b"));
        assert_eq!(cluster.tick(3 * LEASE), Changes::default());
        assert_eq!(cluster.tick(4 * LEASE), Changes::default());
    }

    #[test]
    fn a_call_with_an_earlier_time_does_not_shorten_a_lease() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

        // The service may well make its calls a little out of order.
        assert!(cluster.heartbeat(2 * LEASE, "a"));
        assert!(cluster.heartbeat(LEASE, "a"));
        cluster.register(LEASE, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(3 * LEASE), Changes::default());
        assert_eq!(cluster.tick(0), Changes::default());
    }

    /// What a worker says of the players of a region it runs as `held`: a tick of the
    /// region, and the chunks with players in them, each with how many.
    fn players_of(held: Assignment, tick: u64, crowds: &[((i32, i32), u32)]) -> PlayersOf {
        let crowd = |((x, z), count): &((i32, i32), u32)| (ChunkPos::new(*x, *z), *count);
        PlayersOf {
            region: held.region,
            epoch: held.epoch,
            tick,
            crowds: crowds.iter().map(crowd).collect(),
        }
    }

    #[test]
    fn a_worker_that_says_where_its_players_are_is_known_and_one_that_never_registered_is_not() {
        let mut cluster = a_runs_and_b_waits();
        let held = cluster.assignments("a")[0];
        let report = [players_of(held, 40, &[((3, -2), 2), ((4, -2), 1)])];
        // Whether it names what it runs, nothing at all, or a region that is not its
        // own.
        assert!(cluster.players(LEASE + 250, "a", &report));
        assert!(cluster.players(LEASE + 250, "a", &[]));
        assert!(cluster.players(LEASE + 250, "b", &report));
        assert!(cluster.players(LEASE + 250, "b", &[]));
        // A worker the coordinator does not know has to register again, as after a
        // heartbeat.
        assert!(!cluster.players(LEASE + 250, "c", &report));
        assert!(!cluster.players(LEASE + 250, "c", &[]));
        // Nothing comes of any of it.
        assert_eq!(cluster.tick(LEASE + 250), Changes::default());
        assert_eq!(cluster.assignments("a"), [held]);
    }

    #[test]
    fn a_worker_that_only_says_where_its_players_are_stays_registered_and_vouches_for_nothing() {
        let mut cluster = a_runs_and_b_waits();
        let held = cluster.assignments("a")[0];
        // `a` says four times a second where the players of its region are, and sends
        // no heartbeat. `b` sends heartbeats.
        let mut now = LEASE;
        while now < 2 * LEASE {
            now += 250;
            let report = [players_of(held, now / 50, &[((0, 0), 1)])];
            assert!(cluster.players(now, "a", &report));
            assert!(cluster.heartbeat(now, "b"));
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        // The region was vouched for when it was assigned, at `LEASE`, and not since:
        // to say where its players are is not to vouch for it. A lease after that was
        // not too long; a moment more is.
        let report = [players_of(held, now / 50 + 1, &[((0, 0), 1)])];
        assert!(cluster.players(2 * LEASE + 1, "a", &report));
        assert!(cluster.heartbeat(2 * LEASE + 1, "b"));
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 2, 1)]
        );

        // The worker is heard from all the same, and stays registered for as long as
        // it says so once per lease, which a worker that says nothing does not.
        let mut now = 2 * LEASE + 1;
        for _ in 0..3 {
            now += LEASE;
            assert!(cluster.players(now, "a", &[]));
            assert!(cluster.heartbeat(now, "b"));
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        // Then it says nothing more. A lease of that is not too long; a moment more
        // is, and it has to register again.
        assert!(cluster.heartbeat(now + LEASE, "b"));
        assert_eq!(cluster.tick(now + LEASE), Changes::default());
        assert!(cluster.coordinator.workers.contains_key("a"));
        assert!(cluster.heartbeat(now + LEASE + 1, "b"));
        assert_eq!(cluster.tick(now + LEASE + 1), Changes::default());
        assert!(!cluster.players(now + LEASE + 1, "a", &[]));
        assert!(!cluster.heartbeat(now + LEASE + 1, "a"));
    }

    #[test]
    fn word_of_players_with_an_earlier_time_does_not_shorten_a_lease() {
        let mut cluster = a_runs_and_b_waits();
        // `b` was silent for two leases, but nobody looked. And the service may well
        // make its calls a little out of order.
        assert!(cluster.players(3 * LEASE, "b", &[]));
        assert!(cluster.players(LEASE, "b", &[]));
        for at in [3 * LEASE, 4 * LEASE] {
            assert!(cluster.heartbeat(at, "a"));
            assert_eq!(cluster.tick(at), Changes::default(), "at {at}");
        }
        assert!(cluster.players(4 * LEASE, "b", &[]));
    }

    #[test]
    fn a_waiting_worker_that_is_silent_is_forgotten_without_anybody_being_told() {
        let mut cluster = Cluster::new(&[]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

        assert!(cluster.heartbeat(LEASE, "a"));
        assert!(cluster.heartbeat(LEASE, "c"));
        assert_eq!(cluster.tick(2 * LEASE), Changes::default());
        assert!(!cluster.heartbeat(2 * LEASE, "b"));

        // It registers again, and is now behind the worker that kept waiting.
        cluster.register(2 * LEASE, "b", "b:25601", &[]);
        assert!(cluster.heartbeat(2 * LEASE, "c"));
        assert_eq!(cluster.tick(3 * LEASE), changes(&["a", "c"], true));
        assert!(cluster.assignments("b").is_empty());
    }

    #[test]
    fn nothing_new_is_assigned_during_the_grace_period_but_holdings_are_honoured() {
        let mut cluster = Cluster::new(&[0]);
        assert_eq!(
            cluster.register(0, "idle", "idle:25601", &[]),
            Changes::default()
        );
        let held = assignment(1, 700, 9);
        assert_eq!(
            cluster.register(1, "busy", "busy:25601", &[held]),
            changes(&["busy"], true)
        );
        assert_eq!(cluster.assignments("busy"), [held]);
        assert_eq!(cluster.table().routes, [route(1, 700, "busy:25601")]);
        assert_eq!(cluster.table().version, FIRST_EPOCH + 1);

        for now in [1, LEASE / 2, LEASE - 1] {
            assert_eq!(cluster.tick(now), Changes::default());
            assert!(cluster.assignments("idle").is_empty());
        }
        assert_eq!(cluster.tick(LEASE), changes(&["idle"], true));
        assert_eq!(
            cluster.assignments("idle"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert_eq!(cluster.assignments("busy"), [held]);
    }

    #[test]
    fn workers_keep_what_they_held_under_a_new_coordinator_whatever_the_order() {
        // What the coordinator that is gone had given them.
        let held = [("a", assignment(0, 41, 3)), ("b", assignment(1, 57, 0))];
        for order in [[0, 1], [1, 0]] {
            // The first epoch of the new one is no help: it is below what they hold.
            let mut cluster = Cluster::with_first_epoch(&[0], 5);
            for index in order {
                let (name, holding) = held[index];
                let address = format!("{name}:25601");
                assert_eq!(
                    cluster.register(1, name, &address, &[holding]),
                    changes(&[name], true)
                );
            }
            cluster.register(2, "c", "c:25601", &[]);
            assert_eq!(cluster.assignments("a"), [held[0].1]);
            assert_eq!(cluster.assignments("b"), [held[1].1]);
            let table = cluster.table();
            assert!(table.is_complete());
            assert_eq!(table.version, 7);
            assert_eq!(
                table.routes,
                [route(0, 41, "a:25601"), route(1, 57, "b:25601")]
            );

            // The grace period ends, and nothing is taken from them.
            for name in ["a", "b", "c"] {
                assert!(cluster.heartbeat(LEASE, name));
            }
            assert_eq!(cluster.tick(LEASE), Changes::default());

            // What is assigned from now on is above every epoch that was reported and
            // has none of the entity ids that were: blocks 0 and 3 are left out.
            let mut now = LEASE;
            let fresh = [(58, 1), (59, 2), (60, 4)];
            for (round, (epoch, block)) in fresh.into_iter().enumerate() {
                let (lost, next) = [("b", "c"), ("c", "d"), ("d", "e")][round];
                cluster.register(now, next, &format!("{next}:25601"), &[]);
                now += LEASE + 1;
                assert!(cluster.heartbeat(now, "a"));
                assert!(cluster.heartbeat(now, next));
                let mut told = [lost, next];
                told.sort_unstable();
                assert_eq!(cluster.tick(now), changes(&told, true));
                assert_eq!(cluster.assignments(next), [assignment(1, epoch, block)]);
            }
            assert_eq!(cluster.assignments("a"), [held[0].1]);
        }
    }

    #[test]
    fn a_new_coordinator_carries_on_above_the_first_epoch_it_is_given() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let old = cluster.table();

        // The clock has moved on by the time another coordinator starts.
        cluster.restart(2 * LEASE, FIRST_EPOCH + 500);
        let table = cluster.table();
        assert!(table.version > old.version);
        assert!(table.routes.is_empty());
        assert!(!cluster.heartbeat(2 * LEASE, "a"));

        // The grace period starts again. Nothing is heard of the worker that ran the
        // region, so in the end another one gets it.
        cluster.register(2 * LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(3 * LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(3 * LEASE), changes(&["b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 501, 0)]
        );
        assert_eq!(cluster.table().version, FIRST_EPOCH + 501);
    }

    #[test]
    fn a_holding_for_a_region_someone_else_owns_is_not_honoured() {
        let mut cluster = Cluster::new(&[0]);
        let held = assignment(0, 7, 0);
        assert_eq!(
            cluster.register(0, "a", "a:25601", &[held]),
            changes(&["a"], true)
        );
        // Neither with a lower epoch than the owner's nor with a higher one.
        let claims = [
            ("b", assignment(0, 5, 1)),
            ("c", assignment(0, FIRST_EPOCH + 50, 2)),
        ];
        for (name, claim) in claims {
            assert_eq!(
                cluster.register(1, name, &format!("{name}:25601"), &[claim]),
                Changes::default()
            );
            assert!(cluster.assignments(name).is_empty());
        }
        assert_eq!(cluster.assignments("a"), [held]);
        assert_eq!(cluster.table().routes, [route(0, 7, "a:25601")]);

        // The two wait like any other worker. What they reported is in use somewhere
        // all the same, so what is issued stays clear of it.
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(1, FIRST_EPOCH + 51, 3)]
        );
        assert!(cluster.assignments("c").is_empty());
    }

    #[test]
    fn a_holding_does_not_take_a_region_back_to_an_epoch_it_has_left_behind() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let stale = cluster.assignments("a");

        // The worker is cut off and goes on running the region. Another takes over
        // and is lost in turn, with nobody waiting.
        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
        assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["b"], true));
        assert!(cluster.table().routes.is_empty());

        // Back again, the first worker cannot carry on where it was: storage has seen
        // its successor. It gets the region like a new owner.
        assert_eq!(
            cluster.register(3 * LEASE + 1, "a", "a:25601", &stale),
            Changes::default()
        );
        assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
    }

    #[test]
    fn a_forgotten_worker_carries_on_with_what_nobody_was_given_since() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a"], true));
        assert!(cluster.table().routes.is_empty());

        assert_eq!(
            cluster.register(2 * LEASE + 1, "a", "a:25601", &held),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), held);
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
    }

    #[test]
    fn a_holding_is_honoured_whatever_entity_ids_it_has() {
        // The world store issues entity ids now: the coordinator only passes on what it
        // is told, and decides nothing by it.
        let mut cluster = Cluster::new(&[0]);
        let held = assignment(0, 3, 4);
        cluster.register(0, "a", "a:25601", &[held]);
        let few = EntityIds {
            first: EntityId(ids(4).first.0 + 5),
            end: EntityId(ids(4).first.0 + 10),
        };
        let claim = Assignment {
            region: RegionId(1),
            epoch: 4,
            entity_ids: few,
        };
        assert_eq!(
            cluster.register(1, "b", "b:25601", &[claim]),
            changes(&["b"], true)
        );
        assert_eq!(cluster.assignments("a"), [held]);
        assert_eq!(cluster.assignments("b"), [claim]);
    }

    /// Until regions split, a holding for a region the layout does not have was turned
    /// away. Such a region is one that was split off another now, and nobody but its
    /// worker may know of it.
    #[test]
    fn a_holding_for_a_region_the_coordinator_does_not_know_is_honoured_on_the_workers_word() {
        let mut cluster = Cluster::new(&[]);
        let claim = assignment(1, FIRST_EPOCH + 20, 0);
        assert_eq!(
            cluster.register(0, "a", "a:25601", &[claim]),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), [claim]);
        // The region of the layout still waits for an owner.
        assert!(!cluster.table().is_complete());

        // Its epoch and its entity ids are left out from now on.
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 21, 1), claim]
        );
        assert!(cluster.table().is_complete());

        // A reading of the list that is older than the region leaves it alone. It
        // names the home region, which is new to the table.
        let older = stripes(1);
        assert_eq!(cluster.listed(LEASE + 1, &older), changes(&[], true));
        assert_eq!(cluster.assignments("a").len(), 2);
        // One that has no such region, and would have if there were one, takes it
        // away again.
        let list = RegionList {
            next: RegionId(2),
            ..older
        };
        assert_eq!(cluster.listed(LEASE + 2, &list), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 21, 1)]
        );
        // The worker may claim it again all the same: the next reading says.
        let again = [assignment(0, FIRST_EPOCH + 21, 1), claim];
        assert_eq!(
            cluster.register(LEASE + 3, "a", "a:25601", &again),
            changes(&["a"], true)
        );
        // Not once the list has it as absorbed: then the holding is turned away.
        let list = RegionList {
            absorbed: vec![(RegionId(1), RegionId(0))],
            ..list
        };
        assert_eq!(cluster.listed(LEASE + 4, &list), changes(&["a"], true));
        assert_eq!(
            cluster.register(LEASE + 5, "a", "a:25601", &again),
            Changes::default()
        );
        assert_eq!(cluster.assignments("a"), again[..1]);
    }

    #[test]
    fn a_worker_keeps_every_region_it_reports() {
        let mut cluster = Cluster::new(&[0]);
        // The same region several times: the highest epoch is the one that counts.
        let holding = [
            assignment(1, 30, 2),
            assignment(0, 12, 5),
            assignment(0, 14, 6),
            assignment(0, 13, 7),
        ];
        assert_eq!(
            cluster.register(0, "a", "a:25601", &holding),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), [holding[2], holding[0]]);
        let table = cluster.table();
        assert!(table.is_complete());
        // One call, one new version.
        assert_eq!(table.version, FIRST_EPOCH + 1);

        // Nothing is left to give to a worker that runs nothing. Once the coordinator
        // is no longer new, it asks for one of the two for that worker.
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(LEASE), asks(&[order("a", 1, 30)]));
        assert_eq!(cluster.assignments("a"), [holding[2], holding[0]]);
    }

    #[test]
    fn what_a_worker_reports_of_its_own_region_replaces_what_was_known() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[assignment(0, 12, 5)]);
        let version = cluster.table().version;

        // Other entity ids are news to the worker alone.
        let other_ids = assignment(0, 12, 6);
        assert_eq!(
            cluster.register(1, "a", "a:25601", &[other_ids]),
            changes(&["a"], false)
        );
        assert_eq!(cluster.assignments("a"), [other_ids]);
        assert_eq!(cluster.table().version, version);

        // A higher epoch is news to the edges as well.
        let later = assignment(0, 15, 6);
        assert_eq!(
            cluster.register(2, "a", "a:25601", &[later]),
            changes(&["a"], true)
        );
        assert_eq!(cluster.assignments("a"), [later]);
        assert_eq!(cluster.table().version, version + 1);

        // A lower one is behind the worker itself, and stays there.
        assert_eq!(
            cluster.register(3, "a", "a:25601", &[other_ids]),
            Changes::default()
        );
        assert_eq!(cluster.assignments("a"), [later]);
    }

    #[test]
    fn registering_again_replaces_the_address_and_moves_the_table_on() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "old:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        let version = cluster.table().version;

        assert_eq!(
            cluster.register(LEASE + 1, "a", "new:25601", &held),
            changes(&[], true)
        );
        assert_eq!(cluster.assignments("a"), held);
        let table = cluster.table();
        assert_eq!(table.routes, [route(0, FIRST_EPOCH + 1, "new:25601")]);
        assert_eq!(table.version, version + 1);

        // From the same address nothing changes, and what the worker owns stays even
        // if it does not say that it holds it.
        assert_eq!(
            cluster.register(LEASE + 2, "a", "new:25601", &[]),
            Changes::default()
        );
        assert_eq!(cluster.assignments("a"), held);
        assert_eq!(cluster.table(), table);

        // To register is to be heard from, but only a region it reports is vouched for.
        // So the region goes a lease after it was last reported, and as nobody else is
        // waiting, it comes back to the same worker with a new epoch.
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        assert_eq!(cluster.tick(2 * LEASE + 2), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 2, 1)]
        );
        // The worker was last heard from when it registered.
        assert_eq!(cluster.tick(2 * LEASE + 3), changes(&["a"], true));
        assert!(cluster.assignments("a").is_empty());
    }

    #[test]
    fn a_waiting_worker_keeps_its_place_when_it_registers_again() {
        let mut cluster = Cluster::new(&[]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let version = cluster.table().version;

        // No edge is sent to a waiting worker, so its address is no news to them.
        assert_eq!(
            cluster.register(LEASE, "b", "elsewhere:25601", &[]),
            Changes::default()
        );
        assert_eq!(cluster.table().version, version);
        assert!(cluster.heartbeat(LEASE, "c"));

        assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
        assert_eq!(
            cluster.table().routes,
            [route(0, FIRST_EPOCH + 2, "elsewhere:25601")]
        );
    }

    #[test]
    fn a_worker_with_another_layout_is_refused() {
        let mut cluster = Cluster::new(&[0]);
        let ours = cluster.layout.fingerprint();
        let theirs = Layout::single().fingerprint();
        let refusal = Refusal::Layout {
            reported: theirs,
            expected: ours,
        };
        let held = assignment(0, 5, 0);
        assert_eq!(
            cluster.register_as(0, "a", "a:25601", &[held], Some(theirs)),
            Err(refusal)
        );
        assert!(!cluster.heartbeat(0, "a"));
        assert_eq!(cluster.table().version, FIRST_EPOCH);
        // The worker is told both fingerprints.
        let reason = refusal.to_string();
        assert!(reason.contains(&format!("{theirs:#018x}")), "{reason}");
        assert!(reason.contains(&format!("{ours:#018x}")), "{reason}");

        // The coordinator's layout is fine, and so is none: a worker that runs nothing
        // has no reason to have one.
        assert_eq!(
            cluster.register_as(0, "a", "a:25601", &[held], Some(ours)),
            Ok(changes(&["a"], true))
        );
        assert_eq!(
            cluster.register_as(0, "b", "b:25601", &[], None),
            Ok(Changes::default())
        );

        // A refusal leaves a registered worker as it was. It is not even a sign of
        // life.
        assert_eq!(
            cluster.register_as(1, "a", "elsewhere:25601", &[], Some(theirs)),
            Err(refusal)
        );
        assert_eq!(cluster.table().routes, [route(0, 5, "a:25601")]);
        assert_eq!(cluster.tick(LEASE + 1), changes(&["a"], true));
    }

    #[test]
    fn a_region_is_assigned_all_the_same_when_the_entity_ids_run_out() {
        let mut cluster = Cluster::new(&[0]);
        let mut now = LEASE;
        // Region 1 keeps the last block; region 0 goes from worker to worker, and each
        // uses up another. The worker that keeps has no connection, or it would be
        // given the other region whenever that loses its owner.
        let kept = assignment(1, 5, EntityIds::BLOCK_COUNT - 1);
        cluster.register(0, "keeps", "keeps:25601", &[kept]);
        assert_eq!(cluster.disconnected(0, "keeps"), Changes::default());
        for block in 0..EntityIds::BLOCK_COUNT - 1 {
            cluster.register(now, "a", "a:25601", &[]);
            assert!(cluster.heartbeat(now, "keeps"));
            assert_eq!(cluster.tick(now), changes(&["a"], true));
            assert_eq!(cluster.assignments("a")[0].entity_ids, ids(block));
            now += LEASE + 1;
            assert!(cluster.heartbeat(now, "keeps"));
            assert_eq!(cluster.tick(now), changes(&["a"], true));
        }
        // Then a block no owner has is filled in, and when there is none, no ids at all.
        cluster.register(now, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(now), changes(&["a"], true));
        assert_eq!(cluster.assignments("a")[0].entity_ids, ids(0));
        assert_eq!(cluster.assignments("keeps"), [kept]);
    }

    #[test]
    fn no_entity_ids_are_filled_in_when_every_block_has_an_owner() {
        let mut coordinator = coordinator(&Layout::single(), Instant::now(), FIRST_EPOCH);
        coordinator.used_blocks = (0..EntityIds::BLOCK_COUNT).collect();
        let everything = EntityIds {
            first: EntityId(1),
            end: EntityId(i32::MAX),
        };
        let region = coordinator.regions.get_mut(&RegionId(0)).unwrap();
        region.owner = Some(Owner::new("a", everything, Instant::now()));
        let none = coordinator.fill_entity_ids();
        assert_eq!(none.first, none.end);
    }

    #[test]
    fn a_region_stays_without_an_owner_when_the_epochs_run_out() {
        let mut cluster = Cluster::new(&[0]);
        // No epoch is above this one.
        let held = assignment(0, u64::MAX, 0);
        assert_eq!(
            cluster.register(0, "a", "a:25601", &[held]),
            changes(&["a"], true)
        );
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), Changes::default());
        assert!(cluster.assignments("b").is_empty());
        assert_eq!(cluster.assignments("a"), [held]);
    }

    #[test]
    fn reported_entity_ids_are_never_issued_whatever_blocks_they_are_in() {
        let size = EntityIds::BLOCK_SIZE;
        let indices = |first, end| {
            block_indices(EntityIds {
                first: EntityId(first),
                end: EntityId(end),
            })
        };
        for block in [0, 1, 7, EntityIds::BLOCK_COUNT - 1] {
            assert_eq!(block_indices(ids(block)), block..block + 1);
        }
        assert_eq!(indices(size - 1, size + 1), 0..2);
        assert_eq!(indices(3 * size, 5 * size), 3..5);
        assert_eq!(indices(-5, 1), 0..1);
        assert_eq!(indices(i32::MIN, i32::MAX), 0..EntityIds::BLOCK_COUNT + 1);
        // Without an id there is nothing to keep clear of.
        for (first, end) in [(5, 5), (9, 2), (-9, -2), (i32::MAX, i32::MIN)] {
            assert!(indices(first, end).is_empty());
        }

        // A worker holds a region with ids that are not a block as the coordinator
        // would make it. The next owner gets the first block that is clear of them.
        let mut cluster = Cluster::new(&[]);
        let held = Assignment {
            region: RegionId(0),
            epoch: 3,
            entity_ids: EntityIds {
                first: EntityId(size / 2),
                end: EntityId(2 * size + 1),
            },
        };
        cluster.register(0, "a", "a:25601", &[held]);
        assert_eq!(cluster.assignments("a"), [held]);
        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(2 * LEASE), changes(&["a", "b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 1, 3)]
        );
    }

    /// The epoch `a` runs the region with in [`a_runs_and_b_waits`], and the one the
    /// next owner gets.
    const FIRST_OWNER: u64 = FIRST_EPOCH + 1;
    const NEXT_OWNER: u64 = FIRST_EPOCH + 2;

    #[test]
    fn a_move_asks_the_owner_to_release_and_the_region_goes_to_the_target_when_it_has() {
        // The owner has not vouched for the region once: it may still be restoring it.
        // That makes no difference to the coordinator.
        let mut cluster = a_runs_and_b_waits();
        let version = cluster.table().version;
        let (moving, asked) = cluster.move_region(LEASE + 100, 0, None, 7).unwrap();
        assert_eq!(moving, begun("a", "b"));
        assert_eq!(asked, asks(&[order("a", 0, FIRST_OWNER)]));
        assert_eq!(cluster.table().version, version);

        // No lease is waited for: the region changes hands when its owner says so.
        let expected = Changes {
            moves: vec![outcome(7, 0, Some(("b", NEXT_OWNER)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 150, "a", 0, FIRST_OWNER), expected);
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
        assert_eq!(cluster.table().version, version + 1);
        assert_eq!(cluster.table().routes, [route(0, NEXT_OWNER, "b:25601")]);
        assert!(cluster.coordinator.releases.is_empty());

        // The new owner has a lease to open and restore it, and the old one waits.
        for name in ["a", "b"] {
            assert!(cluster.heartbeat_with(2 * LEASE + 150, name, &[]));
        }
        assert_eq!(cluster.tick(2 * LEASE + 150), Changes::default());
        // Saying it again changes nothing.
        assert_eq!(
            cluster.released(2 * LEASE + 150, "a", 0, FIRST_OWNER),
            Changes::default()
        );
    }

    #[test]
    fn a_move_to_a_named_worker_goes_to_that_one_and_not_to_the_one_that_waited_longest() {
        let mut cluster = a_runs_and_b_waits();
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 1, 0, Some("c"), 1).unwrap();
        assert_eq!(moving, begun("a", "c"));
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("c", NEXT_OWNER)), true)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(cluster.released(LEASE + 2, "a", 0, FIRST_OWNER), expected);
        assert!(cluster.assignments("b").is_empty());
    }

    #[test]
    fn a_move_is_refused_with_the_reason_and_changes_nothing() {
        let mut cluster = Cluster::new(&[0]);
        // The coordinator is new: the region that nobody reports has no owner yet.
        cluster.register(0, "a", "a:25601", &[assignment(0, FIRST_OWNER, 0)]);
        assert_eq!(
            cluster.move_region(1, 1, None, 1),
            Err(MoveRefusal::NoOwner(RegionId(1)))
        );
        // Nobody else is there.
        assert_eq!(
            cluster.move_region(1, 0, None, 1),
            Err(MoveRefusal::NoTarget(RegionId(0)))
        );
        assert_eq!(
            cluster.move_region(1, 5, None, 1),
            Err(MoveRefusal::NoSuchRegion(RegionId(5)))
        );

        cluster.register(LEASE, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        let unfit = |cluster: &mut Cluster, region: u32, worker: &str, why: &'static str| {
            let refusal = MoveRefusal::NotATarget {
                region: RegionId(region),
                worker: worker.to_owned(),
                why,
            };
            assert_eq!(
                cluster.move_region(LEASE + 1, region, Some(worker), 1),
                Err(refusal)
            );
        };
        unfit(&mut cluster, 0, "nobody", "is not registered");
        unfit(&mut cluster, 0, "a", "owns the region");

        // One of the others has lost its connection, and the other one leaves, for
        // which its region is to be released to the only worker left.
        assert_eq!(cluster.disconnected(LEASE + 1, "c"), Changes::default());
        unfit(&mut cluster, 0, "c", "has no connection to the coordinator");
        let second = cluster.assignments("b")[0].epoch;
        assert_eq!(
            cluster.leaving(LEASE + 1, "b"),
            asks(&[order("b", 1, second)])
        );
        unfit(&mut cluster, 0, "b", "is leaving");
        assert_eq!(
            cluster.move_region(LEASE + 1, 0, None, 1),
            Err(MoveRefusal::NoTarget(RegionId(0)))
        );
        let refusal = MoveRefusal::BeingReleased {
            region: RegionId(1),
            from: "b".to_owned(),
            to: "a".to_owned(),
        };
        assert_eq!(
            cluster.move_region(LEASE + 2, 1, None, 1),
            Err(refusal.clone())
        );
        assert_eq!(
            cluster.move_region(LEASE + 2, 1, Some("c"), 1),
            Err(refusal.clone())
        );

        // Whoever asked is told in words.
        assert_eq!(
            refusal.to_string(),
            "region 1 is being released already, by b for a"
        );
        let refusal = MoveRefusal::NotATarget {
            region: RegionId(0),
            worker: "c".to_owned(),
            why: "is leaving",
        };
        assert_eq!(
            refusal.to_string(),
            "region 0 cannot be moved to c, which is leaving"
        );
        assert_eq!(
            MoveRefusal::NoTarget(RegionId(0)).to_string(),
            "no other worker is there that region 0 could be moved to"
        );
    }

    #[test]
    fn a_region_is_moved_to_a_worker_that_runs_one_already_by_name_and_by_choice() {
        // By name, although another worker runs nothing.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, asked) = cluster.move_region(LEASE + 1, 0, Some("b"), 1).unwrap();
        assert_eq!(moving, begun("a", "b"));
        assert_eq!(asked, asks(&[order("a", 0, FIRST_EPOCH + 1)]));
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("b", FIRST_EPOCH + 3)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1),
            expected
        );
        assert_eq!(
            cluster.assignments("b"),
            [
                assignment(0, FIRST_EPOCH + 3, 2),
                assignment(1, FIRST_EPOCH + 2, 1)
            ]
        );
        assert!(cluster.assignments("a").is_empty());
        assert!(cluster.assignments("c").is_empty());

        // By choice, when every other worker runs one: the one that has waited
        // longest of those with the fewest.
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 3]);
        let (moving, _) = cluster.move_region(LEASE + 1, 0, None, 2).unwrap();
        assert_eq!(moving, begun("a", "b"));
        let expected = Changes {
            moves: vec![outcome(2, 0, Some(("b", FIRST_EPOCH + 5)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 2, "a", 0, FIRST_EPOCH + 1),
            expected
        );
        assert_eq!(regions_of(&cluster, "b"), [0, 1]);
        // The next by choice goes to the worker with the fewest then, whichever
        // registered first.
        let (moving, _) = cluster.move_region(LEASE + 3, 1, None, 3).unwrap();
        assert_eq!(moving, begun("b", "c"));
        let (moving, _) = cluster.move_region(LEASE + 3, 3, None, 4).unwrap();
        assert_eq!(moving, begun("a", "b"));
    }

    #[test]
    fn a_reserved_target_counts_as_having_the_region_when_the_next_target_is_chosen() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        cluster.register(LEASE, "d", "d:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 1).unwrap();
        assert_eq!(moving, begun("a", "c"));
        // A second move does not pick the same worker, which has waited longest of
        // the two that own nothing: it is being given a region just now.
        let (moving, _) = cluster.move_region(LEASE + 100, 1, None, 2).unwrap();
        assert_eq!(moving, begun("b", "d"));
        // Each gets the region it was reserved for.
        let expected = Changes {
            moves: vec![outcome(2, 1, Some(("d", FIRST_EPOCH + 3)), true)],
            ..changes(&["b", "d"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 200, "b", 1, FIRST_EPOCH + 2),
            expected
        );
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), true)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 200, "a", 0, FIRST_EPOCH + 1),
            expected
        );

        // The same when a region without an owner is given away while a release
        // lasts.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 1).unwrap();
        assert_eq!(moving, begun("a", "c"));
        // The other owner dies. The worker that owns nothing counts as having one
        // region, like the worker that is asked to release, which registered before
        // it and is given this one.
        assert!(cluster.heartbeat(2 * LEASE, "a"));
        assert!(cluster.heartbeat(2 * LEASE, "c"));
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert!(cluster.assignments("c").is_empty());
        // The release goes on, and ends as it was meant to.
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), true)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(
            cluster.released(2 * LEASE + 2, "a", 0, FIRST_EPOCH + 1),
            expected
        );
        assert_eq!(regions_of(&cluster, "a"), [1]);
        assert_eq!(regions_of(&cluster, "c"), [0]);
    }

    #[test]
    fn a_reserved_target_can_be_named_for_another_region_and_be_given_one_meanwhile() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        for (region, from, mover) in [(0, "a", 1), (1, "b", 2)] {
            let (moving, _) = cluster
                .move_region(LEASE + 100, region, Some("c"), mover)
                .unwrap();
            assert_eq!(moving, begun(from, "c"));
        }
        // One owner lets go, and the other does not answer: both regions end up
        // with the worker they were meant for.
        let expected = Changes {
            moves: vec![outcome(2, 1, Some(("c", FIRST_EPOCH + 3)), true)],
            ..changes(&["b", "c"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 200, "b", 1, FIRST_EPOCH + 2),
            expected
        );
        assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "c");
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE + 100, name));
        }
        // That leaves it with two regions and the others with none, which the same
        // tick begins to even out, whoever asked for it to be so: one of them is to
        // go to the worker that has waited longer since it lost its own.
        let expected = Changes {
            releases: vec![order("c", 1, FIRST_EPOCH + 3)],
            moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), false)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 101), expected);
        assert_eq!(regions_of(&cluster, "c"), [0, 1]);
        assert_eq!(cluster.coordinator.releases[&RegionId(1)].to, "b");
    }

    #[test]
    fn a_release_is_dropped_when_its_owner_dies_and_takes_nothing_from_the_next_one() {
        let mut cluster = a_runs_and_b_waits();
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 9).unwrap();
        assert_eq!(moving, begun("a", "b"));

        // The owner's lease runs out before the release does. The region is assigned
        // like any that lost its owner, and whoever asked is told that the owner did
        // not let go of it.
        assert!(cluster.heartbeat(2 * LEASE, "b"));
        assert!(cluster.heartbeat(2 * LEASE, "c"));
        let expected = Changes {
            moves: vec![outcome(9, 0, Some(("b", NEXT_OWNER)), false)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 1), expected);
        assert!(cluster.coordinator.releases.is_empty());

        // When the release would have run out, the region stays with its new owner,
        // and what the old one says of it now changes nothing.
        for name in ["b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE + 101, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 101), Changes::default());
        assert_eq!(
            cluster.released(2 * LEASE + 101, "a", 0, FIRST_OWNER),
            Changes::default()
        );
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    }

    #[test]
    fn a_release_is_dropped_when_the_store_refuses_the_owners_epoch_and_its_target_is_free() {
        let mut cluster = a_runs_and_b_waits();
        let (moving, _) = cluster.move_region(LEASE + 100, 0, None, 9).unwrap();
        assert_eq!(moving, begun("a", "b"));

        // The region is given to the worker that registered first, as after any
        // refusal, which is the same one. It is another tenure, which nobody asked to
        // have released.
        let seen = FIRST_EPOCH + 50;
        let expected = Changes {
            moves: vec![outcome(9, 0, Some(("a", seen + 1)), false)],
            ..changes(&["a"], true)
        };
        assert_eq!(cluster.epoch_refused(LEASE + 200, "a", 0, seen), expected);
        assert!(cluster.coordinator.releases.is_empty());
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(2 * LEASE + 101, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 101), Changes::default());
        assert_eq!(cluster.assignments("a"), [assignment(0, seen + 1, 1)]);

        // The worker that was reserved can be picked again.
        let (moving, _) = cluster.move_region(2 * LEASE + 101, 0, None, 10).unwrap();
        assert_eq!(moving, begun("a", "b"));
    }

    #[test]
    fn a_region_that_is_being_released_is_not_taken_for_want_of_vouching() {
        let mut cluster = a_runs_and_b_waits();
        assert!(cluster.heartbeat_with(LEASE + 3000, "a", &COMMITTED));
        // The owner is asked shortly before its last vouch is a lease old, and stops
        // ticking, as it should. It is heard from all the while.
        let asked = 2 * LEASE + 2000;
        cluster.move_region(asked, 0, None, 3).unwrap();
        for now in [asked, asked + LEASE] {
            for name in ["a", "b"] {
                assert!(cluster.heartbeat_with(now, name, &[]));
            }
        }
        // Without the release this tick would have taken the region.
        assert_eq!(cluster.tick(2 * LEASE + 3001), Changes::default());
        assert_eq!(cluster.tick(asked + LEASE), Changes::default());
        assert_eq!(cluster.assignments("a"), [assignment(0, FIRST_OWNER, 0)]);
    }

    #[test]
    fn a_release_that_is_not_answered_within_a_lease_of_being_asked_ends_like_a_death() {
        let mut cluster = a_runs_and_b_waits();
        let held = cluster.assignments("a");
        let asked = LEASE + 500;
        cluster.move_region(asked, 0, None, 4).unwrap();

        // The owner loses its connection and is asked again when it is back. That
        // does not give it more time.
        assert_eq!(cluster.disconnected(asked + 100, "a"), Changes::default());
        assert_eq!(
            cluster.register(2 * LEASE, "a", "a:25601", &held),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
        assert!(cluster.heartbeat(2 * LEASE, "a"));
        assert!(cluster.heartbeat(2 * LEASE, "b"));

        // A lease is not too long; a moment more is. The region goes to the worker
        // that was reserved for it, and whoever asked is told that it was not let go.
        assert_eq!(cluster.tick(asked + LEASE), Changes::default());
        let expected = Changes {
            moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), false)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.tick(asked + LEASE + 1), expected);
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
        // The worker that did not answer is still registered, and waits.
        assert!(cluster.heartbeat(asked + LEASE + 1, "a"));
        assert!(cluster.assignments("a").is_empty());
    }

    #[test]
    fn an_owner_that_registers_again_is_asked_again_and_without_the_region_it_has_released() {
        let mut cluster = a_runs_and_b_waits();
        let held = cluster.assignments("a");
        cluster.move_region(LEASE + 100, 0, None, 4).unwrap();

        // Whether `Release` reached it nobody knows, so it is told again, as often as
        // it registers with the region.
        for now in [LEASE + 200, LEASE + 300] {
            assert_eq!(
                cluster.register(now, "a", "a:25601", &held),
                asks(&[order("a", 0, FIRST_OWNER)])
            );
        }
        assert_eq!(cluster.assignments("a"), held);

        // It released the region, and `Released` was lost with the connection. What it
        // holds when it is back says the same.
        let expected = Changes {
            moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.register(LEASE + 400, "a", "a:25601", &[]), expected);
        assert!(cluster.assignments("a").is_empty());
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);

        // Without a release, a worker that registers without its region keeps it, as
        // ever: only its vouching says whether it runs it.
        assert_eq!(
            cluster.register(LEASE + 500, "b", "b:25601", &[]),
            Changes::default()
        );
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    }

    #[test]
    fn a_holding_with_another_epoch_is_not_the_region_the_owner_was_asked_to_release() {
        let mut cluster = a_runs_and_b_waits();
        cluster.move_region(LEASE + 100, 0, None, 4).unwrap();
        // The worker runs something older, which is not honoured; it does not hold
        // what it was asked to release.
        let stale = assignment(0, FIRST_OWNER - 1, 0);
        let expected = Changes {
            moves: vec![outcome(4, 0, Some(("b", NEXT_OWNER)), true)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(
            cluster.register(LEASE + 200, "a", "a:25601", &[stale]),
            expected
        );

        // A worker that reports its region with a later epoch than the coordinator
        // knows has another tenure, which the release was not of.
        let mut cluster = a_runs_and_b_waits();
        cluster.move_region(LEASE + 100, 0, None, 5).unwrap();
        let later = assignment(0, FIRST_OWNER + 20, 0);
        let expected = Changes {
            moves: vec![outcome(5, 0, Some(("a", FIRST_OWNER + 20)), false)],
            ..changes(&["a"], true)
        };
        assert_eq!(
            cluster.register(LEASE + 200, "a", "a:25601", &[later]),
            expected
        );
        assert!(cluster.coordinator.releases.is_empty());
    }

    #[test]
    fn released_from_anyone_but_the_owner_with_its_epoch_changes_nothing() {
        let mut cluster = a_runs_and_b_waits();
        cluster.move_region(LEASE + 100, 0, None, 4).unwrap();
        let said = [
            ("b", 0, FIRST_OWNER),
            ("a", 0, FIRST_OWNER - 1),
            ("a", 0, FIRST_OWNER + 1),
            ("a", 1, FIRST_OWNER),
            ("nobody", 0, FIRST_OWNER),
        ];
        for (name, region, epoch) in said {
            assert_eq!(
                cluster.released(LEASE + 200, name, region, epoch),
                Changes::default(),
                "{name} {region} {epoch}"
            );
        }
        assert_eq!(cluster.assignments("a"), [assignment(0, FIRST_OWNER, 0)]);
        assert_eq!(cluster.coordinator.releases.len(), 1);

        // It is a sign of life all the same: both were last heard from a lease ago
        // otherwise.
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        assert!(cluster.heartbeat_with(2 * LEASE + 1, "a", &[]));
        assert!(cluster.heartbeat_with(2 * LEASE + 1, "b", &[]));
    }

    #[test]
    fn a_region_released_without_being_asked_is_assigned_at_once_even_by_a_new_coordinator() {
        // The coordinator before this one asked for the release. This one knows
        // nothing of it and gives nothing away yet, but the owner itself says that
        // the region is free.
        let mut cluster = Cluster::new(&[]);
        let held = assignment(0, 7, 0);
        assert_eq!(
            cluster.register(1, "a", "a:25601", &[held]),
            changes(&["a"], true)
        );
        cluster.register(2, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(3), Changes::default());
        assert_eq!(cluster.released(4, "a", 0, 7), changes(&["a", "b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 1, 1)]
        );
    }

    #[test]
    fn a_region_released_with_nobody_to_take_it_goes_to_the_first_that_waits_at_a_tick() {
        let mut cluster = Cluster::new(&[0]);
        let held = assignment(0, 7, 0);
        cluster.register(1, "a", "a:25601", &[held]);
        assert_eq!(cluster.released(2, "a", 0, 7), changes(&["a"], true));
        assert!(cluster.table().routes.is_empty());

        // The grace period does not count for it: its owner said that it is free. The
        // worker that released it is the first that waits, being the only one. The
        // other region, of which nobody said anything, waits for the grace period.
        assert_eq!(cluster.tick(3), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 1)]
        );
        cluster.register(4, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
    }

    #[test]
    fn a_region_released_before_a_new_coordinator_knew_of_it_is_assigned_at_once() {
        // The owner was asked by the coordinator before this one, and had let go
        // before it found this one: it registers holding nothing and says so.
        let mut cluster = Cluster::new(&[]);
        assert_eq!(cluster.register(1, "a", "a:25601", &[]), Changes::default());
        assert_eq!(cluster.register(2, "b", "b:25601", &[]), Changes::default());
        assert_eq!(cluster.released(3, "a", 0, 7), changes(&["b"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
        assert!(cluster.assignments("a").is_empty());
    }

    #[test]
    fn a_region_released_before_anyone_waits_goes_to_the_next_that_registers_and_ticks() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(1, "a", "a:25601", &[]);
        // It may be given back to the worker that let go of it, if that is all there is.
        assert_eq!(cluster.released(2, "a", 0, 7), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 1, 0)]
        );
    }

    #[test]
    fn a_release_from_before_does_not_free_a_region_that_has_an_owner_or_a_later_epoch() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(1, "a", "a:25601", &[]);
        cluster.register(2, "b", "b:25601", &[assignment(0, 9, 0)]);
        // Another worker reported it first.
        assert_eq!(cluster.released(3, "a", 0, 7), Changes::default());
        assert_eq!(cluster.released(3, "a", 0, 9), Changes::default());
        assert_eq!(cluster.assignments("b"), [assignment(0, 9, 0)]);

        // Its owner lets go with nobody fit to take it; an epoch from before that owner
        // says nothing about the region as it is now.
        cluster.disconnected(4, "a");
        cluster.released(5, "b", 0, 9);
        cluster.disconnected(6, "b");
        assert_eq!(cluster.released(7, "a", 0, 7), Changes::default());
        // Nor does a worker that is not registered free anything.
        assert_eq!(cluster.released(7, "nobody", 0, 9), Changes::default());
        assert!(cluster.table().routes.is_empty());
    }

    #[test]
    fn a_worker_that_released_a_region_waits_behind_the_others() {
        let mut cluster = Cluster::new(&[]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        // Each owner in turn lets go, and the region goes to the worker that has
        // waited longest: `b` and `c` before `a` gets it again.
        let mut epoch = FIRST_OWNER;
        for (releases, next) in [("a", "b"), ("b", "c"), ("c", "a")] {
            let mut told = [releases, next];
            told.sort_unstable();
            assert_eq!(
                cluster.released(LEASE + 1, releases, 0, epoch),
                changes(&told, true)
            );
            epoch += 1;
            assert_eq!(cluster.assignments(next)[0].epoch, epoch, "{next}");
        }
    }

    #[test]
    fn the_region_goes_to_another_target_when_the_reserved_one_is_no_longer_one() {
        let mut cluster = a_runs_and_b_waits();
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 100, 0, Some("b"), 5).unwrap();
        assert_eq!(moving, begun("a", "b"));
        assert_eq!(cluster.disconnected(LEASE + 200, "b"), Changes::default());

        // Whoever asked is told whom the region went to, which no routing table says.
        let expected = Changes {
            moves: vec![outcome(5, 0, Some(("c", NEXT_OWNER)), true)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(cluster.released(LEASE + 300, "a", 0, FIRST_OWNER), expected);
        assert!(cluster.assignments("b").is_empty());
    }

    #[test]
    fn whoever_asked_is_told_when_the_region_ends_up_without_an_owner() {
        let mut cluster = a_runs_and_b_waits();
        cluster.move_region(LEASE + 100, 0, None, 5).unwrap();
        assert_eq!(cluster.disconnected(LEASE + 200, "b"), Changes::default());
        let expected = Changes {
            moves: vec![outcome(5, 0, None, true)],
            ..changes(&["a"], true)
        };
        assert_eq!(cluster.released(LEASE + 300, "a", 0, FIRST_OWNER), expected);
        assert!(cluster.table().routes.is_empty());
        assert!(cluster.coordinator.releases.is_empty());

        // When the owner does not answer and nobody else is left, there is somebody
        // to name after all.
        let mut cluster = a_runs_and_b_waits();
        cluster.move_region(LEASE + 100, 0, None, 6).unwrap();
        cluster.leaving(LEASE + 200, "b");
        assert!(cluster.heartbeat(2 * LEASE, "a"));
        let told = cluster.tick(2 * LEASE + 101);
        assert_eq!(told.moves, [outcome(6, 0, Some(("a", NEXT_OWNER)), false)]);
        // Nobody else waits, so the tick that took the region gave it back to the
        // worker it was taken from, with a new epoch.
        assert_eq!(cluster.assignments("a"), [assignment(0, NEXT_OWNER, 1)]);
    }

    #[test]
    fn a_worker_without_a_connection_is_no_target_until_it_registers_again() {
        let mut cluster = a_runs_and_b_waits();
        assert_eq!(cluster.disconnected(LEASE + 1, "b"), Changes::default());
        assert_eq!(
            cluster.move_region(LEASE + 2, 0, None, 1),
            Err(MoveRefusal::NoTarget(RegionId(0)))
        );
        // Nor is the owner asked to release its region for it when it leaves.
        assert_eq!(cluster.leaving(LEASE + 2, "a"), Changes::default());

        // An earlier connection that ends after the worker is back is none of the
        // coordinator's business: the service only tells it of the latest.
        assert_eq!(
            cluster.register(LEASE + 3, "b", "b:25601", &[]),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
        assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "b");
    }

    #[test]
    fn a_leaving_worker_that_owns_nothing_is_forgotten_at_once() {
        let mut cluster = a_runs_and_b_waits();
        let gone = |name: &str| Changes {
            gone: vec![name.to_owned()],
            ..Changes::default()
        };
        assert_eq!(cluster.leaving(LEASE + 1, "b"), gone("b"));
        assert!(!cluster.heartbeat(LEASE + 1, "b"));
        // One the coordinator does not know has nothing to wait for either.
        assert_eq!(cluster.leaving(LEASE + 1, "nobody"), gone("nobody"));
        assert_eq!(cluster.leaving(LEASE + 1, "b"), gone("b"));
    }

    #[test]
    fn the_region_of_a_leaving_worker_is_released_as_soon_as_a_worker_is_there_to_take_it() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));

        // Nobody is there, so the worker goes on running its region.
        assert_eq!(cluster.leaving(LEASE + 1, "a"), Changes::default());
        assert!(cluster.heartbeat(LEASE + 2, "a"));
        assert_eq!(cluster.tick(LEASE + 2), Changes::default());

        // The spare registers a moment later, and the worker is asked at once, and
        // only once.
        assert_eq!(
            cluster.register(LEASE + 3, "b", "b:25601", &[]),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
        assert_eq!(cluster.tick(LEASE + 3), Changes::default());
        assert_eq!(cluster.leaving(LEASE + 3, "a"), Changes::default());

        // When it owns nothing any more, it is forgotten.
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 4, "a", 0, FIRST_OWNER), expected);
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
        assert!(!cluster.heartbeat(LEASE + 4, "a"));
    }

    #[test]
    fn a_worker_that_leaves_while_another_waits_is_asked_to_release_at_once() {
        let mut cluster = a_runs_and_b_waits();
        assert_eq!(
            cluster.leaving(LEASE + 1, "a"),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
        // Nobody asked for this move, so nobody is told how it ends.
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 2, "a", 0, FIRST_OWNER), expected);
    }

    #[test]
    fn a_leaving_worker_with_two_regions_hands_both_over_and_is_let_go() {
        let mut cluster = Cluster::new(&[-8, 0, 8]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);
        let held = cluster.assignments("a");

        // It is asked to release both at once, for the only other worker there is,
        // which runs two regions itself.
        let both = [order("a", 0, held[0].epoch), order("a", 2, held[1].epoch)];
        assert_eq!(cluster.leaving(LEASE + 1, "a"), asks(&both));
        let meant = |cluster: &Cluster, region: u32| {
            cluster.coordinator.releases[&RegionId(region)].to.clone()
        };
        assert_eq!(
            (meant(&cluster, 0), meant(&cluster, 2)),
            ("b".into(), "b".into())
        );

        // It lets go of one and is still there, as it has another.
        assert_eq!(
            cluster.released(LEASE + 2, "a", 2, held[1].epoch),
            changes(&["a", "b"], true)
        );
        assert_eq!(regions_of(&cluster, "a"), [0]);
        assert!(cluster.heartbeat(LEASE + 2, "a"));
        // With the last one it is forgotten.
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 3, "a", 0, held[0].epoch), expected);
        assert_eq!(regions_of(&cluster, "b"), [0, 1, 2, 3]);
        assert!(!cluster.heartbeat(LEASE + 3, "a"));
        assert!(cluster.coordinator.releases.is_empty());
        assert!(cluster.table().is_complete());

        // With two workers that run nothing, each is the target of one of them.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        cluster.register(LEASE, "b", "b:25601", &[]);
        cluster.register(LEASE, "c", "c:25601", &[]);
        let both = [order("a", 0, held[0].epoch), order("a", 1, held[1].epoch)];
        assert_eq!(cluster.leaving(LEASE + 1, "a"), asks(&both));
        assert_eq!(
            (meant(&cluster, 0), meant(&cluster, 1)),
            ("b".into(), "c".into())
        );
        // It answers neither. Both regions are taken from it when the lease is out,
        // and it is forgotten.
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE + 1, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "b", "c"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 2), expected);
        assert_eq!(regions_of(&cluster, "b"), [0]);
        assert_eq!(regions_of(&cluster, "c"), [1]);
    }

    #[test]
    fn a_leaving_worker_is_asked_although_the_only_target_is_reserved_for_another_release() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        let (moving, _) = cluster.move_region(LEASE + 100, 1, None, 1).unwrap();
        assert_eq!(moving, begun("b", "c"));
        // The owner that is asked has lost its connection, so the worker that is
        // reserved for its region is all there is for the one that leaves.
        assert_eq!(cluster.disconnected(LEASE + 150, "b"), Changes::default());
        assert_eq!(
            cluster.leaving(LEASE + 200, "a"),
            asks(&[order("a", 0, FIRST_EPOCH + 1)])
        );
        assert_eq!(cluster.coordinator.releases[&RegionId(0)].to, "c");

        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(
            cluster.released(LEASE + 300, "a", 0, FIRST_EPOCH + 1),
            expected
        );
        assert_eq!(regions_of(&cluster, "c"), [0]);
        // The other release goes on.
        assert_eq!(cluster.coordinator.releases[&RegionId(1)].to, "c");
    }

    #[test]
    fn leaving_belongs_to_one_registration_and_not_to_a_name() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        assert_eq!(cluster.leaving(LEASE + 1, "a"), Changes::default());

        // The worker registers again and has not said that it leaves since. So it is
        // not asked to release anything when a spare turns up.
        assert_eq!(
            cluster.register(LEASE + 2, "a", "a:25601", &held),
            Changes::default()
        );
        assert_eq!(
            cluster.register(LEASE + 3, "b", "b:25601", &[]),
            Changes::default()
        );
        assert_eq!(cluster.tick(LEASE + 3), Changes::default());

        // It says so again, hands over and is forgotten.
        assert_eq!(
            cluster.leaving(LEASE + 4, "a"),
            asks(&[order("a", 0, FIRST_OWNER)])
        );
        let told = cluster.released(LEASE + 5, "a", 0, FIRST_OWNER);
        assert_eq!(told.gone, ["a"]);

        // The pod that replaces it has its name. It is a worker like any other: the
        // next one to leave hands over to it.
        assert_eq!(
            cluster.register(LEASE + 6, "a", "a:25601", &[]),
            Changes::default()
        );
        assert_eq!(
            cluster.leaving(LEASE + 7, "b"),
            asks(&[order("b", 0, NEXT_OWNER)])
        );
        let expected = Changes {
            gone: vec!["b".to_owned()],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.released(LEASE + 8, "b", 0, NEXT_OWNER), expected);
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
    }

    #[test]
    fn a_leaving_worker_is_not_given_a_region_again_that_was_taken_from_it() {
        let mut cluster = Cluster::new(&[]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(cluster.leaving(LEASE, "a"), Changes::default());

        // It is there but does not vouch for its region, which is not being released,
        // as there is nobody to release it for. A worker that is not leaving would be
        // given the region again, being the only one that waits.
        assert!(cluster.heartbeat_with(2 * LEASE + 1, "a", &[]));
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 1), expected);
        assert!(cluster.table().routes.is_empty());
    }

    #[test]
    fn a_leaving_worker_that_does_not_release_in_time_loses_its_region_and_is_forgotten() {
        let mut cluster = a_runs_and_b_waits();
        cluster.leaving(LEASE + 1, "a");
        for name in ["a", "b"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        let expected = Changes {
            gone: vec!["a".to_owned()],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 2), expected);
        assert_eq!(cluster.assignments("b"), [assignment(0, NEXT_OWNER, 1)]);
    }

    #[test]
    fn a_leaving_worker_whose_connection_ends_is_gone_at_once_and_any_other_keeps_its_region() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        // The other worker is there to hand over to, although it runs a region.
        assert_eq!(
            cluster.leaving(LEASE + 1, "a"),
            asks(&[order("a", 0, FIRST_EPOCH + 1)])
        );
        assert!(cluster.heartbeat(LEASE + 1, "b"));

        // A lost connection alone takes nothing from a worker that is not leaving.
        assert_eq!(cluster.disconnected(LEASE + 2, "b"), Changes::default());
        // The one that is leaving will not be back: no lease is waited for.
        assert_eq!(cluster.disconnected(LEASE + 2, "a"), changes(&["a"], true));
        assert!(!cluster.heartbeat(LEASE + 2, "a"));
        assert_eq!(cluster.table().routes.len(), 1);

        // Its region is given away like any without an owner, once a worker with a
        // connection is there.
        assert_eq!(cluster.tick(LEASE + 2), Changes::default());
        cluster.register(LEASE + 3, "c", "c:25601", &[]);
        assert_eq!(cluster.tick(LEASE + 3), changes(&["c"], true));
        assert_eq!(
            cluster.assignments("c"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
    }

    #[test]
    fn the_region_of_a_leaving_worker_that_vanishes_goes_at_once_to_another_worker() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        cluster.register(LEASE, "c", "c:25601", &[]);
        // Both owners are asked to release for the one worker that does not leave.
        // One of them goes on running, and then its process dies.
        assert_eq!(
            cluster.leaving(LEASE + 1, "a"),
            asks(&[order("a", 0, FIRST_EPOCH + 1)])
        );
        assert_eq!(
            cluster.leaving(LEASE + 1, "b"),
            asks(&[order("b", 1, FIRST_EPOCH + 2)])
        );
        assert_eq!(
            cluster.disconnected(LEASE + 2, "a"),
            changes(&["a", "c"], true)
        );
        // The release went with its owner, and the region to the worker it was
        // meant for all the same, as nobody else is there. The other release goes on.
        assert_eq!(
            cluster.assignments("c"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
        let releases = &cluster.coordinator.releases;
        assert_eq!(Vec::from_iter(releases.keys()), [&RegionId(1)]);
        assert_eq!(releases[&RegionId(1)].to, "c");
    }

    /// The workers that can be given a region, each with how many regions it has,
    /// counting those it is the reserved target of, and its place among the workers:
    /// the least of these is the worker that is given the next region. Counted afresh
    /// from what the coordinator holds, for the randomised test to check its choices by.
    fn fit_workers(coordinator: &Coordinator) -> BTreeMap<String, (usize, u64)> {
        let fit = coordinator
            .workers
            .iter()
            .filter(|(_, worker)| worker.connected && !worker.leaving);
        fit.map(|(name, worker)| {
            let owned = coordinator.assignments(name).len();
            let targets = coordinator.releases.values();
            let reserved = targets.filter(|release| release.to == *name).count();
            (name.clone(), (owned + reserved, worker.arrival))
        })
        .collect()
    }

    /// Numbers that look random and are the same in every run (xorshift64*).
    struct Generator(u64);

    impl Generator {
        fn draw(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        /// A number below `bound`.
        fn below(&mut self, bound: u64) -> u64 {
            self.draw() % bound
        }

        /// True once in `times` times.
        fn once_in(&mut self, times: u64) -> bool {
            self.below(times) == 0
        }
    }

    /// Workers register, send heartbeats or fall silent, hear what they are to run or
    /// miss it, and start afresh, and now and then the coordinator is replaced, all at
    /// random. Regions are moved, workers release them when asked, unasked or not at
    /// all, say that they leave and lose their connections. [`Cluster`] checks every
    /// call; this adds what only shows over time, that whoever is given a region or
    /// picked as a target is the worker with the fewest regions then, that ticks even
    /// regions out, one release at a time, and that a worker that failed a region is
    /// passed over for both as long as that is remembered.
    #[test]
    fn epochs_only_rise_and_nothing_is_shared_whatever_workers_do() {
        const WORKERS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
        let (mut issued, mut resumed, mut turned_away, mut lost) = (0, 0, 0, 0);
        let (mut refused, mut restarts, mut dropped) = (0, 0, 0);
        let (mut moves, mut unmoved, mut let_go, mut overdue) = (0, 0, 0, 0);
        let (mut left, mut vanished, mut asked_to_leave, mut cut_off) = (0, 0, 0, 0);
        let (mut compared, mut meant, mut shared, mut handed_on) = (0, 0, 0, 0);
        let (mut evened, mut even, mut faulted, mut passed_over) = (0, 0, 0, 0);
        // For how long a worker that failed a region is at fault.
        let memory = u64::from(Coordinator::FAULT_MEMORY) * LEASE;

        for seed in 1..=24_u64 {
            let mut random = Generator(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            // In every other run the workers only report what a coordinator gave them.
            let honest = seed % 2 == 0;
            let mut cluster = Cluster::new(&[-8, 0, 8]);
            let fingerprint = cluster.layout.fingerprint();
            let mut now = 0;
            let mut grace_ends = LEASE;
            // The regions that a worker said it let go of and nobody was given since:
            // the grace period does not hold these back.
            let mut free: BTreeSet<RegionId> = BTreeSet::new();
            // What each worker runs: what it was to run when it last heard of it.
            let mut running: BTreeMap<String, Vec<Assignment>> = BTreeMap::new();
            // What each worker has been asked to release and has not answered.
            let mut asked: BTreeMap<String, Vec<(RegionId, u64)>> = BTreeMap::new();
            // The highest epoch any of the coordinators issued or was told of.
            let mut highest_epoch = FIRST_EPOCH;
            // What the present coordinator has seen: the blocks of entity ids it issued
            // or was told of, the latest owner of each region with its epoch, and who
            // waits to hear how a move ended.
            let mut used_blocks: BTreeSet<i32> = BTreeSet::new();
            let mut tenures: BTreeMap<RegionId, (String, u64)> = BTreeMap::new();
            let mut movers: BTreeSet<u64> = BTreeSet::new();
            // When each worker that the present coordinator knows last failed a
            // region: when one was taken from it that it neither released nor was
            // refused by the store, while it was still registered afterwards.
            let mut faults: BTreeMap<String, u64> = BTreeMap::new();

            for step in 0..4000_u64 {
                now += random.below(LEASE / 8);
                let name = WORKERS[random.below(6) as usize];
                let before = cluster.view();
                let releases_before = cluster.coordinator.releases.clone();
                let leaving_before: BTreeSet<String> = cluster
                    .coordinator
                    .workers
                    .iter()
                    .filter(|(_, worker)| worker.leaving)
                    .map(|(name, _)| name.clone())
                    .collect();
                let fit_before = fit_workers(&cluster.coordinator);
                // The workers that are at fault as of this step, going by what was
                // known before it.
                let at_fault = |faults: &BTreeMap<String, u64>| -> BTreeSet<String> {
                    let recent = faults.iter().filter(|(_, failed)| now < **failed + memory);
                    recent.map(|(name, _)| name.clone()).collect()
                };
                let faulty_before = at_fault(&faults);
                cluster.last = None;
                // What the worker reports to hold, if this step is a registration.
                let mut reported: Option<Vec<Assignment>> = None;
                // The region its owner lets go of in this step, if one does.
                let mut let_go_of: Option<RegionId> = None;
                // Whether this step is a tick.
                let mut ticked = false;
                // Whether this step is a call that ends like a tick, in which regions
                // are taken from workers that failed them, and the worker and region
                // of which that is not true: what the store refused it.
                let mut settled = false;
                let mut excused: Option<(&str, RegionId)> = None;
                // Whether what is assigned in this step has to respect the grace
                // period of a new coordinator.
                let mut graceful = true;

                let roll = random.below(120);
                if roll < 30 {
                    // The worker vouches for what it believes it runs, mostly as being
                    // committed, now and then as waiting for the store, or not at all.
                    let mut vouches = Vec::new();
                    for held in running.get(name).into_iter().flatten() {
                        match random.below(8) {
                            0 => {}
                            1 => vouches.push((held.region, Vouch::WaitingForStore)),
                            _ => vouches.push((held.region, Vouch::Committed)),
                        }
                    }
                    if !honest && random.once_in(8) {
                        vouches.push((RegionId(random.below(6) as u32), Vouch::Committed));
                    }
                    cluster.heartbeat_with(now, name, &vouches);
                } else if roll < 60 {
                    let mut holding = running.get(name).cloned().unwrap_or_default();
                    if !honest && random.once_in(4) {
                        // Something made up: any region, even one that does not exist,
                        // an epoch near the latest, and ids that may well be in use.
                        let live: Vec<&Assignment> =
                            before.assignments.values().flatten().collect();
                        let entity_ids = if live.is_empty() || random.once_in(2) {
                            ids(random.below(40) as u32)
                        } else {
                            live[random.below(live.len() as u64) as usize].entity_ids
                        };
                        holding.push(Assignment {
                            region: RegionId(random.below(6) as u32),
                            epoch: highest_epoch - 3 + random.below(7),
                            entity_ids,
                        });
                    }
                    let layout = match random.below(20) {
                        0 => Some(fingerprint ^ 1),
                        1..=4 => None,
                        _ => Some(fingerprint),
                    };
                    let address = format!("{name}:{}", 25601 + random.below(3));
                    if cluster
                        .register_as(now, name, &address, &holding, layout)
                        .is_err()
                    {
                        assert_eq!(layout, Some(fingerprint ^ 1));
                        refused += 1;
                        continue;
                    }
                    let has = cluster.assignments(name);
                    // Whatever the worker has that it did not have, it reported.
                    for gained in has.iter().filter(|a| !before.assignments(name).contains(a)) {
                        assert!(holding.contains(gained), "{gained:?} came from nowhere");
                        resumed += 1;
                    }
                    turned_away += holding.iter().filter(|held| !has.contains(held)).count();
                    for reported in &holding {
                        highest_epoch = highest_epoch.max(reported.epoch);
                        used_blocks.insert(reported.entity_ids.first.0 / EntityIds::BLOCK_SIZE);
                    }
                    // Most of the time the answer reaches the worker.
                    if !random.once_in(4) {
                        running.insert(name.to_owned(), has);
                    }
                    // A region it was asked to release and came back without went to
                    // another worker at once, whatever the grace period says.
                    graceful = false;
                    reported = Some(holding);
                } else if roll < 88 {
                    let changes = cluster.tick(now);
                    ticked = true;
                    settled = true;
                    for worker in WORKERS {
                        let had = before.assignments(worker);
                        let has = cluster.assignments(worker);
                        lost += had.iter().filter(|held| !has.contains(held)).count();
                    }
                    // Not every worker gets to hear that something has changed for it.
                    for worker in changes.workers {
                        if random.once_in(2) {
                            let has = cluster.assignments(&worker);
                            running.insert(worker, has);
                        }
                    }
                } else if roll < 91 {
                    // The store refuses to let the worker open one of its regions: it
                    // has seen a higher epoch, which some coordinator issued.
                    let held = running.get(name).and_then(|held| held.first()).copied();
                    let (region, epoch) = match held {
                        Some(held) => (held.region, held.epoch),
                        None => (RegionId(random.below(6) as u32), highest_epoch - 3),
                    };
                    let seen = if honest {
                        if epoch >= highest_epoch {
                            continue;
                        }
                        epoch + 1 + random.below(highest_epoch - epoch)
                    } else {
                        highest_epoch - 3 + random.below(7)
                    };
                    let owned = before
                        .assignments(name)
                        .iter()
                        .any(|held| held.region == region && held.epoch < seen);
                    cluster.epoch_refused(now, name, region.0, seen);
                    highest_epoch = highest_epoch.max(seen);
                    let has = cluster.assignments(name);
                    assert!(
                        !has.iter()
                            .any(|held| held.region == region && held.epoch < seen)
                    );
                    settled = true;
                    if owned {
                        dropped += 1;
                        excused = Some((name, region));
                    }
                    // The worker has dropped the region.
                    if let Some(held) = running.get_mut(name) {
                        held.retain(|held| held.region != region);
                    }
                } else if roll < 94 {
                    // The worker catches up with what it is to run.
                    running.insert(name.to_owned(), cluster.assignments(name));
                } else if roll < 97 {
                    // The worker's process is replaced by a new one, which runs nothing
                    // and has been asked nothing.
                    running.remove(name);
                    asked.remove(name);
                } else if roll < 100 {
                    if random.once_in(3) {
                        // The coordinator is replaced by a new one, whose clock tells
                        // it no more than that it is not behind the old one.
                        cluster.restart(now, highest_epoch);
                        grace_ends = now + LEASE;
                        free.clear();
                        used_blocks.clear();
                        tenures.clear();
                        movers.clear();
                        faults.clear();
                        restarts += 1;
                        continue;
                    }
                } else if roll < 106 {
                    // Somebody asks for a region to be moved, now and then for one that
                    // does not exist, and sometimes to a certain worker.
                    let region = random.below(5) as u32;
                    let to = random.once_in(3).then_some(name);
                    match cluster.move_region(now, region, to, step) {
                        Ok((begun, _)) => {
                            let had = before.assignments(&begun.from).iter();
                            assert!(had.map(|held| held.region.0).any(|held| held == region));
                            // The target can be given a region, however many it runs.
                            // If nobody named it, no other has fewer, nor has one
                            // with as many waited longer.
                            assert!(fit_before.contains_key(&begun.to), "{begun:?}");
                            assert_ne!(begun.from, begun.to);
                            // One that failed a region of late comes after all
                            // those that did not.
                            let others = fit_before.iter();
                            let chosen = others
                                .filter(|(other, _)| **other != begun.from)
                                .min_by_key(|(other, load)| {
                                    (faulty_before.contains(*other), **load)
                                })
                                .map(|(other, _)| other.as_str());
                            match to {
                                Some(to) => assert_eq!(to, begun.to),
                                None => assert_eq!(chosen, Some(begun.to.as_str())),
                            }
                            movers.insert(step);
                            moves += 1;
                        }
                        Err(_) => unmoved += 1,
                    }
                } else if roll < 112 {
                    // The worker lets go of a region: one it was asked to release,
                    // whether or not it runs it, or one it runs without being asked,
                    // or, if it makes things up, anything.
                    let pending = asked.get_mut(name).and_then(Vec::pop);
                    let own = running.get(name).and_then(|held| held.first());
                    let own = own.map(|held| (held.region, held.epoch));
                    let made_up = (
                        RegionId(random.below(5) as u32),
                        highest_epoch - 3 + random.below(7),
                    );
                    let said = match (pending, own) {
                        _ if !honest && random.once_in(4) => Some(made_up),
                        (Some(pending), _) => Some(pending),
                        (None, Some(own)) if random.once_in(2) => Some(own),
                        _ => None,
                    };
                    let Some((region, epoch)) = said else {
                        continue;
                    };
                    let owned = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
                    let owned = before.assignments(name).iter().any(owned);
                    let changes = cluster.released(now, name, region.0, epoch);
                    if owned {
                        // It no longer has the region, whoever has it now: the
                        // worker it was to be released for, if that can be given it.
                        let has = cluster.assignments(name);
                        assert!(has.iter().all(|held| held.region != region), "{has:?}");
                        let target = releases_before.get(&region).map(|release| &release.to);
                        if let Some(target) = target.filter(|to| fit_before.contains_key(*to)) {
                            let has = cluster.assignments(target);
                            assert!(has.iter().any(|held| held.region == region), "{has:?}");
                        }
                        let_go += 1;
                        let_go_of = Some(region);
                        free.insert(region);
                    } else if before.table.route(region).is_none() {
                        // Nobody owns it: the worker may have been its last owner,
                        // before this coordinator knew of it.
                        free.insert(region);
                    } else {
                        assert_eq!(changes, Changes::default());
                    }
                    if let Some(held) = running.get_mut(name) {
                        held.retain(|held| (held.region, held.epoch) != (region, epoch));
                    }
                    graceful = false;
                } else if roll < 116 {
                    let known = cluster.coordinator.workers.contains_key(name);
                    cluster.leaving(now, name);
                    asked_to_leave += usize::from(known);
                } else {
                    // A connection ends: that of a worker that is leaving, if there is
                    // one, half of the time, as such a worker soon exits.
                    let leaver = leaving_before.first().filter(|_| random.once_in(2));
                    let name = leaver.map_or(name, String::as_str);
                    let owned = !before.assignments(name).is_empty();
                    let known = cluster.coordinator.workers.contains_key(name);
                    cluster.disconnected(now, name);
                    settled = true;
                    if leaving_before.contains(name) {
                        // It is gone, with what it owned.
                        assert!(!cluster.coordinator.workers.contains_key(name));
                        vanished += usize::from(owned);
                    } else {
                        assert_eq!(cluster.assignments(name), before.assignments(name));
                        cut_off += usize::from(known);
                    }
                }

                let said = cluster.last.take().unwrap_or_default();
                let after = cluster.view();

                // Who failed a region in this step. The coordinator remembers that
                // of a worker for as long as it knows the worker, and nothing else.
                if settled {
                    for worker in WORKERS {
                        let has = after.assignments(worker);
                        let gone = |held: &&Assignment| {
                            let same = |has: &Assignment| {
                                (has.region, has.epoch) == (held.region, held.epoch)
                            };
                            excused != Some((worker, held.region)) && !has.iter().any(same)
                        };
                        if before.assignments(worker).iter().any(|held| gone(&held)) {
                            faults.insert(worker.to_owned(), now);
                            faulted += 1;
                        }
                    }
                }
                faults.retain(|name, _| cluster.coordinator.workers.contains_key(name));
                for (name, worker) in &cluster.coordinator.workers {
                    let failed = faults.get(name).map(|failed| cluster.at(*failed));
                    assert_eq!(worker.failed, failed, "{name}");
                }
                // A worker that came to be at fault in this step was so for some of
                // what the step did and not for the rest.
                let faulty_after = at_fault(&faults);
                let faulty = |name: &str| {
                    let before = faulty_before.contains(name);
                    (before == faulty_after.contains(name)).then_some(before)
                };

                // What workers were given in this step, as opposed to what they said
                // they held, in the order it was issued: it is above every epoch
                // before, and has entity ids nobody was given.
                let mut gained: Vec<(&str, Assignment)> = Vec::new();
                for worker in WORKERS {
                    let had = before.assignments(worker);
                    let held = reported.as_deref().filter(|_| worker == name);
                    for has in after.assignments(worker) {
                        if !had.contains(has) && !held.is_some_and(|held| held.contains(has)) {
                            gained.push((worker, *has));
                        }
                    }
                }
                gained.sort_by_key(|(_, assignment)| assignment.epoch);
                for (worker, assignment) in &gained {
                    assert!(
                        !graceful || now >= grace_ends || free.contains(&assignment.region),
                        "assigned during the grace period"
                    );
                    free.remove(&assignment.region);
                    assert!(assignment.epoch > highest_epoch, "{assignment:?}");
                    highest_epoch = assignment.epoch;
                    let block = assignment.entity_ids.first.0 / EntityIds::BLOCK_SIZE;
                    assert_eq!(assignment.entity_ids, ids(block as u32));
                    assert!(used_blocks.insert(block), "{assignment:?}");
                    issued += 1;
                    // A leaving worker is never given a region.
                    assert!(
                        !leaving_before.contains(*worker) || reported.is_some() && *worker == name,
                        "{worker} is leaving and was given {assignment:?}"
                    );
                }
                // Whoever was given a region in this step had the fewest regions of the
                // workers that could be given it at that moment, and was at fault only
                // if all of them were: a worker that is not at fault comes before one
                // that is, whatever the two run. What the others had
                // then is known for sure only if they lost nothing in this step and no
                // release that was meant for them ended in it, so each of them is
                // counted between the least and the most it can have had. A region
                // that goes to the target of its release was counted as that worker's
                // before; one that was let go of or released does not go back to the
                // worker it was taken from.
                let coordinator = &cluster.coordinator;
                let fit = fit_workers(coordinator);
                let lasted = |region: &RegionId, release: &Release| {
                    coordinator.releases.get(region).is_some_and(|now| {
                        (now.epoch, &now.from, &now.to)
                            == (release.epoch, &release.from, &release.to)
                    })
                };
                for (index, (worker, assignment)) in gained.iter().enumerate() {
                    let region = assignment.region;
                    assert!(
                        fit.contains_key(*worker),
                        "{worker} was given {assignment:?}"
                    );
                    let release = releases_before.get(&region);
                    if release.is_some_and(|release| release.to == *worker) {
                        meant += 1;
                        continue;
                    }
                    let handed_over = release.is_some() || let_go_of == Some(region);
                    let owned = |held: &&Assignment| held.region == region;
                    let old = WORKERS
                        .into_iter()
                        .find(|old| before.assignments(old).iter().any(|held| owned(&held)));
                    let least = |other: &str| {
                        let later = gained[index..].iter();
                        let reserved = releases_before.iter();
                        after.assignments(other).len()
                            - later.filter(|(given, _)| *given == other).count()
                            + reserved
                                .filter(|(region, release)| lasted(region, release))
                                .filter(|(_, release)| release.to == other)
                                .count()
                    };
                    let most = |other: &str| {
                        let had = before.assignments(other).iter();
                        let has = after.assignments(other);
                        let ended = releases_before.iter();
                        least(other)
                            + had.filter(|held| !has.contains(held)).count()
                            + ended
                                .filter(|(ended, release)| {
                                    **ended != region && !lasted(ended, release)
                                })
                                .filter(|(_, release)| release.to == other)
                                .count()
                    };
                    let had = least(worker);
                    shared += usize::from(had > 0);
                    for (other, (_, arrival)) in &fit {
                        if other == worker || handed_over && old == Some(other.as_str()) {
                            continue;
                        }
                        match (faulty(worker), faulty(other)) {
                            (Some(mine), Some(theirs)) if mine == theirs => {}
                            (Some(false), Some(true)) => {
                                passed_over += usize::from(least(other) < had);
                                continue;
                            }
                            (Some(true), Some(false)) => panic!(
                                "{worker} is at fault and was given {assignment:?}, \
                                 although {other} is not"
                            ),
                            _ => continue,
                        }
                        assert!(
                            had <= most(other),
                            "{worker} had {had} regions and was given {assignment:?}, \
                             although {other} had {} at most",
                            most(other)
                        );
                        if least(other) == most(other) && had == most(worker) {
                            let waited = fit[*worker].1;
                            assert!(
                                had < least(other) || waited < *arrival,
                                "{worker} was given {assignment:?} before {other}, \
                                 which had as few regions and had waited longer"
                            );
                            compared += 1;
                        }
                    }
                }
                // The releases that nobody asked for and that were begun in this step:
                // for workers that leave, or to even regions out.
                let leaves = |name: &str| {
                    let worker = coordinator.workers.get(name);
                    worker.is_some_and(|worker| worker.leaving)
                };
                let (begun, evening): (Vec<_>, Vec<_>) = coordinator
                    .releases
                    .iter()
                    .filter(|(_, release)| release.mover.is_none())
                    .filter(|(region, release)| {
                        let before = releases_before.get(region);
                        !before.is_some_and(|before| before.epoch == release.epoch)
                    })
                    .partition(|(_, release)| leaves(&release.from));

                // A release to even out is begun by a tick alone, when the coordinator
                // is no longer new and no other release is under way. It takes the
                // highest region of a worker with the most regions among those that
                // can be given one and are not at fault, the one of them that has
                // waited least, for the one of them with the fewest, which has fewer
                // by two regions or more. All faults of the step are known by then.
                let owned = |name: &str| after.assignments(name).len();
                let sound: Vec<(&String, u64)> = fit
                    .iter()
                    .filter(|(name, _)| !faulty_after.contains(*name))
                    .map(|(name, (_, arrival))| (name, *arrival))
                    .collect();
                for (region, release) in &evening {
                    assert!(ticked && now >= grace_ends, "{release:?}");
                    assert_eq!(coordinator.releases.len(), 1, "{release:?}");
                    let heavy = sound
                        .iter()
                        .max_by_key(|(name, arrival)| (owned(name), *arrival))
                        .map(|(name, _)| *name);
                    let light = sound
                        .iter()
                        .min_by_key(|(name, arrival)| (owned(name), *arrival))
                        .map(|(name, _)| *name);
                    assert_eq!(heavy, Some(&release.from), "{release:?}");
                    assert_eq!(light, Some(&release.to), "{release:?}");
                    assert!(owned(&release.from) >= owned(&release.to) + 2);
                    let highest = after.assignments(&release.from).last();
                    assert_eq!(highest.map(|held| held.region), Some(**region));
                    evened += 1;
                }
                // And no tick leaves two of those workers further apart than by one
                // region without having begun to do something about it.
                if ticked && now >= grace_ends && coordinator.releases.is_empty() {
                    let most = sound.iter().map(|(name, _)| owned(name)).max();
                    let fewest = sound.iter().map(|(name, _)| owned(name)).min();
                    assert!(most <= fewest.map(|fewest| fewest + 1), "{sound:?}");
                    even += 1;
                }

                // The releases that were begun for workers that leave are begun in the
                // order of their regions, each for the worker with the fewest regions,
                // and each counting for its target from then on.
                for (index, (_, release)) in begun.iter().enumerate() {
                    let later = |other: &str| {
                        let later = begun[index..].iter();
                        later.filter(|(_, later)| later.to == other).count()
                    };
                    let chosen = fit
                        .iter()
                        .filter(|(other, _)| **other != release.from)
                        .min_by_key(|(other, (load, arrival))| {
                            let at_fault = faulty_after.contains(*other);
                            (at_fault, load - later(other), *arrival)
                        })
                        .map(|(other, _)| other);
                    assert_eq!(chosen, Some(&release.to), "{release:?}");
                    handed_on += 1;
                }

                // Whoever asked for a move hears once how it ended, and that is when
                // the release ends.
                for outcome in &said.moves {
                    assert!(movers.remove(&outcome.mover), "{outcome:?}");
                    overdue += usize::from(!outcome.released);
                }
                let waiting: BTreeSet<u64> = cluster
                    .coordinator
                    .releases
                    .values()
                    .filter_map(|release| release.mover)
                    .collect();
                assert_eq!(movers, waiting);
                left += said.gone.len();
                // Not every worker gets to hear that it is to release something.
                for order in said.releases {
                    if !random.once_in(3) {
                        let orders = asked.entry(order.worker).or_default();
                        orders.push((order.region, order.epoch));
                    }
                }

                // A region never goes back to a lower epoch, and unless workers make
                // things up, another owner means a higher one.
                for (worker, assignments) in &after.assignments {
                    for assignment in assignments {
                        if let Some((owner, epoch)) = tenures.get(&assignment.region) {
                            assert!(assignment.epoch >= *epoch, "{assignment:?}");
                            if honest && owner != worker {
                                assert!(assignment.epoch > *epoch, "{assignment:?}");
                            }
                        }
                        let tenure = (worker.clone(), assignment.epoch);
                        tenures.insert(assignment.region, tenure);
                    }
                }
            }
        }

        // All of it has in fact happened, and often.
        let counts = [
            issued,
            resumed,
            turned_away,
            lost,
            refused,
            restarts,
            dropped,
            moves,
            unmoved,
            let_go,
            overdue,
            left,
            vanished,
            asked_to_leave,
            cut_off,
            compared,
            meant,
            shared,
            handed_on,
            evened,
            even,
            faulted,
            passed_over,
        ];
        assert!(counts.iter().all(|count| *count >= 100), "{counts:?}");
    }

    /// Short for [`FIRST_EPOCH`], where every epoch of a test is spelt out.
    const E: u64 = FIRST_EPOCH;

    fn prepare(worker: &str, region: u32, epoch: u64) -> ReshapeOrder {
        ReshapeOrder {
            worker: worker.to_owned(),
            order: Order::Prepare {
                region: RegionId(region),
                epoch,
            },
        }
    }

    fn absorb(worker: &str, region: u32, epoch: u64, absorbed: u32, as_epoch: u64) -> ReshapeOrder {
        ReshapeOrder {
            worker: worker.to_owned(),
            order: Order::Absorb {
                region: RegionId(region),
                epoch,
                absorbed: RegionId(absorbed),
                as_epoch,
            },
        }
    }

    /// The chunks every split of these tests names.
    const CHUNKS: [ChunkPos; 2] = [ChunkPos::new(7, 1), ChunkPos::new(8, 1)];

    fn split_off(worker: &str, region: u32, epoch: u64, as_epoch: u64, part: u32) -> ReshapeOrder {
        ReshapeOrder {
            worker: worker.to_owned(),
            order: Order::SplitOff {
                region: RegionId(region),
                epoch,
                chunks: CHUNKS.to_vec(),
                as_epoch,
                part: RegionId(part),
            },
        }
    }

    /// How the merge that `asker` asked for ended.
    fn merged(
        asker: Option<u64>,
        survivor: u32,
        absorbed: u32,
        outcome: Result<u32, Undone>,
    ) -> Reshaped {
        Reshaped {
            asker,
            asked: Asked::Merge {
                survivor: RegionId(survivor),
                absorbed: RegionId(absorbed),
            },
            outcome: outcome.map(RegionId),
        }
    }

    /// How the split that `asker` asked for ended.
    fn was_split(asker: Option<u64>, region: u32, outcome: Result<u32, Undone>) -> Reshaped {
        Reshaped {
            asker,
            asked: Asked::Split {
                region: RegionId(region),
            },
            outcome: outcome.map(RegionId),
        }
    }

    /// What a call says that only asks for the list to be read.
    fn reads() -> Changes {
        Changes {
            read: true,
            ..Changes::default()
        }
    }

    /// A world of three stripes with a worker each, `a`, `b` and `c` in the order of
    /// the regions, at the time `LEASE`, and the list as the world store has it then,
    /// which the coordinator has been handed. The regions have the epochs `E + 1` to
    /// `E + 3`.
    fn three_stripes() -> (Cluster, RegionList) {
        let mut cluster = Cluster::new(&[0, 4]);
        for name in ["a", "b", "c"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b", "c"], true));
        let list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 2), living(2, E + 3)],
            ..stripes(3)
        };
        // The home region is new to the table.
        assert_eq!(cluster.listed(LEASE, &list), changes(&[], true));
        beat(&mut cluster, LEASE);
        (cluster, list)
    }

    /// Every worker of [`three_stripes`] is heard from and vouches for what it runs.
    fn beat(cluster: &mut Cluster, at: u64) {
        for name in ["a", "b", "c"] {
            assert!(cluster.heartbeat(at, name));
        }
    }

    /// The list of [`three_stripes`] after region 0 has absorbed region 1.
    fn after_the_merge(list: &RegionList) -> RegionList {
        RegionList {
            regions: vec![living(0, E + 1), living(2, E + 3)],
            absorbed: vec![(RegionId(1), RegionId(0))],
            ..list.clone()
        }
    }

    /// [`three_stripes`] with region 0 asked at `LEASE + 1` to absorb region 1, whose
    /// owner let go of it at `LEASE + 2`: the survivor's owner has been told to open
    /// it with the epoch `E + 4`. Somebody numbered 7 asked.
    fn absorbing() -> (Cluster, RegionList) {
        let (mut cluster, list) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        cluster.released(LEASE + 2, "b", 1, E + 2);
        (cluster, list)
    }

    #[test]
    fn a_merge_has_the_one_region_released_and_then_absorbed_and_the_list_says_when_it_is_done() {
        let (mut cluster, list) = three_stripes();
        let version = cluster.table().version;

        // The owner of the one is asked to release it, that of the other to prepare.
        let asked = cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        let expected = Changes {
            releases: vec![order("b", 1, E + 2)],
            orders: vec![prepare("a", 0, E + 1)],
            ..Changes::default()
        };
        assert_eq!(asked, expected);

        // When it has let go, the region is nobody's and is given to nobody: the
        // survivor's owner is to open it, with an epoch above every other.
        let released = cluster.released(LEASE + 2, "b", 1, E + 2);
        let expected = Changes {
            orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
            ..changes(&["b"], true)
        };
        assert_eq!(released, expected);
        assert!(cluster.assignments("b").is_empty());
        let table = cluster.table();
        assert_eq!(table.version, version + 1);
        assert_eq!(
            table.routes,
            [route(0, E + 1, "a:25601"), route(2, E + 3, "c:25601")]
        );
        assert_eq!(table.waiting, 1);
        assert!(!table.is_complete());
        assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
        // Ticks leave it so, although a worker waits that runs nothing.
        for quarter in 1..=3 {
            assert_eq!(
                cluster.tick(LEASE + quarter * LEASE / 4),
                Changes::default()
            );
        }

        // The worker's word that it is done decides nothing: the list is asked for.
        let said = cluster.absorb_ended(LEASE + 3 * LEASE / 4, "a", 0, 1, Ok(()));
        assert_eq!(said, reads());
        assert_eq!(cluster.table(), table);

        // The list has the region as absorbed: it is gone, and whoever asked is told.
        let merged_list = after_the_merge(&list);
        let done = cluster.listed(LEASE + 3 * LEASE / 4 + 1, &merged_list);
        let expected = Changes {
            routing: true,
            reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
            ..Changes::default()
        };
        assert_eq!(done, expected);
        let table = cluster.table();
        assert_eq!(table.version, version + 2);
        assert_eq!(table.absorbed, [(RegionId(1), RegionId(0))]);
        assert_eq!(table.home, Some(RegionId(0)));
        assert_eq!(table.routes.len(), 2);
        assert!(table.is_complete());
        assert!(cluster.coordinator.merges.is_empty());

        // Nothing is left to do, and the region can be merged or moved again.
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(2 * LEASE), Changes::default());
        assert_eq!(
            cluster.merge(2 * LEASE, 0, 1, None),
            Err(ReshapeRefusal::NoSuchRegion(RegionId(1)))
        );
        assert!(cluster.move_region(2 * LEASE, 0, None, 9).is_ok());
    }

    #[test]
    fn the_merges_and_splits_under_way_are_told_from_when_they_begin_until_they_end() {
        let (mut cluster, list) = three_stripes();
        assert!(cluster.coordinator.under_way().is_empty());
        let merge = Asked::Merge {
            survivor: RegionId(0),
            absorbed: RegionId(1),
        };
        let split = Asked::Split {
            region: RegionId(2),
        };

        // Whoever asked, and at every stage of a merge. A refusal begins nothing.
        cluster.split(LEASE + 1, 2, &CHUNKS, Some(8)).unwrap();
        assert_eq!(cluster.coordinator.under_way(), [split]);
        cluster.merge(LEASE + 1, 0, 1, None).unwrap();
        assert!(cluster.merge(LEASE + 1, 0, 2, None).is_err());
        assert_eq!(cluster.coordinator.under_way(), [merge, split]);
        cluster.released(LEASE + 2, "b", 1, E + 2);
        cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(()));
        assert_eq!(cluster.coordinator.under_way(), [merge, split]);

        // The merge ends with the list, and the split with its worker's word.
        cluster.listed(LEASE + 4, &after_the_merge(&list));
        assert_eq!(cluster.coordinator.under_way(), [split]);
        cluster.split_ended(LEASE + 5, "c", 2, E + 4, Err(Off::Nobody));
        assert!(cluster.coordinator.under_way().is_empty());
    }

    #[test]
    fn a_merge_is_refused_for_the_first_reason_that_holds_and_changes_nothing() {
        let (mut cluster, _) = three_stripes();
        let refused = |cluster: &mut Cluster, survivor, absorbed| {
            cluster
                .merge(LEASE + 1, survivor, absorbed, None)
                .unwrap_err()
        };
        let (zero, one, two) = (RegionId(0), RegionId(1), RegionId(2));

        // A region nobody knows comes before everything, the survivor first.
        assert_eq!(
            refused(&mut cluster, 8, 9),
            ReshapeRefusal::NoSuchRegion(RegionId(8))
        );
        assert_eq!(
            refused(&mut cluster, 1, 9),
            ReshapeRefusal::NoSuchRegion(RegionId(9))
        );
        assert_eq!(
            refused(&mut cluster, 9, 9),
            ReshapeRefusal::NoSuchRegion(RegionId(9))
        );
        assert_eq!(refused(&mut cluster, 1, 1), ReshapeRefusal::Same);
        // The home region absorbs, and is never absorbed.
        assert_eq!(refused(&mut cluster, 1, 0), ReshapeRefusal::Home);
        assert_eq!(refused(&mut cluster, 0, 0), ReshapeRefusal::Same);

        // A region that is being moved.
        cluster.move_region(LEASE + 1, 2, Some("a"), 5).unwrap();
        assert_eq!(
            refused(&mut cluster, 1, 2),
            ReshapeRefusal::BeingReleased(two)
        );
        assert_eq!(
            refused(&mut cluster, 2, 1),
            ReshapeRefusal::BeingReleased(two)
        );
        cluster.released(LEASE + 1, "c", 2, E + 3);
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);

        // A worker that could not be told, or that will not be there. The owner of
        // the region to absorb only has to hear that it is to let go.
        cluster.disconnected(LEASE + 1, "b");
        let unfit = |worker: &str, why| ReshapeRefusal::Unfit {
            worker: worker.to_owned(),
            why,
        };
        let cut_off = "has no connection to the coordinator";
        assert_eq!(refused(&mut cluster, 1, 2), unfit("b", cut_off));
        assert_eq!(refused(&mut cluster, 2, 1), unfit("b", cut_off));
        cluster.register(LEASE + 1, "b", "b:25601", &[assignment(1, E + 2, 1)]);
        cluster.leaving(LEASE + 1, "b");
        // It is being released for the leaver at once, as there is a target.
        assert_eq!(
            refused(&mut cluster, 1, 2),
            ReshapeRefusal::BeingReleased(one)
        );
        cluster.released(LEASE + 1, "b", 1, E + 2);

        // A region that is part of a merge under way, as either part of another.
        cluster.merge(LEASE + 1, 0, 1, None).unwrap();
        assert_eq!(refused(&mut cluster, 0, 2), ReshapeRefusal::Reserved(zero));
        assert_eq!(refused(&mut cluster, 2, 1), ReshapeRefusal::Reserved(one));
        assert_eq!(refused(&mut cluster, 1, 2), ReshapeRefusal::Reserved(one));
        // Being reserved comes after being the same and being the home region.
        assert_eq!(refused(&mut cluster, 1, 1), ReshapeRefusal::Same);
        assert_eq!(refused(&mut cluster, 1, 0), ReshapeRefusal::Home);
        assert_eq!(refused(&mut cluster, 2, 0), ReshapeRefusal::Home);
        assert_eq!(
            cluster.split(LEASE + 1, 0, &CHUNKS, None),
            Err(ReshapeRefusal::Reserved(zero))
        );
        assert_eq!(
            cluster.move_region(LEASE + 1, 1, None, 5),
            Err(MoveRefusal::Reserved(one))
        );
    }

    #[test]
    fn a_leaving_survivor_is_unfit_and_a_region_without_an_owner_is_not_merged() {
        let (mut cluster, _) = three_stripes();
        // The owner of region 2 says that it leaves, with nobody to hand over to: the
        // other two have no connection.
        cluster.disconnected(LEASE + 1, "a");
        cluster.disconnected(LEASE + 1, "b");
        assert_eq!(cluster.leaving(LEASE + 1, "c"), Changes::default());
        let unfit = |worker: &str, why| ReshapeRefusal::Unfit {
            worker: worker.to_owned(),
            why,
        };
        let cut_off = "has no connection to the coordinator";
        assert_eq!(
            cluster.merge(LEASE + 1, 2, 1, None),
            Err(unfit("c", "is leaving"))
        );
        assert_eq!(
            cluster.split(LEASE + 1, 2, &CHUNKS, None),
            Err(unfit("c", "is leaving"))
        );
        assert_eq!(
            cluster.merge(LEASE + 1, 0, 2, None),
            Err(unfit("a", cut_off))
        );
        assert_eq!(
            cluster.split(LEASE + 1, 0, &CHUNKS, None),
            Err(unfit("a", cut_off))
        );
        // What a split is to take is looked at after who is to do it.
        assert_eq!(
            cluster.split(LEASE + 1, 0, &[], None),
            Err(unfit("a", cut_off))
        );
        assert_eq!(
            cluster.split(LEASE + 1, 9, &[], None),
            Err(ReshapeRefusal::NoSuchRegion(RegionId(9)))
        );

        // The lease of one of them runs out, and nobody can be given its region.
        for name in ["a", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        let later = 2 * LEASE + 1;
        assert_eq!(cluster.tick(later), changes(&["b"], true));
        let no_owner = Err(ReshapeRefusal::NoOwner(RegionId(1)));
        // That comes before whether the other owner can be told.
        assert_eq!(cluster.merge(later, 0, 1, None), no_owner);
        assert_eq!(cluster.merge(later, 1, 2, None), no_owner);
        assert_eq!(cluster.split(later, 1, &CHUNKS, None), no_owner);
    }

    #[test]
    fn a_split_wants_chunks_and_a_reading_of_the_list() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.tick(LEASE);
        assert_eq!(
            cluster.split(LEASE, 0, &[], None),
            Err(ReshapeRefusal::NoChunks)
        );
        // The new region is named by the list, which nobody has read.
        assert_eq!(
            cluster.split(LEASE, 0, &CHUNKS, None),
            Err(ReshapeRefusal::Unlisted)
        );
        // A reading that failed does not help.
        assert_eq!(cluster.unlisted(LEASE), Changes::default());
        assert_eq!(
            cluster.split(LEASE, 0, &CHUNKS, None),
            Err(ReshapeRefusal::Unlisted)
        );
        cluster.listed(LEASE, &stripes(2));
        assert!(cluster.split(LEASE, 0, &CHUNKS, None).is_ok());

        // A merge is asked of the regions the coordinator knows, with or without it.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        cluster.tick(LEASE);
        assert!(cluster.merge(LEASE, 1, 0, None).is_ok());
    }

    #[test]
    fn what_a_refusal_and_an_outcome_say_in_words() {
        let unfit = ReshapeRefusal::Unfit {
            worker: "a".to_owned(),
            why: "is leaving",
        };
        assert_eq!(
            unfit.to_string(),
            "the worker a, which would have to do it, is leaving"
        );
        assert_eq!(
            ReshapeRefusal::Reserved(RegionId(3)).to_string(),
            "region 3 is part of a merge or a split that is under way"
        );
        assert_eq!(
            Undone::Off(Off::Nobody).to_string(),
            "no player stands in a chunk named that the region holds"
        );
        let declined = Off::Declined(Decline::NotOpened { epoch: Some(12) });
        assert_eq!(
            Undone::Off(declined).to_string(),
            "the world store declined: the region to absorb has an owner with epoch 12"
        );
        assert_eq!(
            Undone::Gone(RegionId(4)).to_string(),
            "the world store's list of regions no longer has region 4"
        );
    }

    #[test]
    fn the_regions_of_a_merge_are_not_taken_for_want_of_vouching_and_count_as_vouched_at_its_end() {
        let (mut cluster, _) = three_stripes();
        // Nobody vouches for regions 0 and 1 after the time `LEASE`; region 2 is
        // vouched for throughout.
        let silent = |cluster: &mut Cluster, at| {
            for name in ["a", "b"] {
                assert!(cluster.heartbeat_with(at, name, &[]));
            }
            assert!(cluster.heartbeat(at, "c"));
        };
        let asked = 2 * LEASE - 10;
        silent(&mut cluster, asked);
        cluster.merge(asked, 0, 1, None).unwrap();

        // Both have gone a lease without being vouched for, and stay where they are.
        silent(&mut cluster, 2 * LEASE + 5);
        assert_eq!(cluster.tick(2 * LEASE + 5), Changes::default());

        // The merge has had its lease: the region that was not released is taken
        // from its owner, which has failed it, and given to the first of the others.
        let late = asked + LEASE + 1;
        silent(&mut cluster, late);
        let ended = cluster.tick(late);
        let expected = Changes {
            reshaped: vec![merged(None, 0, 1, Err(Undone::NotReleased))],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert_eq!(
            cluster.coordinator.workers["b"].failed,
            Some(cluster.at(late))
        );
        // The survivor counts as vouched for at that moment, and so has another
        // lease, like the region its owner was given just now.
        silent(&mut cluster, late + LEASE);
        assert_eq!(cluster.tick(late + LEASE), Changes::default());
        silent(&mut cluster, late + LEASE + 1);
        assert_eq!(cluster.tick(late + LEASE + 1), changes(&["a", "c"], true));
        assert_eq!(regions_of(&cluster, "c"), [0, 1, 2]);
    }

    #[test]
    fn a_merge_that_the_worker_calls_off_ends_with_the_region_assigned_at_once() {
        let (mut cluster, list) = absorbing();
        let why = Off::Declined(Decline::Uncheckpointed {
            region: RegionId(1),
        });
        assert_eq!(
            cluster.absorb_ended(LEASE + 3, "a", 0, 1, Err(why)),
            reads()
        );
        // Nothing is given away on the worker's word.
        assert!(cluster.assignments("b").is_empty());
        assert_eq!(cluster.tick(LEASE + 4), Changes::default());

        // The list still has the region, opened with the epoch for the merge. It
        // goes to the worker with the fewest, with an epoch above that one.
        let list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
            ..list
        };
        let off = cluster.listed(LEASE + 5, &list);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Off(why)))],
            ..changes(&["b"], true)
        };
        assert_eq!(off, expected);
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);
        assert!(cluster.table().is_complete());
        assert!(cluster.table().absorbed.is_empty());
    }

    #[test]
    fn a_worker_that_says_done_against_the_list_is_not_believed() {
        let (mut cluster, list) = absorbing();
        cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(()));
        let off = cluster.listed(LEASE + 4, &list);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Contradicted))],
            ..changes(&["b"], true)
        };
        assert_eq!(off, expected);
        assert_eq!(regions_of(&cluster, "b"), [1]);
    }

    #[test]
    fn a_merge_waits_for_a_list_that_cannot_be_read_and_asks_again_at_every_tick() {
        let (mut cluster, list) = absorbing();
        assert_eq!(cluster.absorb_ended(LEASE + 3, "a", 0, 1, Ok(())), reads());
        // The reading is under way: a tick does not ask for another.
        assert_eq!(cluster.tick(LEASE + 4), Changes::default());
        // It failed. Nothing is decided, and the next tick asks again; once.
        assert_eq!(cluster.unlisted(LEASE + 5), Changes::default());
        assert_eq!(cluster.tick(LEASE + 6), reads());
        assert_eq!(cluster.tick(LEASE + 7), Changes::default());
        assert_eq!(cluster.unlisted(LEASE + 8), Changes::default());
        assert_eq!(cluster.tick(LEASE + 9), reads());
        assert!(cluster.assignments("b").is_empty());

        let done = cluster.listed(LEASE + 10, &after_the_merge(&list));
        assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
        assert_eq!(cluster.tick(LEASE + 11), Changes::default());
    }

    #[test]
    fn a_merge_that_is_not_done_within_the_lease_ends_by_what_the_list_says() {
        // The list has the region as absorbed: the merge was done, and only the
        // worker's word was lost.
        let (mut cluster, list) = absorbing();
        let late = 2 * LEASE + 2;
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
        assert_eq!(cluster.tick(late), reads());
        // Not before the list has been read is the region given to anyone.
        assert!(cluster.assignments("b").is_empty());
        assert_eq!(cluster.tick(late + 1), Changes::default());
        assert_eq!(
            cluster.merge(late + 1, 0, 2, None),
            Err(ReshapeRefusal::Reserved(RegionId(0)))
        );
        let done = cluster.listed(late + 2, &after_the_merge(&list));
        let expected = Changes {
            routing: true,
            reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
            ..Changes::default()
        };
        assert_eq!(done, expected);

        // The list has the region as living: it is assigned, above the epoch the
        // survivor's owner was to open it with, which fences an absorb on its way.
        let (mut cluster, list) = absorbing();
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(late), reads());
        let off = cluster.listed(late + 2, &list);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Overdue))],
            ..changes(&["b"], true)
        };
        assert_eq!(off, expected);
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);

        // The list cannot be read either: the region is assigned all the same.
        let (mut cluster, list) = absorbing();
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(late), reads());
        let off = cluster.unlisted(late + 2);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Unread))],
            ..changes(&["b"], true)
        };
        assert_eq!(off, expected);
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 5, 3)]);
        // It had been absorbed: the world store refuses the worker that was given
        // it, which says so, and the list that is read for that takes it away.
        assert_eq!(cluster.absorb_ended(late + 3, "b", 0, 1, Ok(())), reads());
        let gone = cluster.listed(late + 4, &after_the_merge(&list));
        assert_eq!(gone, changes(&["b"], true));
        assert!(cluster.assignments("b").is_empty());
        assert!(cluster.table().is_complete());
    }

    #[test]
    fn a_survivors_owner_that_registers_again_is_told_again_to_absorb() {
        let (mut cluster, _) = absorbing();
        let held = cluster.assignments("a");
        let again = cluster.register(LEASE + 3, "a", "a:25601", &held);
        let expected = Changes {
            orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
            ..Changes::default()
        };
        assert_eq!(again, expected);
        // Also when it comes back without the region, as a process that was replaced
        // does: it is to run it still, and to absorb.
        assert_eq!(cluster.register(LEASE + 4, "a", "a:25601", &[]), expected);
        // Another worker's registration tells nobody anything.
        let held = cluster.assignments("c");
        assert_eq!(
            cluster.register(LEASE + 5, "c", "c:25601", &held),
            Changes::default()
        );
        // Nor is it told again once it has said what came of it.
        cluster.absorb_ended(LEASE + 6, "a", 0, 1, Err(Off::Busy));
        let held = cluster.assignments("a");
        assert_eq!(
            cluster.register(LEASE + 7, "a", "a:25601", &held),
            Changes::default()
        );
    }

    #[test]
    fn the_owner_of_the_region_to_absorb_is_asked_again_and_without_it_has_released_it() {
        let (mut cluster, _) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, None).unwrap();
        // It lost its connection and may not have heard.
        let held = cluster.assignments("b");
        let again = cluster.register(LEASE + 2, "b", "b:25601", &held);
        assert_eq!(again, asks(&[order("b", 1, E + 2)]));
        // The survivor's owner is not told to prepare a second time.
        let held = cluster.assignments("a");
        assert_eq!(
            cluster.register(LEASE + 3, "a", "a:25601", &held),
            Changes::default()
        );
        // It comes back without the region: that is its word that it let go.
        let released = cluster.register(LEASE + 4, "b", "b:25601", &[]);
        let expected = Changes {
            orders: vec![absorb("a", 0, E + 1, 1, E + 4)],
            ..changes(&["b"], true)
        };
        assert_eq!(released, expected);
        // What it says afterwards changes nothing.
        assert_eq!(
            cluster.released(LEASE + 5, "b", 1, E + 2),
            Changes::default()
        );
        assert_eq!(
            cluster.released(LEASE + 5, "b", 1, E + 4),
            Changes::default()
        );
        assert!(cluster.assignments("b").is_empty());
    }

    #[test]
    fn one_worker_that_owns_both_regions_merges_them() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let list = stripes(2);
        cluster.listed(LEASE, &list);
        let asked = cluster.merge(LEASE + 1, 0, 1, Some(3)).unwrap();
        let expected = Changes {
            releases: vec![order("a", 1, E + 2)],
            orders: vec![prepare("a", 0, E + 1)],
            ..Changes::default()
        };
        assert_eq!(asked, expected);
        let released = cluster.released(LEASE + 2, "a", 1, E + 2);
        let expected = Changes {
            orders: vec![absorb("a", 0, E + 1, 1, E + 3)],
            ..changes(&["a"], true)
        };
        assert_eq!(released, expected);
        assert_eq!(regions_of(&cluster, "a"), [0]);
        // It registers again without the region it is to absorb, which it never
        // reports: it is told once, not twice.
        let held = cluster.assignments("a");
        assert_eq!(
            cluster.register(LEASE + 3, "a", "a:25601", &held).orders,
            [absorb("a", 0, E + 1, 1, E + 3)]
        );
        cluster.absorb_ended(LEASE + 4, "a", 0, 1, Ok(()));
        let list = RegionList {
            regions: vec![living(0, E + 1)],
            absorbed: vec![(RegionId(1), RegionId(0))],
            ..list
        };
        let done = cluster.listed(LEASE + 5, &list);
        assert_eq!(done.reshaped, [merged(Some(3), 0, 1, Ok(0))]);
        assert!(cluster.table().is_complete());
    }

    #[test]
    fn a_merge_ends_when_the_survivor_changes_hands_and_the_list_is_read_before_anything_is_assigned()
     {
        // At the second stage the survivor's worker falls silent. Its lease runs
        // out before the merge's does.
        let (mut cluster, list) = absorbing();
        let late = 2 * LEASE + 1;
        for name in ["b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        // The survivor goes to another worker at once; the other region waits for
        // the list.
        let lost = cluster.tick(late);
        let expected = Changes {
            read: true,
            ..changes(&["a", "b"], true)
        };
        assert_eq!(lost, expected);
        assert_eq!(cluster.assignments("b"), [assignment(0, E + 5, 3)]);
        assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
        // The list has the pair: the worker had done it before it died.
        let done = cluster.listed(late + 1, &after_the_merge(&list));
        assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
        assert!(cluster.table().is_complete());

        // The same, and the list has both regions: the other one is assigned too.
        let (mut cluster, list) = absorbing();
        for name in ["b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        cluster.tick(late);
        let off = cluster.listed(late + 1, &list);
        let disowned = Err(Undone::Disowned(RegionId(0)));
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, disowned)],
            ..changes(&["c"], true)
        };
        assert_eq!(off, expected);
        assert_eq!(regions_of(&cluster, "b"), [0]);
        assert_eq!(regions_of(&cluster, "c"), [1, 2]);

        // At the first stage: the region that was to be absorbed is taken from its
        // owner like one that was not released, and both are assigned.
        let (mut cluster, _) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        for name in ["b", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        let lost = cluster.tick(late);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, disowned)],
            ..changes(&["a", "b", "c"], true)
        };
        assert_eq!(lost, expected);
        assert!(cluster.table().is_complete());
        // Its owner was asked to let go and still had the time to: it has not failed
        // the region, and is given the survivor like any worker that runs nothing.
        assert_eq!(cluster.coordinator.workers["b"].failed, None);
        assert_eq!(regions_of(&cluster, "b"), [0]);
        assert_eq!(regions_of(&cluster, "c"), [1, 2]);
    }

    /// A worker that has failed a region is passed over for six leases. That is for
    /// the owner that did not release in time, and for no other.
    #[test]
    fn only_a_merge_whose_time_is_up_counts_against_the_owner_that_was_to_release() {
        // The time is up: the owner has failed the region.
        let (mut cluster, _) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, None).unwrap();
        beat(&mut cluster, 2 * LEASE);
        let late = 2 * LEASE + 2;
        let ended = cluster.tick(late);
        assert_eq!(
            ended.reshaped,
            [merged(None, 0, 1, Err(Undone::NotReleased))]
        );
        assert_eq!(
            cluster.coordinator.workers["b"].failed,
            Some(cluster.at(late))
        );
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert!(cluster.assignments("b").is_empty());

        // The survivor's owner says by itself that it lets go of the survivor, well
        // within the merge's time. The merge is off, and the other region is taken
        // from its owner all the same, which was told to release it and may be in the
        // middle of that; but that owner has not failed it, and is passed over for
        // nothing.
        let (mut cluster, _) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        let ended = cluster.released(LEASE + 2, "a", 0, E + 1);
        let disowned = Err(Undone::Disowned(RegionId(0)));
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, disowned)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(cluster.coordinator.workers["b"].failed, None);
        // The survivor went to the first of the others, and the other region to the
        // worker with the fewest then.
        assert_eq!(regions_of(&cluster, "b"), [0]);
        assert_eq!(regions_of(&cluster, "a"), [1]);
        assert_eq!(regions_of(&cluster, "c"), [2]);
        // What it says of the region it was to release changes nothing any more.
        assert_eq!(
            cluster.released(LEASE + 3, "b", 1, E + 2),
            Changes::default()
        );
    }

    #[test]
    fn a_merge_ends_when_the_owner_of_the_region_to_absorb_dies_before_it_has_let_go() {
        let (mut cluster, _) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        for name in ["a", "c"] {
            assert!(cluster.heartbeat(2 * LEASE, name));
        }
        // Its lease runs out a moment before the merge's would. The region is
        // assigned like any whose owner died.
        let lost = cluster.tick(2 * LEASE + 1);
        let disowned = Err(Undone::Disowned(RegionId(1)));
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, disowned)],
            ..changes(&["a", "b"], true)
        };
        assert_eq!(lost, expected);
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        assert!(cluster.coordinator.merges.is_empty());
    }

    #[test]
    fn a_region_whose_epoch_the_store_refused_for_a_merge_is_assigned_above_it() {
        let (mut cluster, list) = absorbing();
        // Somebody has opened the region since, with an epoch nobody here issued.
        let refused = cluster.epoch_refused(LEASE + 3, "a", 1, E + 50);
        assert_eq!(refused, Changes::default());
        // An order that is given again still names the epoch the merge was begun
        // with: the survivor's worker is not to take the region from that somebody.
        let held = cluster.assignments("a");
        assert_eq!(
            cluster.register(LEASE + 3, "a", "a:25601", &held).orders,
            [absorb("a", 0, E + 1, 1, E + 4)]
        );
        assert_eq!(
            cluster.absorb_ended(LEASE + 4, "a", 0, 1, Err(Off::Refused)),
            reads()
        );
        let list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 50), living(2, E + 3)],
            ..list
        };
        let off = cluster.listed(LEASE + 5, &list);
        assert_eq!(
            off.reshaped,
            [merged(Some(7), 0, 1, Err(Undone::Off(Off::Refused)))]
        );
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 51, 3)]);
    }

    #[test]
    fn a_merge_whose_region_went_elsewhere_is_not_said_to_be_done() {
        let (mut cluster, list) = absorbing();
        // Somebody else had region 2 absorb the region meanwhile.
        let elsewhere = RegionList {
            regions: vec![living(0, E + 1), living(2, E + 3)],
            absorbed: vec![(RegionId(1), RegionId(2))],
            ..list
        };
        let gone = cluster.listed(LEASE + 3, &elsewhere);
        let expected = Changes {
            routing: true,
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::Gone(RegionId(1))))],
            ..Changes::default()
        };
        assert_eq!(gone, expected);
        assert!(cluster.table().is_complete());
    }

    #[test]
    fn nothing_is_merged_or_split_once_the_epochs_have_run_out() {
        // A worker reports a region with the last epoch there is.
        let last = part(9, u64::MAX);
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        cluster.listed(LEASE, &stripes(2));
        cluster.register(LEASE, "b", "b:25601", &[last]);
        assert_eq!(
            cluster.merge(LEASE, 0, 1, None),
            Err(ReshapeRefusal::NoEpoch)
        );
        assert_eq!(
            cluster.split(LEASE, 1, &CHUNKS, None),
            Err(ReshapeRefusal::NoEpoch)
        );

        // They run out between the asking and the release: the region that was let
        // go of cannot be opened anew, by anyone.
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        cluster.listed(LEASE, &stripes(2));
        cluster.merge(LEASE, 0, 1, Some(7)).unwrap();
        cluster.register(LEASE, "b", "b:25601", &[last]);
        let ended = cluster.released(LEASE, "a", 1, E + 2);
        let expected = Changes {
            reshaped: vec![merged(Some(7), 0, 1, Err(Undone::NoEpoch))],
            ..changes(&["a"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(cluster.coordinator.waiting(), [RegionId(1)]);
        assert_eq!(cluster.tick(LEASE), Changes::default());
    }

    /// The list is read for other reasons too, such as a worker that registers.
    #[test]
    fn a_reading_while_a_merge_is_under_way_and_not_done_decides_nothing() {
        let (mut cluster, list) = three_stripes();
        cluster.merge(LEASE + 1, 0, 1, Some(7)).unwrap();
        assert_eq!(cluster.listed(LEASE + 2, &list), Changes::default());
        cluster.released(LEASE + 3, "b", 1, E + 2);
        let table = cluster.table();
        // The survivor's worker has opened the region, and nothing more yet.
        let opened = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
            ..list.clone()
        };
        for at in [LEASE + 4, LEASE + 5] {
            assert_eq!(cluster.listed(at, &opened), Changes::default());
            assert_eq!(cluster.unlisted(at), Changes::default());
        }
        assert_eq!(cluster.table(), table);
        assert_eq!(cluster.tick(LEASE + 6), Changes::default());
        // One that shows it done ends it, without the worker's word.
        let done = cluster.listed(LEASE + 7, &after_the_merge(&list));
        assert_eq!(done.reshaped, [merged(Some(7), 0, 1, Ok(0))]);
        // Its word then only has the list read once more.
        assert_eq!(cluster.absorb_ended(LEASE + 8, "a", 0, 1, Ok(())), reads());
        assert_eq!(
            cluster.listed(LEASE + 9, &after_the_merge(&list)),
            Changes::default()
        );
    }

    #[test]
    fn a_reading_that_takes_the_survivor_away_ends_the_merge() {
        // Somebody else has merged the survivor into a third region meanwhile.
        let gone = |list: &RegionList| RegionList {
            regions: vec![living(0, E + 1), living(2, E + 3)],
            absorbed: vec![(RegionId(1), RegionId(0))],
            ..list.clone()
        };
        let taken_away = Err(Undone::Gone(RegionId(1)));

        // At the first stage the other region is taken from its owner all the same,
        // and goes to the worker that has just lost the survivor.
        let (mut cluster, list) = three_stripes();
        cluster.merge(LEASE + 1, 1, 2, Some(7)).unwrap();
        let ended = cluster.listed(LEASE + 2, &gone(&list));
        let expected = Changes {
            reshaped: vec![merged(Some(7), 1, 2, taken_away)],
            ..changes(&["b", "c"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(regions_of(&cluster, "b"), [2]);
        assert!(cluster.table().is_complete());
        // Its owner has not failed it: the survivor's side ended the merge.
        assert_eq!(cluster.coordinator.workers["c"].failed, None);

        // At the second it has no owner, and is assigned by the reading that ended
        // the merge.
        let (mut cluster, list) = three_stripes();
        cluster.merge(LEASE + 1, 1, 2, Some(7)).unwrap();
        cluster.released(LEASE + 2, "c", 2, E + 3);
        let ended = cluster.listed(LEASE + 3, &gone(&list));
        let expected = Changes {
            reshaped: vec![merged(Some(7), 1, 2, taken_away)],
            ..changes(&["b"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(cluster.assignments("b"), [assignment(2, E + 5, 3)]);
        assert!(cluster.table().is_complete());
    }

    /// What the worker `name` runs of a region that was split off another: it has the
    /// epoch its worker was told, and no entity ids.
    fn part(region: u32, epoch: u64) -> Assignment {
        Assignment {
            region: RegionId(region),
            epoch,
            entity_ids: NO_ENTITY_IDS,
        }
    }

    /// The list of [`three_stripes`] after region 2 was split, the new region having
    /// been opened with the epoch `E + 4`.
    fn after_the_split(list: &RegionList) -> RegionList {
        let mut regions = list.regions.clone();
        regions.push(living(3, E + 4));
        RegionList {
            regions,
            next: RegionId(4),
            ..list.clone()
        }
    }

    #[test]
    fn a_split_names_the_next_region_of_the_list_and_the_new_region_is_its_workers() {
        let (mut cluster, list) = three_stripes();
        let asked = cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
        let expected = Changes {
            orders: vec![split_off("c", 2, E + 3, E + 4, 3)],
            ..Changes::default()
        };
        assert_eq!(asked, expected);
        // The region is reserved, and its owner is not asked a second time.
        assert_eq!(
            cluster.split(LEASE + 1, 2, &CHUNKS, None),
            Err(ReshapeRefusal::Reserved(RegionId(2)))
        );
        assert_eq!(
            cluster.move_region(LEASE + 1, 2, None, 5),
            Err(MoveRefusal::Reserved(RegionId(2)))
        );
        let held = cluster.assignments("c");
        assert_eq!(
            cluster.register(LEASE + 1, "c", "c:25601", &held),
            Changes::default()
        );

        // The worker's word makes the new region its, with the epoch it was told.
        let done = cluster.split_ended(LEASE + 2, "c", 2, E + 4, Ok(3));
        let expected = Changes {
            read: true,
            reshaped: vec![was_split(Some(7), 2, Ok(3))],
            ..changes(&["c"], true)
        };
        assert_eq!(done, expected);
        assert_eq!(
            cluster.assignments("c"),
            [assignment(2, E + 3, 2), part(3, E + 4)]
        );
        let table = cluster.table();
        assert_eq!(table.route(RegionId(3)), Some(&route(3, E + 4, "c:25601")));
        assert!(table.is_complete());
        // The list that is read for it has nothing new.
        assert_eq!(
            cluster.listed(LEASE + 3, &after_the_split(&list)),
            Changes::default()
        );
        // Said again, as a worker does until its orders name the region: the list is
        // read, and that is all.
        assert_eq!(
            cluster.split_ended(LEASE + 4, "c", 2, E + 4, Ok(3)),
            reads()
        );

        // The new region is kept by its worker's heartbeats like any other, and lost
        // without them.
        beat(&mut cluster, 2 * LEASE);
        assert_eq!(cluster.tick(2 * LEASE + 3), Changes::default());
        let vouches = [(RegionId(2), Vouch::Committed)];
        for at in [3 * LEASE, 3 * LEASE + 1] {
            for name in ["a", "b"] {
                assert!(cluster.heartbeat(at, name));
            }
            assert!(cluster.heartbeat_with(at, "c", &vouches));
        }
        assert_eq!(cluster.tick(3 * LEASE), Changes::default());
        assert_eq!(cluster.tick(3 * LEASE + 1), changes(&["a", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 3]);
    }

    #[test]
    fn a_split_that_came_to_nothing_leaves_the_region_as_it_was() {
        let (mut cluster, list) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
        // An answer for another order, or from another worker, is not this one's.
        assert_eq!(
            cluster.split_ended(LEASE + 2, "c", 2, E + 9, Err(Off::Nobody)),
            reads()
        );
        assert_eq!(
            cluster.split_ended(LEASE + 2, "b", 2, E + 4, Err(Off::Nobody)),
            reads()
        );
        let off = cluster.split_ended(LEASE + 2, "c", 2, E + 4, Err(Off::Nobody));
        let expected = Changes {
            read: true,
            reshaped: vec![was_split(Some(7), 2, Err(Undone::Off(Off::Nobody)))],
            ..Changes::default()
        };
        assert_eq!(off, expected);
        assert_eq!(cluster.listed(LEASE + 3, &list), Changes::default());
        // It can be asked again, and gets another epoch for the new region.
        let asked = cluster.split(LEASE + 4, 2, &CHUNKS, None).unwrap();
        assert_eq!(asked.orders, [split_off("c", 2, E + 3, E + 5, 3)]);
    }

    #[test]
    fn a_split_without_the_workers_word_is_overdue_after_a_lease_whatever_the_list_shows() {
        let late = 2 * LEASE + 2;
        let lapsing = || {
            let (mut cluster, list) = three_stripes();
            cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
            beat(&mut cluster, 2 * LEASE);
            assert_eq!(cluster.tick(2 * LEASE + 1), Changes::default());
            // Whoever asked is told at once, and the list is asked for.
            let expected = Changes {
                read: true,
                reshaped: vec![was_split(Some(7), 2, Err(Undone::Overdue))],
                ..Changes::default()
            };
            assert_eq!(cluster.tick(late), expected);
            // The region is free again.
            assert!(cluster.coordinator.splits.is_empty());
            (cluster, list)
        };

        // The list has a region with the id that was ordered, which nobody is known
        // to run: it is assigned. That it is this split's nobody can tell, as the
        // store may have made this one's under the next id; nobody is told anything.
        let (mut cluster, list) = lapsing();
        let found = cluster.listed(late + 1, &after_the_split(&list));
        assert_eq!(found, changes(&["a"], true));
        assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
        assert!(cluster.table().is_complete());
        assert_eq!(cluster.tick(late + 2), Changes::default());

        // The list has none.
        let (mut cluster, list) = lapsing();
        assert_eq!(cluster.listed(late + 1, &list), Changes::default());
        assert_eq!(cluster.tick(late + 2), Changes::default());

        // The list cannot be read: it is asked for at every tick until it can be, as
        // there may be a region that nobody runs.
        let (mut cluster, list) = lapsing();
        assert_eq!(cluster.unlisted(late + 1), Changes::default());
        assert_eq!(cluster.tick(late + 2), reads());
        assert_eq!(cluster.tick(late + 3), Changes::default());
        assert_eq!(cluster.unlisted(late + 4), Changes::default());
        assert_eq!(cluster.tick(late + 5), reads());
        let found = cluster.listed(late + 6, &after_the_split(&list));
        assert_eq!(found, changes(&["a"], true));
        assert_eq!(cluster.tick(late + 7), Changes::default());
    }

    #[test]
    fn a_split_ends_when_its_region_changes_hands() {
        let (mut cluster, list) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
        // The world store refuses the owner's epoch: the worker has dropped the
        // region, and is given it anew, with another epoch. The split was of the
        // one it had.
        // Whoever asked is told that nobody knows what came of it.
        let refused = cluster.epoch_refused(LEASE + 2, "c", 2, E + 20);
        let disowned = Err(Undone::Disowned(RegionId(2)));
        let expected = Changes {
            read: true,
            reshaped: vec![was_split(Some(7), 2, disowned)],
            ..changes(&["c"], true)
        };
        assert_eq!(refused, expected);
        assert_eq!(cluster.assignments("c"), [assignment(2, E + 21, 3)]);
        // Its answer comes when the reservation is over. It is taken as a report:
        // nobody else is known to run the new region.
        let late = cluster.split_ended(LEASE + 3, "c", 2, E + 4, Ok(3));
        let expected = Changes {
            read: true,
            ..changes(&["c"], true)
        };
        assert_eq!(late, expected);
        // The list has nothing to add to that, and says nothing to whoever asked.
        let found = cluster.listed(LEASE + 4, &after_the_split(&list));
        assert_eq!(found, Changes::default());
        assert_eq!(cluster.assignments("c")[1], part(3, E + 4));

        // When a reading of the list takes the region away, it is gone, not disowned.
        let (mut cluster, list) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, Some(7)).unwrap();
        let gone = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 2)],
            absorbed: vec![(RegionId(2), RegionId(1))],
            ..list
        };
        let ended = cluster.listed(LEASE + 2, &gone);
        let expected = Changes {
            reshaped: vec![was_split(Some(7), 2, Err(Undone::Gone(RegionId(2))))],
            ..changes(&["c"], true)
        };
        assert_eq!(ended, expected);
        assert_eq!(cluster.tick(LEASE + 3), Changes::default());
    }

    #[test]
    fn a_reading_while_a_split_is_reserved_leaves_the_new_region_to_the_worker_that_made_it() {
        let (mut cluster, list) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
        // The store has the record, and the worker's word is still on its way.
        let split_list = after_the_split(&list);
        assert_eq!(cluster.listed(LEASE + 2, &split_list), Changes::default());
        assert!(cluster.table().route(RegionId(3)).is_none());
        assert!(cluster.table().is_complete());
        assert_eq!(cluster.tick(LEASE + 3), Changes::default());

        let done = cluster.split_ended(LEASE + 4, "c", 2, E + 4, Ok(3));
        assert_eq!(done.workers, ["c"]);
        assert_eq!(cluster.assignments("c")[1], part(3, E + 4));

        // Had the reservation run out instead, the same reading would have added the
        // region, without an owner, and it would have been assigned.
        let (mut cluster, _) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
        beat(&mut cluster, 2 * LEASE);
        let lapsed = cluster.tick(2 * LEASE + 2);
        assert_eq!(lapsed.reshaped, [was_split(None, 2, Err(Undone::Overdue))]);
        assert!(lapsed.read);
        let found = cluster.listed(2 * LEASE + 3, &split_list);
        assert_eq!(found, changes(&["a"], true));
        assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
    }

    #[test]
    fn word_of_a_split_that_nobody_reserved_is_taken_as_a_report_of_the_new_region() {
        let (mut cluster, _) = three_stripes();
        let said = cluster.split_ended(LEASE + 1, "c", 2, E + 30, Ok(5));
        let expected = Changes {
            read: true,
            ..changes(&["c"], true)
        };
        assert_eq!(said, expected);
        assert_eq!(cluster.assignments("c")[1], part(5, E + 30));
        // No epoch is issued at or below the one it has.
        assert_eq!(
            cluster.split(LEASE + 1, 0, &CHUNKS, None).unwrap().orders,
            [split_off("a", 0, E + 1, E + 31, 3)]
        );

        // Another worker that says the same of that region is not believed, nor is
        // one that says it of a region somebody runs with a higher epoch.
        assert_eq!(
            cluster.split_ended(LEASE + 2, "b", 1, E + 40, Ok(5)),
            reads()
        );
        assert_eq!(
            cluster.split_ended(LEASE + 2, "c", 2, E + 2, Ok(5)),
            reads()
        );
        assert_eq!(cluster.assignments("c")[1], part(5, E + 30));
        // That nothing came of a split nobody knows of only has the list read, and
        // so has whatever a worker says that is not registered.
        assert_eq!(
            cluster.split_ended(LEASE + 3, "b", 1, E + 41, Err(Off::Busy)),
            reads()
        );
        assert_eq!(
            cluster.split_ended(LEASE + 3, "z", 1, E + 41, Ok(9)),
            reads()
        );
        assert_eq!(cluster.absorb_ended(LEASE + 3, "z", 1, 2, Ok(())), reads());
    }

    #[test]
    fn the_list_adds_the_regions_nobody_runs_and_takes_away_those_that_are_no_more() {
        let (mut cluster, list) = three_stripes();
        let version = cluster.table().version;
        // A region that was split off, whose worker died before it said so.
        let found = cluster.listed(LEASE + 1, &after_the_split(&list));
        assert_eq!(found, changes(&["a"], true));
        assert_eq!(cluster.assignments("a")[1], assignment(3, E + 5, 3));
        assert_eq!(cluster.table().version, version + 1);

        // Region 2 went into region 1: its owner loses it, and the table has the pair.
        let merged_list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 2), living(3, E + 5)],
            absorbed: vec![(RegionId(2), RegionId(1))],
            next: RegionId(4),
            ..list
        };
        let gone = cluster.listed(LEASE + 2, &merged_list);
        assert_eq!(gone, changes(&["c"], true));
        assert!(cluster.assignments("c").is_empty());
        let table = cluster.table();
        assert_eq!(table.absorbed, [(RegionId(2), RegionId(1))]);
        assert_eq!(table.version, version + 2);
        assert!(table.is_complete());
        // The same list again changes nothing, and the table keeps its version.
        assert_eq!(cluster.listed(LEASE + 3, &merged_list), Changes::default());
        assert_eq!(cluster.table(), table);

        // A reading that failed changes nothing either: the table keeps the pairs.
        assert_eq!(cluster.unlisted(LEASE + 4), Changes::default());
        assert_eq!(cluster.table(), table);

        // A region that is being moved and is no more: whoever asked is told.
        cluster.move_region(LEASE + 5, 3, Some("b"), 9).unwrap();
        let merged_list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 2)],
            absorbed: vec![(RegionId(2), RegionId(1)), (RegionId(3), RegionId(0))],
            ..merged_list
        };
        let gone = cluster.listed(LEASE + 6, &merged_list);
        let expected = Changes {
            moves: vec![outcome(9, 3, None, false)],
            ..changes(&["a"], true)
        };
        assert_eq!(gone, expected);
    }

    #[test]
    fn a_new_coordinator_assigns_what_the_list_shows_when_its_grace_period_is_over() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        // Three regions more than the layout has; one of them was opened with an
        // epoch far above the coordinator's.
        let list = RegionList {
            regions: vec![living(0, 0), living(1, 0), living(4, E + 70), living(6, 3)],
            absorbed: vec![(RegionId(2), RegionId(0)), (RegionId(3), RegionId(5))],
            next: RegionId(7),
            ..stripes(2)
        };
        assert_eq!(cluster.listed(1, &list), changes(&[], true));
        let table = cluster.table();
        assert_eq!(table.waiting, 4);
        assert_eq!(table.home, Some(RegionId(0)));
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());

        // A worker that ran one of them under the coordinator before reports it.
        let held = part(6, 3);
        assert_eq!(
            cluster.register(LEASE - 1, "b", "b:25601", &[held]),
            changes(&["b"], true)
        );
        // One that reports a region with an epoch below the one the store has was
        // replaced, whoever replaced it.
        assert_eq!(
            cluster.register(LEASE - 1, "c", "c:25601", &[part(4, E + 69)]),
            Changes::default()
        );

        // The lowest first, each to the worker with the fewest then.
        assert_eq!(cluster.tick(LEASE), changes(&["a", "c"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 4]);
        assert_eq!(regions_of(&cluster, "b"), [6]);
        assert_eq!(regions_of(&cluster, "c"), [1]);
        // Every epoch it issues is above the highest of the list.
        let epochs: Vec<u64> = cluster.table().routes.iter().map(|r| r.epoch).collect();
        assert_eq!(epochs, [E + 71, E + 72, E + 73, 3]);
        assert!(cluster.table().is_complete());
    }

    #[test]
    fn a_new_coordinator_finds_a_merge_that_was_under_way_as_the_list_and_the_workers_have_it() {
        // Region 1 was released for region 0 to absorb it, and the survivor's worker
        // has opened it with the epoch `E + 4`. Then the coordinator was replaced.
        let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
        let list = RegionList {
            regions: vec![living(0, E + 1), living(1, E + 4), living(2, E + 3)],
            ..stripes(3)
        };
        cluster.register(1, "a", "a:25601", &[assignment(0, E + 1, 0)]);
        cluster.register(1, "c", "c:25601", &[assignment(2, E + 3, 2)]);
        cluster.register(1, "b", "b:25601", &[]);
        assert_eq!(cluster.listed(2, &list), changes(&[], true));
        // The worker that released it says so again only to a coordinator whose
        // orders name the region, which this one's never did. If its word was still
        // on its way, it is of an epoch the region has left behind.
        assert_eq!(cluster.released(3, "b", 1, E + 2), Changes::default());
        // So the region waits out the grace period like any without an owner.
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert!(!cluster.table().is_complete());
        beat(&mut cluster, LEASE);
        assert_eq!(cluster.tick(LEASE), changes(&["b"], true));
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 101, 1)]);

        // Had the survivor's worker absorbed it meanwhile, its word, which is for a
        // merge this coordinator has no note of, would have the list read, and the
        // region would be gone.
        assert_eq!(cluster.absorb_ended(LEASE + 1, "a", 0, 1, Ok(())), reads());
        let gone = cluster.listed(LEASE + 2, &after_the_merge(&list));
        assert_eq!(gone, changes(&["b"], true));
        assert!(cluster.table().is_complete());

        // A coordinator that has not read the list yet takes the word of the worker
        // that released the region: it is free, and assigned at once, to that very
        // worker if it has the fewest. That fences the absorb if it is not done yet.
        let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
        cluster.register(1, "a", "a:25601", &[assignment(0, E + 1, 0)]);
        cluster.register(1, "b", "b:25601", &[]);
        assert_eq!(cluster.released(3, "b", 1, E + 2), changes(&["b"], true));
        assert_eq!(cluster.assignments("b"), [assignment(1, E + 101, 1)]);
    }

    #[test]
    fn a_new_coordinator_learns_of_a_region_that_was_split_off_from_its_worker_or_from_the_list() {
        let list = after_the_split(&RegionList {
            regions: vec![living(0, E + 1), living(1, E + 2), living(2, E + 3)],
            ..stripes(3)
        });
        // Its worker lives and reports it.
        let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
        let held = [assignment(2, E + 3, 2), part(3, E + 4)];
        assert_eq!(
            cluster.register(1, "c", "c:25601", &held),
            changes(&["c"], true)
        );
        assert_eq!(cluster.assignments("c"), held);
        assert_eq!(cluster.listed(2, &list), changes(&[], true));
        assert_eq!(cluster.assignments("c"), held);

        // Its worker died: the list has the region, and it is assigned when the
        // grace period is over.
        let mut cluster = Cluster::with_first_epoch(&[0, 4], E + 100);
        cluster.register(1, "a", "a:25601", &[]);
        assert_eq!(cluster.listed(2, &list), changes(&[], true));
        assert_eq!(cluster.table().waiting, 4);
        assert_eq!(cluster.tick(LEASE - 1), Changes::default());
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1, 2, 3]);
    }

    #[test]
    fn nothing_is_evened_out_during_a_split_or_within_a_lease_of_its_end() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        cluster.listed(LEASE, &stripes(2));
        assert!(cluster.heartbeat(LEASE, "a"));
        cluster.split(LEASE + 1, 1, &CHUNKS, None).unwrap();
        // A worker that runs nothing, beside one that runs two regions.
        cluster.register(LEASE + 2, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE + 3), Changes::default());

        let ended = LEASE + 4;
        cluster.split_ended(ended, "a", 1, E + 3, Ok(2));
        assert_eq!(regions_of(&cluster, "a"), [0, 1, 2]);
        for quarter in 0..4 {
            let now = ended + quarter * LEASE / 4;
            for name in ["a", "b"] {
                assert!(cluster.heartbeat(now, name));
            }
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        assert_eq!(cluster.tick(ended + LEASE - 1), Changes::default());
        // A lease after it ended, the new region is the first to go.
        assert_eq!(cluster.tick(ended + LEASE), asks(&[order("a", 2, E + 3)]));
    }

    #[test]
    fn nothing_is_evened_out_during_a_merge_or_within_a_lease_of_its_end() {
        let mut cluster = Cluster::new(&[-4, 0, 4]);
        for name in ["a", "b"] {
            cluster.register(0, name, &format!("{name}:25601"), &[]);
        }
        assert_eq!(cluster.tick(LEASE), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 2]);
        let list = stripes(4);
        cluster.listed(LEASE, &list);
        cluster.merge(LEASE + 1, 0, 1, None).unwrap();
        cluster.register(LEASE + 1, "c", "c:25601", &[]);
        beat(&mut cluster, LEASE + 1);
        // Two regions on one worker and none on another, and nothing is moved.
        assert_eq!(cluster.tick(LEASE + 2), Changes::default());
        cluster.released(LEASE + 3, "b", 1, E + 2);
        assert_eq!(cluster.tick(LEASE + 4), Changes::default());
        cluster.absorb_ended(LEASE + 5, "a", 0, 1, Ok(()));
        assert_eq!(cluster.tick(LEASE + 6), Changes::default());
        let ended = LEASE + 7;
        let list = RegionList {
            regions: vec![living(0, 0), living(2, 0), living(3, 0)],
            absorbed: vec![(RegionId(1), RegionId(0))],
            ..list
        };
        assert_eq!(cluster.listed(ended, &list).reshaped.len(), 1);
        for quarter in 0..4 {
            let now = ended + quarter * LEASE / 4;
            beat(&mut cluster, now);
            assert_eq!(cluster.tick(now), Changes::default(), "at {now}");
        }
        beat(&mut cluster, ended + LEASE);
        assert_eq!(cluster.tick(ended + LEASE), asks(&[order("a", 2, E + 3)]));
    }

    #[test]
    fn a_leaving_workers_region_is_released_when_the_merge_it_is_part_of_has_ended() {
        let (mut cluster, list) = absorbing();
        // The survivor's owner is told to stop in the middle of it.
        assert_eq!(cluster.leaving(LEASE + 3, "a"), Changes::default());
        assert_eq!(cluster.tick(LEASE + 4), Changes::default());
        cluster.absorb_ended(LEASE + 5, "a", 0, 1, Ok(()));
        let done = cluster.listed(LEASE + 6, &after_the_merge(&list));
        let expected = Changes {
            routing: true,
            releases: vec![order("a", 0, E + 1)],
            reshaped: vec![merged(Some(7), 0, 1, Ok(0))],
            ..Changes::default()
        };
        assert_eq!(done, expected);
        // For the worker that has run nothing since it let go of the other region.
        assert_eq!(targets(&cluster), [(0, "b")]);

        // The same for a region that is split.
        let (mut cluster, _) = three_stripes();
        cluster.split(LEASE + 1, 2, &CHUNKS, None).unwrap();
        assert_eq!(cluster.leaving(LEASE + 2, "c"), Changes::default());
        let done = cluster.split_ended(LEASE + 3, "c", 2, E + 4, Ok(3));
        assert_eq!(done.releases, [order("c", 2, E + 3), order("c", 3, E + 4)]);
    }

    /// The world store of the randomised test below, as far as the coordinator and
    /// the workers of that test can tell: the regions there are, each with the highest
    /// epoch it was opened with, the regions that were absorbed, and the next id.
    struct Store {
        living: BTreeMap<RegionId, u64>,
        absorbed: Vec<(RegionId, RegionId)>,
        next: u32,
    }

    /// Why the store does not let a worker open a region.
    enum Shut {
        /// The region went into this one.
        Absorbed(RegionId),
        /// It has seen an owner with this epoch, which is higher.
        Seen(u64),
    }

    impl Store {
        fn list(&self) -> RegionList {
            let regions = self.living.iter();
            RegionList {
                home: RegionId(0),
                regions: regions.map(|(id, epoch)| living(id.0, *epoch)).collect(),
                absorbed: self.absorbed.clone(),
                next: RegionId(self.next),
            }
        }

        /// A worker opens `region` with `epoch`, or goes on running it.
        fn open(&mut self, region: RegionId, epoch: u64) -> Result<(), Shut> {
            if let Some((_, into)) = self.absorbed.iter().find(|(gone, _)| *gone == region) {
                return Err(Shut::Absorbed(*into));
            }
            let seen = self
                .living
                .get_mut(&region)
                .expect("no worker makes a region up");
            if *seen > epoch {
                return Err(Shut::Seen(*seen));
            }
            *seen = epoch;
            Ok(())
        }
    }

    /// What a worker of the randomised test was told to do and has not done yet.
    #[derive(Debug)]
    enum Told {
        Release(RegionId, u64),
        Reshape(Order),
    }

    /// What a worker of the randomised test tells the coordinator on finding the
    /// store shut, or of a merge or a split.
    enum Says {
        EpochRefused(RegionId, u64),
        AbsorbEnded(RegionId, RegionId, Result<(), Off>),
        SplitEnded(RegionId, u64, Result<RegionId, Off>),
    }

    /// The workers of the randomised test.
    const CREW: [&str; 4] = ["a", "b", "c", "d"];

    /// A coordinator, a world store and four workers that do what workers do
    /// (ADR-0014, section 4), as far as the coordinator can tell: they open what they
    /// are given at the store, release, absorb and split when told, run a region they
    /// split off from memory, and say what came of it.
    struct World {
        cluster: Cluster,
        store: Store,
        random: Generator,
        now: u64,
        /// Whether messages get lost and workers call things off; not while the
        /// world is left to settle.
        lossy: bool,
        /// The workers whose processes run and have a connection.
        alive: BTreeSet<&'static str>,
        /// What each worker runs: what it has open at the store.
        running: BTreeMap<&'static str, Vec<Assignment>>,
        /// The regions each worker split off that its orders have not named yet,
        /// each with the region it was split off and the epoch it was to have.
        parts: BTreeMap<&'static str, Vec<(RegionId, u64, RegionId)>>,
        inbox: BTreeMap<&'static str, Vec<Told>>,
        /// The list as it was read for the coordinator, or that it could not be,
        /// until it is handed in.
        reading: Option<Option<RegionList>>,
        /// Who waits to hear of a merge or a split.
        askers: BTreeMap<u64, Asked>,
        /// The highest epoch any coordinator issued.
        highest: u64,
        /// How many merges and splits were done, and how the others ended.
        merged: usize,
        split: usize,
        undone: BTreeMap<String, usize>,
    }

    impl World {
        fn new(seed: u64) -> Self {
            let store = Store {
                living: (0..3).map(|region| (RegionId(region), 0)).collect(),
                absorbed: Vec::new(),
                next: 3,
            };
            Self {
                cluster: Cluster::new(&[0, 4]),
                store,
                random: Generator(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
                now: 0,
                lossy: true,
                alive: BTreeSet::new(),
                running: CREW.iter().map(|name| (*name, Vec::new())).collect(),
                parts: CREW.iter().map(|name| (*name, Vec::new())).collect(),
                inbox: CREW.iter().map(|name| (*name, Vec::new())).collect(),
                reading: None,
                askers: BTreeMap::new(),
                highest: FIRST_EPOCH,
                merged: 0,
                split: 0,
                undone: BTreeMap::new(),
            }
        }

        /// Whether something gets lost, or is called off, this once in `times`.
        fn lost(&mut self, times: u64) -> bool {
            self.lossy && self.random.once_in(times)
        }

        /// The list is read for the coordinator; now and then it cannot be.
        fn read(&mut self) -> Option<RegionList> {
            (!self.lost(10)).then(|| self.store.list())
        }

        /// Deals with what a call into the coordinator said, and with what the
        /// workers say in turn when they hear of it.
        fn said(&mut self, changes: Changes) {
            let mut says = Vec::new();
            self.take(&changes, &mut says);
            while let Some((name, say)) = says.pop() {
                let changes = self.tell(name, say);
                self.take(&changes, &mut says);
            }
        }

        /// The worker `name` tells the coordinator something.
        fn tell(&mut self, name: &'static str, say: Says) -> Changes {
            let now = self.now;
            match say {
                Says::EpochRefused(region, seen) => {
                    self.cluster.epoch_refused(now, name, region.0, seen)
                }
                Says::AbsorbEnded(region, absorbed, outcome) => self
                    .cluster
                    .absorb_ended(now, name, region.0, absorbed.0, outcome),
                Says::SplitEnded(region, as_epoch, outcome) => {
                    let outcome = outcome.map(|part| part.0);
                    self.cluster
                        .split_ended(now, name, region.0, as_epoch, outcome)
                }
            }
        }

        /// The same, with what comes of it.
        fn say(&mut self, name: &'static str, say: Says) {
            let changes = self.tell(name, say);
            self.said(changes);
        }

        /// What [`World::said`] does with one call.
        fn take(&mut self, changes: &Changes, says: &mut Vec<(&'static str, Says)>) {
            for held in self.cluster.view().assignments.values().flatten() {
                self.highest = self.highest.max(held.epoch);
            }
            for told in &changes.orders {
                if let Order::Absorb { as_epoch, .. } | Order::SplitOff { as_epoch, .. } =
                    &told.order
                {
                    self.highest = self.highest.max(*as_epoch);
                }
            }
            for reshaped in &changes.reshaped {
                self.ended(reshaped);
            }
            for name in CREW {
                // The coordinator has closed the connection of a worker that left.
                if changes.gone.iter().any(|gone| gone == name) {
                    self.stop(name);
                }
                let heard = changes.workers.iter().any(|worker| worker == name);
                if heard && self.alive.contains(name) && !self.lost(8) {
                    self.hear(name, says);
                }
            }
            let releases = changes.releases.iter().map(|told| {
                let release = Told::Release(told.region, told.epoch);
                (told.worker.as_str(), release)
            });
            let reshapes = changes.orders.iter();
            let reshapes =
                reshapes.map(|told| (told.worker.as_str(), Told::Reshape(told.order.clone())));
            for (worker, told) in releases.chain(reshapes).collect::<Vec<_>>() {
                let name = CREW.into_iter().find(|name| *name == worker);
                let name = name.expect("only the crew is told anything");
                if self.alive.contains(name) && !self.lost(8) {
                    self.inbox.get_mut(name).unwrap().push(told);
                }
            }
            if changes.read {
                // A reading that was under way was asked for too early.
                self.reading = Some(self.read());
            }
        }

        /// Whoever asked for a merge or a split is told once, what was asked, and
        /// nothing is said to be done that the store does not have.
        fn ended(&mut self, reshaped: &Reshaped) {
            if let Some(asker) = reshaped.asker {
                let asked = self.askers.remove(&asker);
                assert_eq!(asked, Some(reshaped.asked), "{reshaped:?}");
            }
            match (reshaped.asked, reshaped.outcome) {
                (Asked::Merge { survivor, absorbed }, Ok(region)) => {
                    assert_eq!(region, survivor);
                    let pair = (absorbed, survivor);
                    assert!(self.store.absorbed.contains(&pair), "{reshaped:?}");
                    self.merged += 1;
                }
                (Asked::Split { .. }, Ok(part)) => {
                    let mut gone = self.store.absorbed.iter().map(|(gone, _)| gone);
                    let made =
                        self.store.living.contains_key(&part) || gone.any(|gone| *gone == part);
                    assert!(made, "{reshaped:?}");
                    self.split += 1;
                }
                (asked, Err(why)) => {
                    // A merge that ended before anybody was told to absorb was not
                    // done.
                    if let (Asked::Merge { survivor, absorbed }, Undone::NotReleased) = (asked, why)
                    {
                        assert!(!self.store.absorbed.contains(&(absorbed, survivor)));
                    }
                    let kind = format!("{why:?}");
                    let kind = kind.split('(').next().unwrap_or_default().to_owned();
                    *self.undone.entry(kind).or_default() += 1;
                }
            }
        }

        /// The worker `name` hears what it is to run: it opens what is new to it,
        /// lets go of what it is no longer to run, and goes on running a region it
        /// split off until its orders have named that once.
        fn hear(&mut self, name: &'static str, says: &mut Vec<(&'static str, Says)>) {
            let ordered = self.cluster.assignments(name);
            let named = |part: RegionId| ordered.iter().any(|held| held.region == part);
            let parts = self.parts.get_mut(name).unwrap();
            parts.retain(|(_, _, made)| !named(*made));
            let mut runs: Vec<Assignment> = parts
                .iter()
                .map(|(_, as_epoch, made)| part(made.0, *as_epoch))
                .collect();
            for held in ordered {
                match self.store.open(held.region, held.epoch) {
                    Ok(()) => runs.push(held),
                    Err(Shut::Absorbed(into)) => {
                        says.push((name, Says::AbsorbEnded(into, held.region, Ok(()))));
                    }
                    Err(Shut::Seen(seen)) => {
                        says.push((name, Says::EpochRefused(held.region, seen)));
                    }
                }
            }
            self.running.insert(name, runs);
        }

        /// The process of the worker `name` is gone, with what it had in memory.
        fn stop(&mut self, name: &'static str) {
            self.alive.remove(name);
            self.running.get_mut(name).unwrap().clear();
            self.parts.get_mut(name).unwrap().clear();
            self.inbox.get_mut(name).unwrap().clear();
        }

        /// The worker `name` registers, reporting what it runs, hears its orders in
        /// answer, and says again of every region it split off and has not been
        /// given yet that it did. The service has the list read.
        fn register(&mut self, name: &'static str) {
            let holding = self.running[name].clone();
            let address = format!("{name}:25601");
            let changes = self.cluster.register(self.now, name, &address, &holding);
            self.alive.insert(name);
            self.said(changes);
            let mut says = Vec::new();
            self.hear(name, &mut says);
            for (region, as_epoch, made) in self.parts[name].clone() {
                says.push((name, Says::SplitEnded(region, as_epoch, Ok(made))));
            }
            for (name, say) in says {
                self.say(name, say);
            }
            self.reading = Some(self.read());
        }

        /// The worker `name` finds out which of its regions the store has shut on it,
        /// as a runner does with its next commit, and lets go of those; it vouches
        /// for the others, and registers again if the coordinator does not know it.
        fn beat(&mut self, name: &'static str) {
            let mut says = Vec::new();
            for held in self.running[name].clone() {
                let say = match self.store.open(held.region, held.epoch) {
                    Ok(()) => continue,
                    Err(Shut::Absorbed(into)) => Says::AbsorbEnded(into, held.region, Ok(())),
                    Err(Shut::Seen(seen)) => Says::EpochRefused(held.region, seen),
                };
                says.push(say);
                self.running
                    .get_mut(name)
                    .unwrap()
                    .retain(|run| *run != held);
                let parts = self.parts.get_mut(name).unwrap();
                parts.retain(|(_, _, made)| *made != held.region);
            }
            for say in says {
                self.say(name, say);
            }
            let runs = self.running[name].iter();
            let vouches: Vec<(RegionId, Vouch)> =
                runs.map(|held| (held.region, Vouch::Committed)).collect();
            if !self.cluster.heartbeat_with(self.now, name, &vouches) {
                self.register(name);
            }
        }

        /// Whether the worker `name` runs `region` with `epoch` and the store still
        /// takes its word for it.
        fn runs(&self, name: &str, region: RegionId, epoch: u64) -> bool {
            let held = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
            self.running[name].iter().any(held) && self.store.living.get(&region) == Some(&epoch)
        }

        /// The worker `name` does the first thing it was told and has not done.
        fn act(&mut self, name: &'static str) {
            let inbox = self.inbox.get_mut(name).unwrap();
            if inbox.is_empty() {
                return;
            }
            let said = match inbox.remove(0) {
                Told::Release(region, epoch) => {
                    let held = |held: &Assignment| (held.region, held.epoch) == (region, epoch);
                    self.running.get_mut(name).unwrap().retain(|run| !held(run));
                    // Now and then the word that it has let go is lost.
                    if self.lost(8) {
                        return;
                    }
                    self.cluster.released(self.now, name, region.0, epoch)
                }
                Told::Reshape(Order::Prepare { .. }) => return,
                Told::Reshape(Order::Absorb {
                    region,
                    epoch,
                    absorbed,
                    as_epoch,
                }) => {
                    let outcome = self.absorb(name, region, epoch, absorbed, as_epoch);
                    if self.lost(6) {
                        return;
                    }
                    let now = self.now;
                    self.cluster
                        .absorb_ended(now, name, region.0, absorbed.0, outcome)
                }
                Told::Reshape(Order::SplitOff {
                    region,
                    epoch,
                    as_epoch,
                    ..
                }) => {
                    let outcome = self.split_off(name, region, epoch, as_epoch);
                    if self.lost(6) {
                        return;
                    }
                    let outcome = outcome.map(|part| part.0);
                    self.cluster
                        .split_ended(self.now, name, region.0, as_epoch, outcome)
                }
            };
            self.said(said);
        }

        /// The worker `name` has `region` absorb `absorbed`, as far as the store lets
        /// it.
        fn absorb(
            &mut self,
            name: &'static str,
            region: RegionId,
            epoch: u64,
            absorbed: RegionId,
            as_epoch: u64,
        ) -> Result<(), Off> {
            if !self.runs(name, region, epoch) {
                return Err(Off::NotRunning);
            }
            match self.store.open(absorbed, as_epoch) {
                // The order came twice.
                Err(Shut::Absorbed(into)) if into == region => return Ok(()),
                Err(Shut::Absorbed(_)) => return Err(Off::Unreadable),
                Err(Shut::Seen(seen)) => {
                    self.say(name, Says::EpochRefused(absorbed, seen));
                    return Err(Off::Refused);
                }
                Ok(()) => {}
            }
            if self.lost(5) {
                return Err(Off::Busy);
            }
            self.store.living.remove(&absorbed);
            self.store.absorbed.push((absorbed, region));
            Ok(())
        }

        /// The worker `name` splits `region`, as far as the store lets it, and runs
        /// the new region from memory. The store names it, whatever the order said.
        fn split_off(
            &mut self,
            name: &'static str,
            region: RegionId,
            epoch: u64,
            as_epoch: u64,
        ) -> Result<RegionId, Off> {
            if !self.runs(name, region, epoch) {
                return Err(Off::NotRunning);
            }
            // While the world settles, nothing new is made.
            if !self.lossy || self.random.once_in(3) {
                return Err(Off::Nobody);
            }
            let made = RegionId(self.store.next);
            self.store.next += 1;
            self.store.living.insert(made, as_epoch);
            self.running
                .get_mut(name)
                .unwrap()
                .push(part(made.0, as_epoch));
            self.parts
                .get_mut(name)
                .unwrap()
                .push((region, as_epoch, made));
            Ok(made)
        }

        /// The reading that is under way comes back, if one is.
        fn hand_in(&mut self) {
            let Some(reading) = self.reading.take() else {
                return;
            };
            let changes = match reading {
                Some(list) => self.cluster.listed(self.now, &list),
                None => self.cluster.unlisted(self.now),
            };
            self.said(changes);
        }

        /// Somebody asks for two regions to be merged or for one to be split: mostly
        /// regions the coordinator knows. The service reads the list first.
        fn ask(&mut self, asker: u64) {
            self.reading = Some(self.read());
            let read = self.reading.as_ref().is_some_and(Option::is_some);
            self.hand_in();
            if !read {
                return;
            }
            let known: Vec<u32> = self
                .cluster
                .coordinator
                .regions
                .keys()
                .map(|id| id.0)
                .collect();
            let mut pick = || match self.random.below(8) {
                0 => self.random.below(u64::from(self.store.next) + 1) as u32,
                _ => known[self.random.below(known.len() as u64) as usize],
            };
            let (one, other) = (pick(), pick());
            let (begun, asked) = if self.random.once_in(2) {
                let asked = Asked::Merge {
                    survivor: RegionId(one),
                    absorbed: RegionId(other),
                };
                (self.cluster.merge(self.now, one, other, Some(asker)), asked)
            } else {
                let asked = Asked::Split {
                    region: RegionId(one),
                };
                let begun = self.cluster.split(self.now, one, &CHUNKS, Some(asker));
                (begun, asked)
            };
            match begun {
                Ok(changes) => {
                    self.askers.insert(asker, asked);
                    self.said(changes);
                }
                Err(_) => *self.undone.entry("refused".to_owned()).or_default() += 1,
            }
        }

        /// One thing happens, at random.
        fn step(&mut self, step: u64) {
            self.now += self.random.below(LEASE / 40);
            let name = CREW[self.random.below(4) as usize];
            let alive = self.alive.contains(name);
            match self.random.below(100) {
                0..30 if alive => self.beat(name),
                30..45 => {
                    let changes = self.cluster.tick(self.now);
                    self.said(changes);
                }
                45..65 if alive => self.act(name),
                65..77 => self.hand_in(),
                77..87 => self.ask(step),
                87..90 if alive => self.register(name),
                // The process dies, and its connection ends with it.
                90..92 if alive => {
                    self.stop(name);
                    let changes = self.cluster.disconnected(self.now, name);
                    self.said(changes);
                }
                92..97 if !alive => self.register(name),
                97 if alive => {
                    let changes = self.cluster.leaving(self.now, name);
                    self.said(changes);
                }
                // The coordinator is replaced by one that knows nothing. Nobody
                // hears of what was asked of the old one; the new one reads the list.
                98 if self.random.once_in(4) => {
                    self.cluster.restart(self.now, self.highest);
                    self.askers.clear();
                    *self.undone.entry("restarts".to_owned()).or_default() += 1;
                    self.reading = Some(self.read());
                }
                _ => {}
            }
        }

        /// Every worker is there and does what it is told, nothing gets lost, and
        /// nobody asks for anything, for eight leases: after that the coordinator has
        /// the regions the store has, each with an owner that runs it, and nothing
        /// is under way.
        fn settle(&mut self) {
            self.lossy = false;
            for name in CREW {
                self.register(name);
            }
            for _ in 0..64 {
                self.now += LEASE / 8;
                for name in CREW {
                    self.beat(name);
                    while !self.inbox[name].is_empty() {
                        self.act(name);
                    }
                    let mut says = Vec::new();
                    self.hear(name, &mut says);
                    assert!(
                        says.is_empty(),
                        "a worker heard what it had not heard before"
                    );
                }
                self.hand_in();
                let changes = self.cluster.tick(self.now);
                self.said(changes);
            }
            self.hand_in();

            let coordinator = &self.cluster.coordinator;
            assert!(coordinator.merges.is_empty(), "{:?}", coordinator.merges);
            assert!(coordinator.splits.is_empty(), "{:?}", coordinator.splits);
            assert!(!coordinator.owed && !coordinator.reading);
            assert!(self.askers.is_empty(), "{:?}", self.askers);
            let known: Vec<&RegionId> = coordinator.regions.keys().collect();
            let there: Vec<&RegionId> = self.store.living.keys().collect();
            assert_eq!(known, there);
            let table = self.cluster.table();
            assert!(table.is_complete(), "{table:?}");
            assert_eq!(table.absorbed, self.store.absorbed);
            for name in CREW {
                let held = |held: &Assignment| (held.region, held.epoch);
                let mut runs: Vec<_> = self.running[name].iter().map(held).collect();
                runs.sort_unstable();
                let owns: Vec<_> = self.cluster.assignments(name).iter().map(held).collect();
                assert_eq!(runs, owns, "{name}");
                for (region, epoch) in owns {
                    assert_eq!(self.store.living[&region], epoch, "{name}");
                }
            }
        }
    }

    /// Regions are merged and split at random while workers come and go, messages
    /// get lost, the list cannot always be read and the coordinator is replaced now
    /// and then. [`Cluster`] checks every call. This adds that whoever asked hears
    /// of it once, that nothing is said to be done that the store does not have, and
    /// that the coordinator ends up with the regions the store has, whatever
    /// happened on the way.
    #[test]
    fn regions_merge_and_split_whatever_gets_lost_and_end_up_as_the_store_has_them() {
        let (mut merged, mut split) = (0, 0);
        let mut undone: BTreeMap<String, usize> = BTreeMap::new();
        for seed in 1..=40_u64 {
            let mut world = World::new(seed);
            for step in 0..3000 {
                world.step(step);
            }
            world.settle();
            merged += world.merged;
            split += world.split;
            for (kind, count) in world.undone {
                *undone.entry(kind).or_default() += count;
            }
        }
        // All of it has in fact happened, and often.
        assert!(merged >= 100 && split >= 100, "{merged} {split} {undone:?}");
        for kind in [
            "Off",
            "NotReleased",
            "Overdue",
            "Disowned",
            "Unread",
            "refused",
            "restarts",
        ] {
            let count = undone.get(kind).copied().unwrap_or(0);
            assert!(count >= 20, "{kind}: {merged} {split} {undone:?}");
        }
    }
}
