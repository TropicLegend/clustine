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

/// What a call changed, so that the service knows whom to tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// The workers whose assignments are not what they were before the call, in
    /// ascending order of their names. A worker that was forgotten is among them if it
    /// owned anything.
    pub workers: Vec<String>,
    /// Whether the routing table is not what it was before the call.
    pub routing: bool,
}

/// A worker that has registered.
#[derive(Debug, Clone)]
struct Worker {
    /// Host and port at which edges reach it.
    address: String,
    /// When it was last heard from.
    heard: Instant,
    /// Its place among the workers: one that registered earlier has a lower number.
    arrival: u64,
}

/// What the coordinator knows about a region of the layout.
#[derive(Debug, Clone, Default)]
struct Region {
    /// Who runs it, if anyone. An owner is always a registered worker.
    owner: Option<Owner>,
    /// The epoch of its owner or, while it has none, of the last one it had. It never
    /// goes down, and it is 0 as long as the region has never had an owner.
    epoch: u64,
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
/// A worker registers and then has to be heard from at least once per lease. One that
/// owns nothing is waiting, and [`Coordinator::tick`] gives the regions without an owner
/// to the waiting workers. A worker that is silent for longer than a lease is forgotten,
/// and what it ran goes to a waiting worker, with a higher epoch. That is also the only
/// way for a worker to leave: it falls silent. The times that are passed in need not be
/// in order; a worker was last heard from at the latest of them.
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
/// The owner stays registered and goes to the back of the waiting workers, so that
/// another one gets the region if there is one.
///
/// Whatever workers report and in whatever order, a region never has two owners and
/// never goes back to an epoch below one it has had, because storage and peers tell the
/// current owner from a replaced one by the epoch alone.
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
    /// of it is given away.
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
                };
                self.workers.insert(name.to_owned(), worker);
                self.arrivals += 1;
            }
        }
        for holding in holding {
            self.report(now, name, holding);
        }
        Ok(self.changes_since(&before))
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
    /// like a [`Coordinator::tick`] at `now`, so that a region the worker dropped goes
    /// to a waiting worker at once, with an epoch above `seen`, and edges see it change
    /// hands in one new routing table. The waiting worker may be the same one: being
    /// refused says nothing against it.
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
        self.changes_since(&before)
    }

    /// Forgets the workers whose lease has run out and hands out regions; to be called
    /// regularly.
    ///
    /// Leases are only looked at here: a worker that is heard from late, but before the
    /// tick that would have forgotten it, keeps what it has.
    ///
    /// Regions without an owner go to the waiting workers, the lowest region first and
    /// the worker that registered first before the others, one region for each worker.
    /// Such an assignment has an epoch above every epoch issued or reported so far, and
    /// entity ids that were never issued or reported. A region stays without an owner if
    /// no worker is waiting, or if epochs or entity ids have run out.
    pub fn tick(&mut self, now: Instant) -> Changes {
        let before = self.holders();
        self.settle(now);
        self.changes_since(&before)
    }

    /// What a tick at `now` does.
    fn settle(&mut self, now: Instant) {
        self.expire(now);
        if now.saturating_duration_since(self.started) >= self.config.lease {
            self.assign(now);
        }
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

    /// Forgets the workers that have been silent for longer than a lease, and takes
    /// regions from owners that no longer vouch for them. Those regions are without an
    /// owner again.
    fn expire(&mut self, now: Instant) {
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
            // region again, so a waiting worker that has not failed gets it first.
            let worker = self
                .workers
                .get_mut(&owner.worker)
                .expect("the owner of a region is a registered worker");
            worker.arrival = self.arrivals;
            self.arrivals += 1;
        }
    }

    /// Gives the regions without an owner to the waiting workers, as of `now`.
    fn assign(&mut self, now: Instant) {
        let busy: BTreeSet<&str> = self
            .regions
            .values()
            .filter_map(|region| region.owner.as_ref())
            .map(|owner| owner.worker.as_str())
            .collect();
        let mut waiting: Vec<(u64, &str)> = self
            .workers
            .iter()
            .filter(|(name, _)| !busy.contains(name.as_str()))
            .map(|(name, worker)| (worker.arrival, name.as_str()))
            .collect();
        waiting.sort_unstable();

        let unowned: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, region)| region.owner.is_none())
            .map(|(id, _)| *id)
            .collect();
        let pairs: Vec<(RegionId, String)> = unowned
            .into_iter()
            .zip(waiting)
            .map(|(id, (_, name))| (id, name.to_owned()))
            .collect();
        for (id, name) in pairs {
            // Epochs do not come back once they have run out, so there is nothing to
            // assign the other regions with either.
            let Some(epoch) = self.last_epoch.checked_add(1) else {
                break;
            };
            self.last_epoch = epoch;
            let entity_ids = self.fill_entity_ids();
            let region = self
                .regions
                .get_mut(&id)
                .expect("the region is one of the layout's");
            region.epoch = epoch;
            region.owner = Some(Owner::new(&name, entity_ids, now));
            info!(region = %id, worker = %name, epoch, "a region was assigned");
        }
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
            let result = self
                .coordinator
                .register(now, name, address, holding, layout);
            match &result {
                Ok(changes) => {
                    self.addresses.insert(name.to_owned(), address.to_owned());
                    self.verify(&before, changes);
                    // A registration is nobody else's business.
                    assert!(changes.workers.iter().all(|worker| worker == name));
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
        fn verify(&self, before: &View, changes: &Changes) {
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
    fn the_table_fills_up_with_a_new_version_for_each_region() {
        let mut cluster = Cluster::new(&[0]);
        // Nobody is there to run anything.
        assert_eq!(cluster.tick(LEASE), Changes::default());

        cluster.register(LEASE, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let table = cluster.table();
        assert_eq!(table.version, FIRST_EPOCH + 1);
        assert_eq!(table.routes, [route(0, FIRST_EPOCH + 1, "a:25601")]);
        assert!(!table.is_complete());

        cluster.register(LEASE + 1, "b", "b:25601", &[]);
        assert_eq!(cluster.tick(LEASE + 1), changes(&["b"], true));
        let table = cluster.table();
        assert_eq!(table.version, FIRST_EPOCH + 2);
        assert_eq!(
            table.routes,
            [
                route(0, FIRST_EPOCH + 1, "a:25601"),
                route(1, FIRST_EPOCH + 2, "b:25601"),
            ]
        );
        assert!(table.is_complete());
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
        cluster.register(0, "a", "a:25601", &[]);
        assert_eq!(cluster.tick(LEASE), changes(&["a"], true));
        let held = cluster.assignments("a");
        let epoch = held[0].epoch;

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
                cluster.epoch_refused(LEASE, name, region, seen),
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
        // uses up another.
        let kept = assignment(1, 5, EntityIds::BLOCK_COUNT - 1);
        cluster.register(0, "keeps", "keeps:25601", &[kept]);
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
    /// random. [`Cluster`] checks every call; this adds what only shows over time.
    #[test]
    fn epochs_only_rise_and_nothing_is_shared_whatever_workers_do() {
        const WORKERS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];
        let (mut issued, mut resumed, mut turned_away, mut lost) = (0, 0, 0, 0);
        let (mut refused, mut restarts, mut dropped) = (0, 0, 0);

        for seed in 1..=24_u64 {
            let mut random = Generator(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            // In every other run the workers only report what a coordinator gave them.
            let honest = seed % 2 == 0;
            let mut cluster = Cluster::new(&[-8, 0, 8]);
            let fingerprint = cluster.layout.fingerprint();
            let mut now = 0;
            let mut grace_ends = LEASE;
            // What each worker runs: what it was to run when it last heard of it.
            let mut running: BTreeMap<String, Vec<Assignment>> = BTreeMap::new();
            // The highest epoch any of the coordinators issued or was told of.
            let mut highest_epoch = FIRST_EPOCH;
            // What the present coordinator has seen: the blocks of entity ids it issued
            // or was told of, and the latest owner of each region with its epoch.
            let mut used_blocks: BTreeSet<i32> = BTreeSet::new();
            let mut tenures: BTreeMap<RegionId, (String, u64)> = BTreeMap::new();

            for _ in 0..4000 {
                now += random.below(LEASE / 8);
                let name = WORKERS[random.below(6) as usize];
                let before = cluster.view();
                let roll = random.below(100);
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
                } else if roll < 88 {
                    let changes = cluster.tick(now);
                    let mut new = Vec::new();
                    for worker in WORKERS {
                        let had = before.assignments(worker);
                        let has = cluster.assignments(worker);
                        if has.iter().any(|assignment| !had.contains(assignment)) {
                            // Only a worker that had nothing, or lost all it had in
                            // this tick, is given a region, and only one.
                            assert_eq!(has.len(), 1, "{worker} has {has:?}");
                            assert!(!had.contains(&has[0]), "{worker} had {had:?}");
                            new.push(has[0]);
                        } else {
                            lost += had.len() - has.len();
                        }
                    }
                    // The lowest region comes first.
                    new.sort_by_key(|assignment| assignment.region);
                    for assignment in new {
                        assert!(now >= grace_ends, "assigned during the grace period");
                        assert!(assignment.epoch > highest_epoch, "{assignment:?}");
                        highest_epoch = assignment.epoch;
                        let block = assignment.entity_ids.first.0 / EntityIds::BLOCK_SIZE;
                        assert_eq!(assignment.entity_ids, ids(block as u32));
                        assert!(used_blocks.insert(block), "{assignment:?}");
                        issued += 1;
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
                    // A region that was dropped may have been given away at once.
                    let mut new: Vec<Assignment> = WORKERS
                        .iter()
                        .flat_map(|worker| {
                            let had = before.assignments(worker).to_vec();
                            let has = cluster.assignments(worker);
                            has.into_iter().filter(move |gained| !had.contains(gained))
                        })
                        .collect();
                    new.sort_by_key(|assignment| assignment.region);
                    for gained in new {
                        assert!(now >= grace_ends, "assigned during the grace period");
                        assert!(gained.epoch > highest_epoch, "{gained:?}");
                        highest_epoch = gained.epoch;
                        let block = gained.entity_ids.first.0 / EntityIds::BLOCK_SIZE;
                        assert!(used_blocks.insert(block), "{gained:?}");
                        issued += 1;
                    }
                    // The worker has dropped the region.
                    if let Some(held) = running.get_mut(name) {
                        held.retain(|held| held.region != region);
                    }
                } else if roll < 94 {
                    // The worker catches up with what it is to run.
                    running.insert(name.to_owned(), cluster.assignments(name));
                } else if roll < 97 {
                    // The worker's process is replaced by a new one, which runs nothing.
                    running.remove(name);
                } else if random.once_in(3) {
                    // The coordinator is replaced by a new one, whose clock tells it no
                    // more than that it is not behind the old one.
                    cluster.restart(now, highest_epoch);
                    grace_ends = now + LEASE;
                    used_blocks.clear();
                    tenures.clear();
                    restarts += 1;
                }

                // A region never goes back to a lower epoch, and unless workers make
                // things up, another owner means a higher one.
                for (worker, assignments) in &cluster.view().assignments {
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
        ];
        assert!(counts.iter().all(|count| *count >= 100), "{counts:?}");
    }
}
