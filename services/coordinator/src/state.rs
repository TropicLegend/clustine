//! Which worker runs which region: the coordinator's decisions as a state machine.
//!
//! Nothing here does I/O or reads a clock. Whoever drives the coordinator passes the
//! time in, so the same calls always lead to the same decisions and each of them can be
//! tested. The service turns messages into calls and [`Changes`] into messages.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use clustine_region::{Layout, RegionId, RegionRoute, RoutingTable};
use clustine_rpc::{Assignment, Vouch};
use clustine_world::{EntityId, EntityIds, Vec3};
use tracing::{info, warn};

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
    /// Who asked for the move, if somebody did; nobody did if the owner is leaving.
    mover: Option<u64>,
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
}

/// What the coordinator knows about a region of the layout.
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
/// region is behind all the others from then on.
///
/// [`Coordinator::tick`] gives the regions without an owner away like that, the lowest
/// first and counting each as it goes, so that they are spread evenly over the workers
/// that are there. A worker that is silent for longer than a lease is forgotten, and
/// what it ran goes to the others, with higher epochs. Nothing is taken from a worker to
/// even things out: one that registers later is given only what has no owner. The times
/// that are passed in need not be in order; a worker was last heard from at the latest
/// of them.
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
/// The owner stays registered and goes behind the other workers, so that of those with
/// as few regions as it has, another one is given the region.
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
/// **release**, which [`Coordinator::move_region`] notes, or the coordinator itself for
/// the regions of a worker that said it is [`Coordinator::leaving`].
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
    /// Every region of the layout.
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
    /// What the call that is being made has to tell the service; empty between calls.
    pending: Pending,
}

impl Coordinator {
    /// How long a region counts as vouched for while its owner says that it waits for
    /// the world store. Moving a region would not help while nobody can reach the store;
    /// after this long the store is likely fine and the owner cut off from it.
    pub const STORE_PATIENCE: Duration = Duration::from_secs(30);

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
    /// - the layout has no such region,
    /// - another worker owns the region: whoever reports a region first keeps it,
    ///   whatever the epochs, or
    /// - the region has had an owner with a higher epoch, so this one was replaced.
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
    /// region, and that is taken as its [`Coordinator::released`].
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

        let before = self.holders();
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
        let before = self.holders();
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
        let before = self.holders();
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
        let before = self.holders();
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
    /// being released already, and there is a target for it, which `to` has to be if it
    /// is given; see [`Coordinator`]. Otherwise a release is noted as asked at `now`,
    /// the target is reserved, and the owner is to be told ([`Changes::releases`]).
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
                .first_target(&from)
                .ok_or(MoveRefusal::NoTarget(region))?,
        };

        let before = self.holders();
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
    /// From any other worker, or with any other epoch, this changes nothing. To say it
    /// is to be heard from, if the worker is registered.
    pub fn released(&mut self, now: Instant, name: &str, region: RegionId, epoch: u64) -> Changes {
        let before = self.holders();
        if let Some(worker) = self.workers.get_mut(name) {
            worker.heard = worker.heard.max(now);
        }
        if self.holds(region, name, epoch) {
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
                .expect("a region without an owner is one of the layout's");
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
    pub fn tick(&mut self, now: Instant) -> Changes {
        let before = self.holders();
        self.settle(now);
        self.finish(&before, now)
    }

    /// What a tick at `now` does.
    fn settle(&mut self, now: Instant) {
        self.forget_silent(now);
        // Before the overdue ones are ended, so that none of them takes a region from
        // an owner it was not asked of, and so that the targets of these are free.
        self.drop_stale_releases();
        self.end_overdue_releases(now);
        self.take_unvouched(now);
        self.assign(now);
    }

    /// Whether `region` has no owner, has not been run with an epoch above `epoch`, and
    /// `name` is a registered worker: then `name` may have been its last owner.
    fn without_owner_since(&self, region: RegionId, name: &str, epoch: u64) -> bool {
        self.workers.contains_key(name)
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

    /// The worker that is given the next region: of those that are registered, have a
    /// connection and are not leaving, the one with the fewest regions, and of several
    /// such the one that has waited longest. `except` is left out, if it is given.
    fn lightest(&self, except: Option<&str>) -> Option<String> {
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
                (load, worker.arrival)
            })
            .map(|(name, _)| name.clone())
    }

    /// The target of a release by `owner` if nobody names one: of the workers that
    /// could be one, the one [`Coordinator::lightest`] picks.
    fn first_target(&self, owner: &str) -> Option<String> {
        self.lightest(Some(owner))
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
            .expect("only a region of the layout is handed over");
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
            .or_else(|| self.first_target(&owner.worker));
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
    /// being released and for which there is a target, the lowest region first.
    fn release_for_leavers(&mut self, now: Instant) {
        let wanted: Vec<(RegionId, String, u64)> = self
            .regions
            .iter()
            .filter(|(region, _)| !self.releases.contains_key(region))
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
            let Some(to) = self.first_target(&from) else {
                break;
            };
            self.note_release(now, region, epoch, &from, &to, None);
        }
    }

    /// What every call that may have changed something ends with; see [`Coordinator`].
    /// Returns what is different from `before`, when these were the owners, and what
    /// else the service has to act on.
    fn finish(&mut self, before: &BTreeMap<RegionId, Holder>, now: Instant) -> Changes {
        self.drop_stale_releases();
        self.forget_leavers();
        self.release_for_leavers(now);

        let mut changes = self.changes_since(before);
        let pending = std::mem::take(&mut self.pending);
        changes.releases = pending.orders;
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

    /// Where edges reach the owner of each region that has one. The version goes up by
    /// one with every call that changes the owner of a region, its address or its epoch.
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
            home: None,
            absorbed: Vec::new(),
            version: self.version,
            layout: self.config.layout.clone(),
            spawn: self.config.spawn,
            routes,
        }
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
        let region = self
            .regions
            .get_mut(&holding.region)
            .expect("the layout has the region, or there would have been an objection");
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
            return Some("the layout has no such region");
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
    /// alone: its owner has stopped ticking on purpose.
    fn take_unvouched(&mut self, now: Instant) {
        let lease = self.config.lease;
        for (id, region) in &mut self.regions {
            if self.releases.contains_key(id) {
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
            // region again, so a worker that has not failed gets it first, unless that
            // one has more regions.
            let worker = self
                .workers
                .get_mut(&owner.worker)
                .expect("the owner of a region is a registered worker");
            worker.arrival = self.arrivals;
            self.arrivals += 1;
        }
    }

    /// Gives each region without an owner to the worker that has the fewest regions
    /// when its turn comes, as of `now`, the lowest region first. A worker that is
    /// leaving or has no connection is given nothing. During the grace period only the
    /// regions that their owners let go of are given.
    fn assign(&mut self, now: Instant) {
        let grace = now.saturating_duration_since(self.started) < self.config.lease;
        let unowned: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, region)| region.owner.is_none() && (region.let_go || !grace))
            .map(|(id, _)| *id)
            .collect();
        for id in unowned {
            // Who can be given a region does not change as these are given.
            let Some(name) = self.lightest(None) else {
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
            .expect("the region is one of the layout's");
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

    /// What is different from `before`, when these were the owners. Moves the routing
    /// table on to its next version if it is among that.
    fn changes_since(&mut self, before: &BTreeMap<RegionId, Holder>) -> Changes {
        let after = self.holders();
        let mut workers = BTreeSet::new();
        let mut routing = false;
        for region in self.regions.keys() {
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
            let result = self
                .coordinator
                .register(now, name, address, holding, layout);
            match &result {
                Ok(changes) => {
                    self.addresses.insert(name.to_owned(), address.to_owned());
                    self.verify(&before, changes);
                    // A registration is nobody else's business, unless the worker
                    // was asked to release a region and comes back without it.
                    assert!(asked || changes.workers.iter().all(|worker| worker == name));
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
            assert_eq!(changes.routing, before.table.routes != now.table.routes);
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

            let live: Vec<Assignment> = now.assignments.values().flatten().copied().collect();
            for (index, one) in live.iter().enumerate() {
                assert!(self.layout.area(one.region).is_some(), "{one:?}");
                for other in &live[index + 1..] {
                    assert_ne!(one.region, other.region, "a region has two owners");
                }
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
            } = &self.coordinator.pending;
            assert!(ended.is_empty() && orders.is_empty() && gone.is_empty());
            // A worker that left is forgotten, and one that is told to release owns
            // what it is told to release.
            for name in &changes.gone {
                assert!(!self.coordinator.workers.contains_key(name), "{name}");
            }
            for order in &changes.releases {
                let release = &self.coordinator.releases[&order.region];
                assert_eq!((&order.worker, order.epoch), (&release.from, release.epoch));
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

    #[test]
    fn a_worker_that_registers_later_is_given_nothing_that_has_an_owner() {
        let mut cluster = Cluster::new(&[0]);
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(regions_of(&cluster, "a"), [0, 1]);
        let table = cluster.table();

        // Nothing is taken from a worker to even things out.
        assert_eq!(
            cluster.register(LEASE, "b", "b:25601", &[]),
            Changes::default()
        );
        for now in [LEASE, 2 * LEASE, 3 * LEASE] {
            for name in ["a", "b"] {
                assert!(cluster.heartbeat(now, name));
            }
            assert_eq!(cluster.tick(now), Changes::default());
        }
        assert!(cluster.assignments("b").is_empty());
        assert_eq!(cluster.table(), table);

        // It is the first to be given a region that loses its owner, though: here one
        // that the other worker no longer vouches for.
        assert!(cluster.heartbeat_with(4 * LEASE + 1, "a", &COMMITTED));
        assert!(cluster.heartbeat_with(4 * LEASE + 1, "b", &[]));
        assert_eq!(cluster.tick(4 * LEASE + 1), changes(&["a", "b"], true));
        assert_eq!(regions_of(&cluster, "a"), [0]);
        assert_eq!(regions_of(&cluster, "b"), [1]);
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
        assert_eq!(cluster.tick(2 * LEASE + 1), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("b"),
            [assignment(1, FIRST_EPOCH + 2, 1)]
        );
        // And it is given to `a` again, which waits alone.
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 3, 2)]
        );
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

    #[test]
    fn a_holding_for_a_region_the_layout_does_not_have_is_not_honoured() {
        let mut cluster = Cluster::new(&[]);
        let claim = assignment(1, FIRST_EPOCH + 20, 0);
        assert_eq!(
            cluster.register(0, "a", "a:25601", &[claim]),
            Changes::default()
        );
        assert!(cluster.assignments("a").is_empty());

        // Its epoch and its entity ids are left out from now on all the same.
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        assert_eq!(
            cluster.assignments("a"),
            [assignment(0, FIRST_EPOCH + 21, 1)]
        );
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

        // Nothing is left for a worker that waits.
        cluster.register(0, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE), Changes::default());
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
        let expected = Changes {
            moves: vec![outcome(1, 0, Some(("c", FIRST_EPOCH + 4)), false)],
            ..changes(&["a", "c"], true)
        };
        assert_eq!(cluster.tick(2 * LEASE + 101), expected);
        assert_eq!(regions_of(&cluster, "c"), [0, 1]);
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
    /// call; this adds what only shows over time, and that whoever is given a region
    /// or picked as a target is the worker with the fewest regions then.
    #[test]
    fn epochs_only_rise_and_nothing_is_shared_whatever_workers_do() {
        const WORKERS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
        let (mut issued, mut resumed, mut turned_away, mut lost) = (0, 0, 0, 0);
        let (mut refused, mut restarts, mut dropped) = (0, 0, 0);
        let (mut moves, mut unmoved, mut let_go, mut overdue) = (0, 0, 0, 0);
        let (mut left, mut vanished, mut asked_to_leave, mut cut_off) = (0, 0, 0, 0);
        let (mut compared, mut meant, mut shared, mut handed_on) = (0, 0, 0, 0);

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
                cluster.last = None;
                // What the worker reports to hold, if this step is a registration.
                let mut reported: Option<Vec<Assignment>> = None;
                // The region its owner lets go of in this step, if one does.
                let mut let_go_of: Option<RegionId> = None;
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
                    if owned {
                        dropped += 1;
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
                            let others = fit_before.iter();
                            let chosen = others
                                .filter(|(other, _)| **other != begun.from)
                                .min_by_key(|(_, load)| **load)
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
                // workers that could be given it at that moment. What the others had
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
                // The same for the releases that were begun for workers that leave,
                // which are begun in the order of their regions, each counting for its
                // target from then on.
                let begun: Vec<(&RegionId, &Release)> = coordinator
                    .releases
                    .iter()
                    .filter(|(_, release)| release.mover.is_none())
                    .filter(|(region, release)| {
                        let before = releases_before.get(region);
                        !before.is_some_and(|before| before.epoch == release.epoch)
                    })
                    .collect();
                for (index, (_, release)) in begun.iter().enumerate() {
                    let later = |other: &str| {
                        let later = begun[index..].iter();
                        later.filter(|(_, later)| later.to == other).count()
                    };
                    let chosen = fit
                        .iter()
                        .filter(|(other, _)| **other != release.from)
                        .min_by_key(|(other, (load, arrival))| (load - later(other), *arrival))
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
        ];
        assert!(counts.iter().all(|count| *count >= 100), "{counts:?}");
    }
}
