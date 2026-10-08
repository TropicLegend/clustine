//! The store for other processes, over TCP.
//!
//! A connection is about one region. Whoever opens it says hello and is told whether the
//! region is theirs now; from then on they send requests and are sent the answers, each
//! in order and everything in the format of [`wire`]. [`serve`] is the store's side of
//! this, [`StoreHandle::connect`] the owner's.
//!
//! What a region is restored with can be far larger than a message may be: a busy region
//! commits a delta every tick, and its checkpoints are minutes apart. So a welcome is
//! followed by the region's state and deltas in parts of bounded size, a single large
//! one in as many pieces as it takes, and the region is its owner's once the owner has
//! the last of them.
//!
//! On either side one thread reads from the connection and another writes to it, so that
//! neither side ever keeps the other from sending. A connection lasts as long as its
//! region is owned through it: the owner gives the region up by closing it, and the store
//! closes it when another owner takes the region over.

use std::collections::BTreeMap;
use std::io::{self, BufReader, BufWriter, ErrorKind, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use clustine_rpc::{
    RegionHello, Restored, RestoredItem, RestoredPart, RestoredPiece, StoreReply, StoreRequest,
    StoreWelcome, TickState, wire,
};
use clustine_world::EntityIds;
use tracing::{info, warn};

use crate::{Link, Store, StoreError, StoreHandle};

/// How long the other side may take over its part of the greeting.
const GREETING_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the server waits before it looks for a new connection again.
const ACCEPT_INTERVAL: Duration = Duration::from_millis(5);

/// How much is read from or written to a connection at a time.
const BUFFER: usize = 64 * 1024;

/// How much of what a region is restored with goes into one message, counted as by
/// [`Parts`]. Far below [`wire::MAX_MESSAGE_LENGTH`], so that neither side holds much
/// more than the region's state itself while it crosses.
const PART_BYTES: usize = 1024 * 1024;

// A part is a few bytes more on the connection than what is counted for it.
const _: () = assert!(PART_BYTES + 16 <= wire::MAX_MESSAGE_LENGTH as usize / 8);

/// What a piece takes in its part besides its bytes, at most: what it is a piece of,
/// its tick, the number of its bytes and whether it is complete.
const PIECE_OVERHEAD: usize = 24;

/// A store that is being served to other processes. Stopping or dropping it ends that;
/// the store itself runs on for whoever else holds it.
pub struct Server {
    address: SocketAddr,
    shared: Arc<Shared>,
    /// The thread that accepts connections, until the server is stopped.
    accepting: Option<JoinHandle<()>>,
}

/// What the threads of a server share.
#[derive(Default)]
struct Shared {
    stopping: AtomicBool,
    /// The connections that are open, by a number each is given.
    connections: Mutex<BTreeMap<u64, Connection>>,
}

/// An open connection, as seen from outside the thread that serves it.
struct Connection {
    stream: Arc<TcpStream>,
    thread: JoinHandle<()>,
}

/// Serves `store` to other processes on `listener` until the returned [`Server`] is
/// stopped or dropped.
pub fn serve(store: Store, listener: TcpListener) -> io::Result<Server> {
    let address = listener.local_addr()?;
    // A thread that waits for a connection cannot be told to stop. So the listener
    // never waits, and is asked again every few milliseconds.
    listener.set_nonblocking(true)?;
    let shared = Arc::new(Shared::default());
    let accepting = thread::Builder::new()
        .name("store-accept".to_owned())
        .spawn({
            let shared = Arc::clone(&shared);
            move || accept(&store, &listener, &shared)
        })?;
    Ok(Server {
        address,
        shared,
        accepting: Some(accepting),
    })
}

impl Server {
    /// The address the store is served at.
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// Stops accepting and closes every connection; returns when its threads have ended.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Relaxed);
        if let Some(accepting) = self.accepting.take() {
            accepting.thread().unpark();
            let _ = accepting.join();
        }
        // No connection is added any more. They are closed without the lock, which
        // their threads take before they end.
        let connections = std::mem::take(&mut *self.shared.connections());
        for connection in connections.values() {
            let _ = connection.stream.shutdown(Shutdown::Both);
        }
        for connection in connections.into_values() {
            let _ = connection.thread.join();
        }
    }
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, BTreeMap<u64, Connection>> {
        // Whoever holds the lock adds or removes a connection and does nothing else, so
        // the list is in order even after a panic.
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts a thread that serves the connection `stream`, the `number`th, and notes
    /// the connection down so that the server can close it.
    fn attach(
        self: &Arc<Self>,
        number: u64,
        stream: TcpStream,
        peer: SocketAddr,
        store: Store,
    ) -> io::Result<()> {
        // On some systems a connection never waits either if its listener does not.
        stream.set_nonblocking(false)?;
        let stream = Arc::new(stream);
        // Held until the connection is noted down, so that its thread cannot find it
        // missing when it ends.
        let mut connections = self.connections();
        let thread = thread::Builder::new()
            .name("store-requests".to_owned())
            .spawn({
                let (shared, stream) = (Arc::clone(self), Arc::clone(&stream));
                move || {
                    converse(&store, &stream, peer);
                    let _ = stream.shutdown(Shutdown::Both);
                    // A server that is stopping has removed it already.
                    shared.connections().remove(&number);
                }
            })?;
        connections.insert(number, Connection { stream, thread });
        Ok(())
    }
}

/// Accepts connections and has each served by a thread of its own, until the server is
/// stopped.
fn accept(store: &Store, listener: &TcpListener, shared: &Arc<Shared>) {
    let mut accepted = 0;
    while !shared.stopping.load(Ordering::Relaxed) {
        let (stream, peer) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::park_timeout(ACCEPT_INTERVAL);
                continue;
            }
            Err(error) => {
                // Typically the process is out of file descriptors; keep serving the
                // connections that exist and retry.
                warn!(%error, "accepting a connection failed");
                thread::park_timeout(Duration::from_millis(100));
                continue;
            }
        };
        accepted += 1;
        if let Err(error) = shared.attach(accepted, stream, peer, store.clone()) {
            warn!(%peer, %error, "a connection could not be served");
        }
    }
}

/// Serves a connection from its hello to its end.
fn converse(store: &Store, stream: &TcpStream, peer: SocketAddr) {
    // Answers are written in batches already, and one must not wait for the next.
    let _ = stream.set_nodelay(true);
    let greeted = greeting(stream, GREETING_TIMEOUT, |stream| {
        wire::blocking::read(stream)
    });
    let hello: RegionHello = match greeted {
        Ok(hello) => hello,
        Err(error) => {
            info!(%peer, %error, "a connection ended without a hello");
            return;
        }
    };
    let RegionHello { region, epoch, .. } = hello;
    let (StoreHandle { link, replies, .. }, restored) = match store.open_region(hello) {
        Ok(opened) => opened,
        Err(error) => {
            info!(%peer, %region, epoch, %error, "a hello was refused");
            let refusal = match error {
                // Said as it is, so that the owner can tell being replaced from
                // anything else.
                StoreError::EpochRefused { seen, .. } => StoreWelcome::EpochRefused { seen },
                error => StoreWelcome::Refused {
                    reason: error.to_string(),
                },
            };
            let _ = wire::blocking::write(&mut &*stream, &refusal);
            return;
        }
    };
    if let Err(error) = welcome(stream, GREETING_TIMEOUT, restored) {
        // The region is given up as with any connection that is lost, by dropping what
        // it was opened with.
        info!(%peer, %region, epoch, %error, "a connection ended before its region was restored");
        return;
    }
    info!(%peer, %region, epoch, "a region was opened by another process");

    thread::scope(|scope| {
        let writing = thread::Builder::new()
            .name("store-replies".to_owned())
            .spawn_scoped(scope, move || {
                let _ = write_queued(&replies, stream, |writer, reply| {
                    wire::blocking::write(writer, reply)
                });
                // There is nothing more to answer: another owner has taken the region
                // over, or this one has given it up and has all its answers. Or the
                // connection has failed, which the thread that reads is told this way.
                let _ = stream.shutdown(Shutdown::Both);
            });
        if let Err(error) = writing {
            warn!(%peer, %region, %error, "a connection could not be served");
            return;
        }

        let mut reader = BufReader::with_capacity(BUFFER, stream);
        loop {
            match wire::blocking::read(&mut reader) {
                Ok(Some(request)) => link.request(request),
                // The owner has closed its side, or the connection was closed here.
                Ok(None) => break,
                Err(error) => {
                    warn!(%peer, %region, %error, "giving up on a connection");
                    // Nothing that makes sense can follow, so no answer is owed either.
                    let _ = stream.shutdown(Shutdown::Both);
                    break;
                }
            }
        }
        // Gives the region up once everything that was asked for has been done. The
        // store closes the replies then, which the thread that writes is waiting for.
        drop(link);
    });
    info!(%peer, %region, epoch, "the connection of a region ended");
}

/// Welcomes the owner of a region on `stream` and sends it what the region is restored
/// with, in parts. The owner may leave the store waiting for `patience` at a time: one
/// that has stopped reading must not hold on to a thread and a region for ever.
fn welcome(stream: &TcpStream, patience: Duration, restored: Restored) -> io::Result<()> {
    let Restored {
        entity_ids,
        state,
        deltas,
    } = restored;
    stream.set_write_timeout(Some(patience))?;
    wire::blocking::write(&mut &*stream, &StoreWelcome::Accepted { entity_ids })?;
    for part in Parts::new(state, deltas, PART_BYTES) {
        wire::blocking::write(&mut &*stream, &part)?;
    }
    // Answers wait for as long as the owner takes to read them.
    stream.set_write_timeout(None)
}

/// The state and the deltas of a [`Restored`] as the parts they are sent in, none of
/// which holds more than `room` bytes.
///
/// What is counted is the bytes of the states and [`PIECE_OVERHEAD`] for each piece, not
/// the number of pieces: one delta can be large, and thousands can be next to nothing.
/// A state or delta that does not fit what is left of a part is cut there and goes on in
/// the next, so that nothing is too large to be sent.
struct Parts {
    state: Option<TickState>,
    deltas: std::vec::IntoIter<TickState>,
    /// What has been taken from the two and is not sent in full, with the number of its
    /// bytes that are.
    cut: Option<(RestoredItem, TickState, usize)>,
    room: usize,
    /// Whether the last part has been made.
    done: bool,
}

impl Parts {
    fn new(state: Option<TickState>, deltas: Vec<TickState>, room: usize) -> Self {
        Self {
            state,
            deltas: deltas.into_iter(),
            cut: None,
            // A part has room for a byte at least, or nothing would ever be sent.
            room: room.max(PIECE_OVERHEAD + 1),
            done: false,
        }
    }

    /// What is sent next, and how many of its bytes have been sent already.
    fn item(&mut self) -> Option<(RestoredItem, TickState, usize)> {
        if let Some(cut) = self.cut.take() {
            return Some(cut);
        }
        if let Some(state) = self.state.take() {
            return Some((RestoredItem::State, state, 0));
        }
        let delta = self.deltas.next()?;
        Some((RestoredItem::Delta, delta, 0))
    }
}

impl Iterator for Parts {
    type Item = RestoredPart;

    fn next(&mut self) -> Option<RestoredPart> {
        if self.done {
            return None;
        }
        let mut pieces = Vec::new();
        let mut room = self.room;
        loop {
            let Some((of, mut state, sent)) = self.item() else {
                // Even if there is nothing in it: the owner waits to be told that it
                // has everything.
                self.done = true;
                return Some(RestoredPart { pieces, last: true });
            };
            if room <= PIECE_OVERHEAD {
                self.cut = Some((of, state, sent));
                return Some(RestoredPart {
                    pieces,
                    last: false,
                });
            }
            let rest = state.state.len() - sent;
            let taken = rest.min(room - PIECE_OVERHEAD);
            room -= PIECE_OVERHEAD + taken;
            let complete = taken == rest;
            let bytes = if sent == 0 && complete {
                // Nearly always, and then nothing is copied.
                std::mem::take(&mut state.state)
            } else {
                state.state[sent..sent + taken].to_vec()
            };
            pieces.push(RestoredPiece {
                of,
                tick: state.tick,
                bytes,
                complete,
            });
            if !complete {
                self.cut = Some((of, state, sent + taken));
                return Some(RestoredPart {
                    pieces,
                    last: false,
                });
            }
        }
    }
}

/// A [`Restored`] that is being put together from the parts it arrives in.
struct Arriving {
    restored: Restored,
    /// The state or delta whose pieces have not all arrived.
    cut: Option<(RestoredItem, TickState)>,
}

impl Arriving {
    fn new(entity_ids: EntityIds) -> Self {
        Self {
            restored: Restored {
                entity_ids,
                state: None,
                deltas: Vec::new(),
            },
            cut: None,
        }
    }

    /// Adds a part. Returns the whole once the part was the last, and itself until then.
    ///
    /// Parts that do not fit together are an error rather than a region restored with
    /// something its store never had.
    fn add(mut self, part: RestoredPart) -> io::Result<Result<Restored, Self>> {
        for piece in part.pieces {
            let (of, state) = match self.cut.take() {
                Some((of, mut state)) => {
                    if (piece.of, piece.tick) != (of, state.tick) {
                        return Err(misfit("a piece does not go on with the one before it"));
                    }
                    state.state.extend_from_slice(&piece.bytes);
                    (of, state)
                }
                None => {
                    let state = TickState {
                        tick: piece.tick,
                        state: piece.bytes,
                    };
                    (piece.of, state)
                }
            };
            if !piece.complete {
                self.cut = Some((of, state));
                continue;
            }
            match of {
                RestoredItem::State => {
                    if self.restored.state.is_some() || !self.restored.deltas.is_empty() {
                        return Err(misfit("a state that is not the first of all"));
                    }
                    self.restored.state = Some(state);
                }
                RestoredItem::Delta => self.restored.deltas.push(state),
            }
        }
        if !part.last {
            return Ok(Err(self));
        }
        if self.cut.is_some() {
            return Err(misfit("the last part ends in the middle of a piece"));
        }
        Ok(Ok(self.restored))
    }
}

fn misfit(what: &str) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        format!("what a region is restored with arrived in disorder: {what}"),
    )
}

/// Takes the other side's part of the greeting from `stream` with `read`, which reads
/// it with [`wire::blocking::read`]. That side may keep this one waiting for `patience`
/// at a time while it does; what follows the greeting may take as long as it likes.
fn greeting<T>(
    stream: &TcpStream,
    patience: Duration,
    read: impl FnOnce(&mut &TcpStream) -> io::Result<Option<T>>,
) -> io::Result<T> {
    stream.set_read_timeout(Some(patience))?;
    // Not through a buffer, which would swallow the beginning of what follows.
    let greeting = read(&mut &*stream);
    stream.set_read_timeout(None)?;
    match greeting {
        Ok(Some(greeting)) => Ok(greeting),
        Ok(None) => Err(io::Error::new(
            ErrorKind::UnexpectedEof,
            "the connection was closed during the greeting",
        )),
        // Which of the two a read that ran out of time reports depends on the system.
        Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Err(io::Error::new(
                ErrorKind::TimedOut,
                "the other side did not get through the greeting in time",
            ))
        }
        Err(error) => Err(error),
    }
}

/// Writes the messages that arrive in `queue` to `stream`, each with `write`, until the
/// queue is closed and empty.
///
/// `write` is [`wire::blocking::write`]. It is passed in, as `read` is to [`greeting`],
/// because this crate cannot name the traits of serde that it asks of a message.
fn write_queued<T>(
    queue: &Receiver<T>,
    stream: &TcpStream,
    write: impl Fn(&mut BufWriter<&TcpStream>, &T) -> io::Result<()>,
) -> io::Result<()> {
    let mut writer = BufWriter::with_capacity(BUFFER, stream);
    while let Ok(first) = queue.recv() {
        write(&mut writer, &first)?;
        // Whatever else is waiting goes out in the same writes.
        for message in queue.try_iter() {
            write(&mut writer, &message)?;
        }
        writer.flush()?;
    }
    Ok(())
}

impl StoreHandle {
    /// Opens a region of the store that is served at `address` (host:port), as
    /// [`Store::open_region`] does with a store in this process.
    ///
    /// Waits for the store's answer. If the store refuses the hello for its epoch, the
    /// error is [`StoreError::EpochRefused`]; if it refuses it otherwise, it is
    /// [`StoreError::Refused`] with the reason the store gave; if the store cannot be
    /// reached or does not answer, it is [`StoreError::Io`].
    pub fn connect(
        address: &str,
        hello: RegionHello,
    ) -> Result<(StoreHandle, Restored), StoreError> {
        connect_within(address, hello, GREETING_TIMEOUT)
    }
}

/// Does what [`StoreHandle::connect`] does, with `patience` for the connection to be
/// made and for the store to answer the hello.
fn connect_within(
    address: &str,
    hello: RegionHello,
    patience: Duration,
) -> Result<(StoreHandle, Restored), StoreError> {
    let stream = Arc::new(reach(address, patience)?);
    // Requests are written in batches already, and one must not wait for the next.
    let _ = stream.set_nodelay(true);
    wire::blocking::write(&mut &*stream, &hello)?;
    // Nothing is asked of the store before all of it is here: the region is not its
    // owner's with half of what it is restored with.
    let restored = greeting(&stream, patience, |stream| welcomed(stream, hello))??;

    let (requests, queued) = mpsc::channel();
    let (answers, replies) = mpsc::channel();
    let lost = Arc::new(AtomicBool::new(false));
    thread::Builder::new()
        .name("store-requests".to_owned())
        .spawn({
            let (stream, lost) = (Arc::clone(&stream), Arc::clone(&lost));
            move || send_requests(&queued, &stream, &lost)
        })?;
    thread::Builder::new()
        .name("store-replies".to_owned())
        .spawn({
            let lost = Arc::clone(&lost);
            move || receive_replies(&stream, &answers, &lost)
        })?;
    let handle = StoreHandle {
        link: Link::Remote(requests),
        replies,
        lost,
    };
    Ok((handle, restored))
}

/// Reads the store's answer to `hello` and, if it is a welcome, what the region is
/// restored with, to the last part. Returns `None` if the store said nothing at all.
fn welcomed(
    stream: &mut &TcpStream,
    hello: RegionHello,
) -> io::Result<Option<Result<Restored, StoreError>>> {
    let entity_ids = match wire::blocking::read(stream)? {
        Some(StoreWelcome::Accepted { entity_ids }) => entity_ids,
        Some(StoreWelcome::EpochRefused { seen }) => {
            return Ok(Some(Err(StoreError::EpochRefused {
                region: hello.region,
                offered: hello.epoch,
                seen,
            })));
        }
        Some(StoreWelcome::Refused { reason }) => {
            return Ok(Some(Err(StoreError::Refused(reason))));
        }
        None => return Ok(None),
    };
    let mut arriving = Arriving::new(entity_ids);
    loop {
        let Some(part) = wire::blocking::read(stream)? else {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "the connection was closed before the region was restored",
            ));
        };
        match arriving.add(part)? {
            Ok(restored) => return Ok(Some(Ok(restored))),
            Err(more) => arriving = more,
        }
    }
}

/// Connects to `address`, trying every address the name stands for.
fn reach(address: &str, patience: Duration) -> io::Result<TcpStream> {
    let mut failure = io::Error::new(ErrorKind::InvalidInput, "the name has no address");
    for address in address.to_socket_addrs()? {
        match TcpStream::connect_timeout(&address, patience) {
            Ok(stream) => return Ok(stream),
            Err(error) => failure = error,
        }
    }
    Err(failure)
}

/// Writes what a handle asks for to the connection, until the handle is dropped or the
/// connection fails.
fn send_requests(requests: &Receiver<StoreRequest>, stream: &TcpStream, lost: &AtomicBool) {
    let written = write_queued(requests, stream, |writer, request| {
        wire::blocking::write(writer, request)
    });
    match written {
        // The handle was dropped and everything it asked for is on its way. The store
        // gives the region up when it gets to the end of that, and then closes the
        // connection, which the thread that reads waits for.
        Ok(()) => {
            let _ = stream.shutdown(Shutdown::Write);
        }
        Err(error) => {
            warn!(%error, "what a region asks of the store could not be sent");
            lost.store(true, Ordering::Relaxed);
            // Ends the thread that reads, and with it a flush that waits for an answer.
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// Reads the answers of the store until the connection ends, which is when the handle
/// is lost unless it was dropped.
fn receive_replies(stream: &TcpStream, replies: &Sender<StoreReply>, lost: &AtomicBool) {
    let mut reader = BufReader::with_capacity(BUFFER, stream);
    loop {
        match wire::blocking::read(&mut reader) {
            // A handle that was dropped does not listen any more. The connection is
            // read to its end all the same: closing it with answers unread would reset
            // it, and the store could lose what it has not yet read of the requests.
            Ok(Some(reply)) => {
                let _ = replies.send(reply);
            }
            Ok(None) => break,
            Err(error) => {
                warn!(%error, "the connection to the store failed");
                break;
            }
        }
    }
    // Before the replies are closed, so that a flush which ends with them finds the
    // handle lost.
    lost.store(true, Ordering::Relaxed);
    // What is asked from now on fails to be written instead of going nowhere.
    let _ = stream.shutdown(Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Read;
    use std::path::Path;
    use std::sync::Barrier;

    use clustine_data::{BLOCK_STATE_COUNT, BlockState, blocks};
    use clustine_region::{Layout, RegionId};
    use clustine_world::{BlockPos, Chunk, ChunkPos};

    use super::*;
    use crate::tests::{
        HELD, Held, any_reply, committed, delta, edited, generator, hello, load, log, open, reply,
        save, stores,
    };

    /// Serves `store` at an address of its own.
    fn served(store: &Store) -> (Server, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = serve(store.clone(), listener).unwrap();
        let address = server.local_addr().to_string();
        (server, address)
    }

    fn connect(address: &str, hello: RegionHello) -> StoreHandle {
        StoreHandle::connect(address, hello).unwrap().0
    }

    /// Waits for `handle` to be lost.
    fn lost(handle: &StoreHandle) {
        for _ in 0..30_000 {
            if handle.is_lost() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("the handle was not lost");
    }

    /// Waits until the server has no connection open any more: the store has been
    /// through everything that came over them.
    fn quiet(server: &Server) {
        for _ in 0..30_000 {
            if server.shared.connections().is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("a connection stayed open");
    }

    /// Whether the other side has closed `connection`, waiting for it if need be.
    fn closed(mut connection: &TcpStream) -> bool {
        connection
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        match connection.read(&mut [0]) {
            Ok(read) => read == 0,
            // A connection that is closed while something is on its way to it is reset.
            Err(error) => !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        }
    }

    /// Says hello on a connection of the test's own and expects to be accepted.
    fn greeted(address: &str, hello: RegionHello) -> TcpStream {
        let mut connection = TcpStream::connect(address).unwrap();
        connection.set_nodelay(true).unwrap();
        wire::blocking::write(&mut connection, &hello).unwrap();
        let welcome = wire::blocking::read(&mut connection).unwrap();
        assert!(
            matches!(welcome, Some(StoreWelcome::Accepted { .. })),
            "{welcome:?}"
        );
        while !part(&mut connection).last {}
        connection
    }

    /// Reads a part of what a region is restored with from a connection of the test's
    /// own.
    fn part(connection: &mut TcpStream) -> RestoredPart {
        wire::blocking::read(connection)
            .unwrap()
            .expect("a part follows")
    }

    /// The number of chunks that are stored in the world in `directory`.
    fn stored(directory: &Path) -> usize {
        fs::read_dir(directory.join("manifests/overworld"))
            .unwrap()
            .map(|region| fs::read_dir(region.unwrap().path()).unwrap().count())
            .sum()
    }

    #[test]
    fn a_remote_handle_loads_generated_and_saved_chunks() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let remote = connect(&address, hello(1, 1));
            assert!(!remote.is_lost());
            let position = ChunkPos::new(7, -9);
            assert_eq!(load(&remote, position), generator().generate(position));
            save(&remote, position, &edited());
            assert_eq!(load(&remote, position), edited());
            // Other chunks are not affected.
            let other = ChunkPos::new(7, -8);
            assert_eq!(load(&remote, other), generator().generate(other));
            assert_eq!(remote.try_reply(), None);
        }
    }

    #[test]
    fn what_a_remote_handle_commits_and_saves_is_found_by_a_local_one_and_after_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        let dug = (BlockPos::new(40, -61, 4), blocks::AIR);
        let entity_ids = {
            let store = Store::local(directory.path(), generator()).unwrap();
            let (server, address) = served(&store);
            let (remote, restored) = StoreHandle::connect(&address, hello(1, 7)).unwrap();
            assert_eq!((restored.state, restored.deltas), (None, Vec::new()));
            // Changes that are in a saved chunk by the time of a checkpoint, and one that
            // is committed after it.
            log(
                &remote,
                5,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            save(&remote, origin, &edited());
            remote.request(StoreRequest::Checkpoint {
                tick: 5,
                state: b"five".to_vec(),
            });
            log(&remote, 6, &[(40, -61, 4, blocks::AIR)]);
            // The answers cross the connection.
            assert_eq!(any_reply(&remote), StoreReply::Committed { tick: 5 });
            assert_eq!(any_reply(&remote), StoreReply::Committed { tick: 6 });
            remote.flush();
            let west = open(&store, hello(0, 1));
            assert_eq!(load(&west, origin), edited());

            // Everything goes away without the chunk that was dug in ever being saved.
            drop((remote, west));
            server.stop();
            restored.entity_ids
        };

        // The region is restored with the state of the checkpoint and the commit after
        // it, in this process or in another, and with the epoch it had.
        let store = Store::local(directory.path(), generator()).unwrap();
        let (server, address) = served(&store);
        let refused = StoreHandle::connect(&address, hello(1, 6));
        assert!(
            matches!(
                refused,
                Err(StoreError::EpochRefused {
                    region: RegionId(1),
                    offered: 6,
                    seen: 7
                })
            ),
            "{:?}",
            refused.err()
        );
        let expected = Restored {
            entity_ids,
            state: Some(TickState {
                tick: 5,
                state: b"five".to_vec(),
            }),
            deltas: vec![TickState {
                tick: 6,
                state: delta(6),
            }],
        };
        let (remote, restored) = StoreHandle::connect(&address, hello(1, 7)).unwrap();
        assert_eq!(restored, expected);
        drop(remote);
        server.stop();
        let (owner, restored) = store.open_region(hello(1, 8)).unwrap();
        assert_eq!(restored, expected);
        assert_eq!(load(&owner, origin), edited());
        let mut expected = generator().generate(dug.0.chunk());
        expected.set(8, -61, 4, blocks::AIR);
        assert_eq!(load(&owner, dug.0.chunk()), expected);
    }

    /// The store is kept busy with a chunk, so that what is asked for after it has to
    /// wait.
    #[test]
    fn a_remote_flush_is_answered_only_when_everything_before_it_is_done() {
        let directory = tempfile::tempdir().unwrap();
        let held = Arc::new(Held(Barrier::new(2)));
        let store = Store::local(directory.path(), held.clone()).unwrap();
        let (_server, address) = served(&store);
        let remote = connect(&address, hello(1, 1));

        remote.request(StoreRequest::Load { position: HELD });
        held.0.wait();
        for x in 0..50 {
            save(&remote, ChunkPos::new(x, 0), &edited());
        }
        remote.request(StoreRequest::Flush);
        thread::sleep(Duration::from_millis(50));
        let answered = remote.try_reply();
        let saved = directory.path().join("manifests/overworld").exists();
        // Looked at only now: a test that fails while it holds the store never ends.
        held.0.wait();
        assert_eq!(answered, None);
        assert!(!saved);

        let loaded = StoreReply::Loaded {
            position: HELD,
            chunk: generator().generate(HELD),
        };
        assert_eq!(reply(&remote), loaded);
        assert_eq!(reply(&remote), StoreReply::Flushed);
        assert_eq!(stored(directory.path()), 50);

        // The same for the flush that waits. A load that is still unanswered does not
        // confuse it.
        for x in 50..100 {
            save(&remote, ChunkPos::new(x, 0), &edited());
        }
        remote.request(StoreRequest::Load {
            position: ChunkPos::new(0, 0),
        });
        remote.flush();
        assert_eq!(stored(directory.path()), 100);
        assert_eq!(remote.try_reply(), None);
    }

    #[test]
    fn two_remote_regions_used_from_two_threads_at_once_get_only_their_own_replies() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let start = Barrier::new(2);
            thread::scope(|scope| {
                for (region, x) in [(0, -1), (1, 0)] {
                    let (address, start) = (&address, &start);
                    scope.spawn(move || {
                        // Every tenth chunk is built on, in a way that is not found
                        // anywhere else in either region.
                        let built = |position: ChunkPos| {
                            let mut chunk = generator().generate(position);
                            if position.z % 10 == 0 {
                                chunk.set(1, 80 + position.z, 1, blocks::STONE);
                                chunk.set(2 + region as usize, 70, 1, blocks::GLASS);
                            }
                            chunk
                        };
                        let remote = connect(address, hello(region, 1));
                        start.wait();
                        for z in 0..100 {
                            let position = ChunkPos::new(x, z);
                            if z % 10 == 0 {
                                save(&remote, position, &built(position));
                            }
                            remote.request(StoreRequest::Load { position });
                        }
                        remote.request(StoreRequest::Flush);
                        for z in 0..100 {
                            let position = ChunkPos::new(x, z);
                            let chunk = built(position);
                            assert_eq!(reply(&remote), StoreReply::Loaded { position, chunk });
                        }
                        assert_eq!(reply(&remote), StoreReply::Flushed);
                        assert_eq!(remote.try_reply(), None);
                    });
                }
            });
        }
    }

    #[test]
    fn refusals_for_a_lower_epoch_and_for_another_layout_arrive_with_their_reason() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let owner = connect(&address, hello(1, 5));
            save(&owner, origin, &edited());
            // Being replaced is told apart from everything else, with the epoch.
            let Err(error) = StoreHandle::connect(&address, hello(1, 4)) else {
                panic!("a lower epoch was accepted");
            };
            assert!(
                matches!(
                    error,
                    StoreError::EpochRefused {
                        region: RegionId(1),
                        offered: 4,
                        seen: 5
                    }
                ),
                "{error}"
            );
            // Otherwise the reason is what the store says to someone in its own process.
            let refused = |hello: RegionHello, reason: StoreError| {
                let Err(error) = StoreHandle::connect(&address, hello) else {
                    panic!("{hello:?} was accepted");
                };
                assert!(
                    matches!(&error, StoreError::Refused(given) if *given == reason.to_string()),
                    "{error}"
                );
                assert!(error.to_string().ends_with(&reason.to_string()), "{error}");
            };
            let other = RegionHello {
                layout: Layout::single().fingerprint(),
                ..hello(0, 1)
            };
            let reason = StoreError::LayoutMismatch {
                expected: hello(0, 1).layout,
                offered: other.layout,
            };
            refused(other, reason);

            // The owner is none the worse for it, and the region that was asked for
            // with another layout was not taken.
            assert!(!owner.is_lost());
            assert_eq!(load(&owner, origin), edited());
            let west = connect(&address, hello(0, 1));
            let position = ChunkPos::new(-1, 0);
            assert_eq!(load(&west, position), generator().generate(position));
        }
    }

    #[test]
    fn a_dropped_remote_handle_frees_its_region_once_what_it_asked_for_is_done() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::local(directory.path(), generator()).unwrap();
        let (server, address) = served(&store);
        let last = ChunkPos::new(49, 0);
        let mut dug = generator().generate(ChunkPos::new(2, 5));
        dug.set(8, -61, 4, blocks::AIR);

        // The next owner is once in another process and once in the store's own.
        let remotely = || connect(&address, hello(1, 3));
        let locally = || open(&store, hello(1, 3));
        let next: [&dyn Fn() -> StoreHandle; 2] = [&remotely, &locally];
        for (state, next) in [blocks::STONE, blocks::GLASS].into_iter().zip(next) {
            let mut built = generator().generate(last);
            built.set(1, 80, 1, state);
            dug.set(9, -61, 4, state);

            let remote = remotely();
            for x in 0..50 {
                // The answers are never read, which must not keep the rest from the store.
                remote.request(StoreRequest::Load {
                    position: ChunkPos::new(x, 0),
                });
                save(&remote, ChunkPos::new(x, 0), &built);
            }
            log(
                &remote,
                9,
                &[(40, -61, 84, blocks::AIR), (41, -61, 84, state)],
            );
            // Dropped without a flush, and with all of that still on its way. The
            // connection ends once the store has been given all of it.
            drop(remote);
            quiet(&server);

            let owner = next();
            assert_eq!(load(&owner, last), built);
            // What was only logged is in the chunk from the hello on.
            assert_eq!(load(&owner, ChunkPos::new(2, 5)), dug);
        }
    }

    #[test]
    fn a_remote_owner_replaced_by_a_higher_epoch_is_lost_and_its_flush_returns() {
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            // The new owner is once in the store's own process and once in another.
            let locally = |hello| open(&store, hello);
            let remotely = |hello| connect(&address, hello);
            let newcomers: [&dyn Fn(RegionHello) -> StoreHandle; 2] = [&locally, &remotely];
            for (region, newcomer) in (0..).zip(newcomers) {
                let position = ChunkPos::new(region as i32 - 1, 0);
                let old = connect(&address, hello(region, 1));
                // Done, because the region is still its own.
                save(&old, position, &edited());
                old.flush();
                assert!(!old.is_lost());
                // Whether or not the answer gets to the old owner before it is lost, it
                // is not given out afterwards.
                old.request(StoreRequest::Load { position });
                let new = newcomer(hello(region, 2));
                lost(&old);
                assert_eq!(old.try_reply(), None);

                // Nothing the old owner asks for is done any more, and none of it
                // answered, which its flush does not wait for.
                save(&old, position, &generator().generate(position));
                old.request(StoreRequest::Load { position });
                old.flush();
                assert_eq!(old.try_reply(), None);
                assert!(!new.is_lost());
                assert_eq!(load(&new, position), edited());

                // The region is not the old owner's to give up either, nor to open again.
                drop(old);
                assert!(matches!(
                    StoreHandle::connect(&address, hello(region, 1)),
                    Err(StoreError::EpochRefused { seen: 2, .. })
                ));
                assert!(!new.is_lost());
                assert_eq!(load(&new, position), edited());
            }
        }
    }

    /// A remote owner takes a region over from one in the store's own process.
    #[test]
    fn a_local_owner_replaced_by_a_remote_one_is_lost() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let old = open(&store, hello(1, 1));
            save(&old, origin, &edited());
            let new = connect(&address, hello(1, 2));
            assert!(old.is_lost() && !new.is_lost());
            save(&old, origin, &generator().generate(origin));
            old.flush();
            assert_eq!(old.try_reply(), None);
            assert_eq!(load(&new, origin), edited());
        }
    }

    /// An owner whose connection was lost before the store noticed comes back with its
    /// own epoch, and takes the region over from its old session.
    #[test]
    fn a_remote_owner_coming_back_with_its_epoch_replaces_its_old_session() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let (old, _) = StoreHandle::connect(&address, hello(1, 3)).unwrap();
            log(
                &old,
                1,
                &[(3, -61, 4, blocks::AIR), (3, 100, 4, blocks::GLASS)],
            );
            committed(&old, 1);
            let (new, restored) = StoreHandle::connect(&address, hello(1, 3)).unwrap();
            assert_eq!(restored.deltas.len(), 1);
            lost(&old);
            log(&old, 2, &[(3, 100, 4, blocks::STONE)]);
            old.flush();
            assert_eq!(old.try_reply(), None);
            assert_eq!(load(&new, origin), edited());
            log(&new, 2, &[(3, 100, 5, blocks::STONE)]);
            committed(&new, 2);
        }
    }

    #[test]
    fn after_the_server_is_stopped_handles_are_lost_and_flush_returns() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (server, address) = served(&store);
            let west = connect(&address, hello(0, 1));
            let east = connect(&address, hello(1, 1));
            save(&east, origin, &edited());
            east.flush();
            // An answer that is still unread when the server goes away.
            west.request(StoreRequest::Load { position: origin });
            west.flush();
            west.request(StoreRequest::Load { position: origin });

            server.stop();
            for handle in [&west, &east] {
                lost(handle);
                // Neither of these panics or waits, and nothing comes of them.
                save(handle, origin, &generator().generate(origin));
                handle.request(StoreRequest::Load { position: origin });
                handle.flush();
                assert_eq!(handle.try_reply(), None);
            }
            // Nobody listens any more: the address can be listened on again.
            TcpListener::bind(&address).unwrap();

            // The store itself runs on, and the regions can be opened again in it.
            let owner = open(&store, hello(1, 1));
            assert_eq!(load(&owner, origin), edited());
        }
    }

    #[test]
    fn dropping_the_server_stops_it() {
        let store = Store::memory(generator());
        let (server, address) = served(&store);
        let remote = connect(&address, hello(1, 1));
        drop(server);
        lost(&remote);
        remote.flush();
    }

    #[test]
    fn connecting_to_a_port_nothing_listens_on_fails() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);
        // A region that no test opens, should one of them be listening there by now.
        let Err(error) = StoreHandle::connect(&address, hello(9, 1)) else {
            panic!("connected to nothing");
        };
        assert!(matches!(error, StoreError::Io(_)), "{error}");

        // Nor is there anything to connect to without a port.
        let Err(error) = StoreHandle::connect("127.0.0.1", hello(9, 1)) else {
            panic!("connected to nothing");
        };
        assert!(matches!(error, StoreError::Io(_)), "{error}");
    }

    #[test]
    fn connecting_to_a_listener_that_says_nothing_fails_without_hanging() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        // The system makes the connection although nobody accepts it, let alone answers.
        let patience = Duration::from_millis(300);
        let Err(error) = connect_within(&address, hello(1, 1), patience) else {
            panic!("connected to a listener that says nothing");
        };
        assert!(
            matches!(&error, StoreError::Io(error) if error.kind() == ErrorKind::TimedOut),
            "{error}"
        );

        // One that hangs up without a word is not waited for at all. The first
        // connection it accepts is the one from above.
        let hanging_up = thread::spawn(move || {
            for connection in listener.incoming().take(2) {
                drop(connection);
            }
        });
        let Err(error) = StoreHandle::connect(&address, hello(1, 1)) else {
            panic!("connected to a listener that hangs up");
        };
        assert!(matches!(error, StoreError::Io(_)), "{error}");
        hanging_up.join().unwrap();
    }

    #[test]
    fn a_connection_that_sends_garbage_is_dropped_and_the_others_are_served() {
        let directory = tempfile::tempdir().unwrap();
        let (origin, far) = (ChunkPos::new(0, 0), ChunkPos::new(-9, 9));
        for store in stores(directory.path()) {
            let (server, address) = served(&store);
            let west = connect(&address, hello(0, 1));

            // Something that is no hello.
            let mut stranger = TcpStream::connect(&address).unwrap();
            stranger.write_all(&[0, 0, 0, 1, 9]).unwrap();
            assert!(closed(&stranger));

            // A hello, a request, and then something that is no request: one that is
            // cut short, one of no known kind, one of four gigabytes, and one for
            // another kind of server altogether.
            let garbage: [&[u8]; 4] = [
                &[0, 0, 0, 2, 0, 1],
                &[0, 0, 0, 1, 99],
                &[0xFF; 4],
                b"GET / HTTP/1.1\r\n\r\n",
            ];
            for (epoch, garbage) in (1..).zip(garbage) {
                // A higher epoch each time, since the store may not have got to the
                // end of the connection before.
                let mut stranger = greeted(&address, hello(1, epoch));
                let mut built = generator().generate(origin);
                built.set(1, 80 + epoch as i32, 1, blocks::STONE);
                let request = StoreRequest::Save {
                    position: origin,
                    tick: 5,
                    chunk: built,
                };
                wire::blocking::write(&mut stranger, &request).unwrap();
                stranger.write_all(garbage).unwrap();
                assert!(closed(&stranger));
            }
            // The region was given up, after what came before the garbage was done.
            let east = connect(&address, hello(1, 4));
            let mut built = generator().generate(origin);
            built.set(1, 84, 1, blocks::STONE);
            assert_eq!(load(&east, origin), built);

            // One that stops in the middle of a request, and one that never says hello.
            let mut silent = greeted(&address, hello(2, 1));
            silent.write_all(&[0, 0, 1, 0, 1, 2, 3]).unwrap();
            let mute = TcpStream::connect(&address).unwrap();

            // The others are served all along.
            assert!(!west.is_lost() && !east.is_lost());
            assert_eq!(load(&west, far), generator().generate(far));
            save(&east, origin, &edited());
            assert_eq!(load(&east, origin), edited());

            // Neither of the two keeps the server from stopping.
            server.stop();
            assert!(closed(&silent) && closed(&mute));
        }
    }

    /// A chunk no two sections of which are alike, and in which hardly a block is like
    /// the one next to it.
    fn motley(seed: u32) -> Chunk {
        let mut chunk = generator().generate(ChunkPos::new(0, 0));
        let mut blocks = seed;
        for y in chunk.min_y()..chunk.min_y() + chunk.height() as i32 {
            for z in 0..16 {
                for x in 0..16 {
                    blocks = blocks.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let state = (blocks >> 8) % BLOCK_STATE_COUNT;
                    chunk.set(x, y, z, BlockState(state as u16));
                }
            }
        }
        chunk
    }

    #[test]
    fn a_chunk_with_every_section_different_survives_the_trip() {
        let directory = tempfile::tempdir().unwrap();
        let chunks = [1, 2].map(|x| (ChunkPos::new(x, 0), motley(x as u32)));
        for (position, chunk) in &chunks {
            let sections = chunk.sections();
            assert!((1..sections.len()).all(|index| !sections[..index].contains(&sections[index])));
            // A few hundred kilobytes on the way to the store, and as many on the way back.
            let request = StoreRequest::Save {
                position: *position,
                tick: 5,
                chunk: chunk.clone(),
            };
            let mut bytes = Vec::new();
            wire::blocking::write(&mut bytes, &request).unwrap();
            assert!(bytes.len() > 200_000, "{} bytes", bytes.len());
        }

        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let remote = connect(&address, hello(1, 1));
            // One after the other without a pause, with small messages in between.
            for (position, chunk) in &chunks {
                save(&remote, *position, chunk);
                remote.request(StoreRequest::Load {
                    position: *position,
                });
            }
            for (position, chunk) in &chunks {
                let loaded = StoreReply::Loaded {
                    position: *position,
                    chunk: chunk.clone(),
                };
                assert_eq!(reply(&remote), loaded);
            }

            // What arrived at the store is what was sent.
            let local = open(&store, hello(0, 1));
            for (position, chunk) in &chunks {
                assert_eq!(load(&local, *position), *chunk);
            }
        }
    }

    /// A store of the test's own: it welcomes the hello of the one connection it accepts,
    /// restores the region with nothing, and leaves the rest to `then`.
    fn fake_store(then: impl FnOnce(TcpStream) + Send + 'static) -> (String, JoinHandle<()>) {
        welcoming_store(|mut connection| {
            let nothing = RestoredPart {
                pieces: Vec::new(),
                last: true,
            };
            wire::blocking::write(&mut connection, &nothing).unwrap();
            then(connection);
        })
    }

    /// A store of the test's own: it welcomes the hello of the one connection it accepts
    /// and leaves the rest to `then`, beginning with what the region is restored with.
    fn welcoming_store(then: impl FnOnce(TcpStream) + Send + 'static) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let store = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let said: Option<RegionHello> = wire::blocking::read(&mut connection).unwrap();
            assert_eq!(said, Some(hello(1, 1)));
            let welcome = StoreWelcome::Accepted {
                entity_ids: EntityIds::block(0).unwrap(),
            };
            wire::blocking::write(&mut connection, &welcome).unwrap();
            then(connection);
        });
        (address, store)
    }

    #[test]
    fn a_handle_is_lost_when_its_store_hangs_up_or_talks_nonsense() {
        let origin = ChunkPos::new(0, 0);
        // A store that goes away over a request, as one does whose process dies.
        let (address, store) = fake_store(move |mut connection| {
            let asked: Option<StoreRequest> = wire::blocking::read(&mut connection).unwrap();
            assert_eq!(asked, Some(StoreRequest::Load { position: origin }));
        });
        let remote = connect(&address, hello(1, 1));
        remote.request(StoreRequest::Load { position: origin });
        // Returns although it is not answered, and the handle is lost by then.
        remote.flush();
        assert!(remote.is_lost());
        save(&remote, origin, &edited());
        remote.flush();
        assert_eq!(remote.try_reply(), None);
        store.join().unwrap();

        // An answer, and after it something that is none.
        let (address, store) = fake_store(|mut connection| {
            wire::blocking::write(&mut connection, &StoreReply::Flushed).unwrap();
            connection.write_all(&[0, 0, 0, 1, 99]).unwrap();
            // The handle gives the connection up.
            assert!(closed(&connection));
        });
        let remote = connect(&address, hello(1, 1));
        lost(&remote);
        remote.flush();
        assert_eq!(remote.try_reply(), None);
        store.join().unwrap();
    }

    /// The system takes a few megabytes for a connection that nobody reads from. What is
    /// asked for beyond that waits, but not whoever asks.
    #[test]
    fn asking_does_not_wait_for_a_store_that_does_not_read() {
        let (close, closing) = mpsc::channel::<()>();
        let (address, store) = fake_store(move |connection| {
            let _ = closing.recv();
            drop(connection);
        });
        let remote = connect(&address, hello(1, 1));
        let chunk = motley(0);
        let (done, asked) = mpsc::channel();
        let asking = thread::spawn(move || {
            for x in 0..40 {
                save(&remote, ChunkPos::new(x, 0), &chunk);
            }
            let answer = remote.try_reply();
            let _ = done.send((remote, answer));
        });
        let (remote, answer) = asked
            .recv_timeout(Duration::from_secs(30))
            .expect("asking waited for the store");
        assert_eq!(answer, None);
        assert!(!remote.is_lost());
        asking.join().unwrap();

        drop(remote);
        close.send(()).unwrap();
        store.join().unwrap();
    }

    /// The thread that writes the answers of a connection waits for as long as nobody
    /// reads them.
    #[test]
    fn a_connection_that_does_not_read_its_answers_keeps_nobody_else_waiting() {
        let (origin, far) = (ChunkPos::new(0, 0), ChunkPos::new(-9, 9));
        let store = Store::memory(generator());
        let (server, address) = served(&store);
        let west = connect(&address, hello(0, 1));

        // Some ten megabytes of answers, which is more than the system takes.
        let mut deaf = greeted(&address, hello(1, 1));
        let request = StoreRequest::Save {
            position: origin,
            tick: 5,
            chunk: motley(0),
        };
        wire::blocking::write(&mut deaf, &request).unwrap();
        for _ in 0..40 {
            wire::blocking::write(&mut deaf, &StoreRequest::Load { position: origin }).unwrap();
        }

        assert_eq!(load(&west, far), generator().generate(far));
        save(&west, far, &edited());
        assert_eq!(load(&west, far), edited());
        // Time for the answers to pile up.
        thread::sleep(Duration::from_millis(100));
        assert!(!west.is_lost());
        server.stop();
        lost(&west);
    }

    const MEBIBYTE: usize = 1024 * 1024;

    /// The most a message may hold.
    const LIMIT: usize = wire::MAX_MESSAGE_LENGTH as usize;

    /// `length` bytes that depend on `seed` and hardly repeat, so that pieces which are
    /// lost, doubled or out of order show.
    fn bytes(seed: u64, length: usize) -> Vec<u8> {
        (0..length as u64)
            .map(|at| (at ^ (at >> 8) ^ (at >> 16)).wrapping_add(seed) as u8)
            .collect()
    }

    /// Has an owner in the store's own process, with `epoch`, commit to region 1: a
    /// state of `state` bytes if that is given, and after it a delta of each of the
    /// lengths in `deltas`. Returns what the store restores the region with in its own
    /// process after that, which is opened with the next epoch for it.
    fn filled(store: &Store, epoch: u64, state: Option<usize>, deltas: &[usize]) -> Restored {
        let owner = open(store, hello(1, epoch));
        log(&owner, 1, &[]);
        if let Some(length) = state {
            owner.request(StoreRequest::Checkpoint {
                tick: 1,
                state: bytes(1, length),
            });
        }
        let mut tick = 1;
        for length in deltas {
            tick += 1;
            owner.request(StoreRequest::Commit {
                tick,
                changes: Vec::new(),
                state: bytes(tick, *length),
            });
        }
        committed(&owner, tick);
        owner.flush();
        drop(owner);

        let (owner, restored) = store.open_region(hello(1, epoch + 1)).unwrap();
        drop(owner);
        let state_length = restored.state.as_ref().map(|state| state.state.len());
        assert_eq!(state_length, state);
        let lengths = restored.deltas.iter().map(|delta| delta.state.len());
        // Without a checkpoint the commit of tick 1 is still among them.
        let skipped = usize::from(state.is_none());
        assert_eq!(lengths.skip(skipped).collect::<Vec<_>>(), deltas);
        restored
    }

    /// The number of bytes `part` is on the connection.
    fn sent_length(part: &RestoredPart) -> usize {
        let mut sent = Vec::new();
        wire::blocking::write(&mut sent, part).unwrap();
        sent.len()
    }

    /// Puts together what [`Parts`] made.
    fn together(entity_ids: EntityIds, parts: Vec<RestoredPart>) -> io::Result<Restored> {
        let mut arriving = Arriving::new(entity_ids);
        let mut parts = parts.into_iter();
        loop {
            let part = parts.next().expect("the last part ends them");
            match arriving.add(part)? {
                Ok(restored) => {
                    assert_eq!(parts.next(), None, "a part after the last");
                    return Ok(restored);
                }
                Err(more) => arriving = more,
            }
        }
    }

    fn tick_state(tick: u64, length: usize) -> TickState {
        TickState {
            tick,
            state: bytes(tick, length),
        }
    }

    #[test]
    fn what_a_region_is_restored_with_is_cut_into_bounded_parts_that_fit_together_again() {
        let entity_ids = EntityIds::block(2).unwrap();
        let restored = |state: Option<usize>, deltas: &[usize]| Restored {
            entity_ids,
            state: state.map(|length| tick_state(7, length)),
            deltas: (8..)
                .zip(deltas)
                .map(|(tick, length)| tick_state(tick, *length))
                .collect(),
        };
        // Next to nothing thousands of times over, with and without bytes at all.
        let tiny: Vec<usize> = (0..3000).map(|index| index % 5).collect();
        let cases = [
            restored(None, &[]),
            restored(Some(0), &[]),
            restored(Some(3), &[4, 0, 5]),
            restored(None, &[0]),
            restored(None, &tiny),
            // A state and deltas that are larger than a part, and one that is as large
            // as a part to the byte.
            restored(
                Some(2500),
                &[1, 3000, 0, 1000 - PIECE_OVERHEAD, 976, 977, 2],
            ),
        ];
        for room in [0, 30, 100, 1000, PART_BYTES] {
            for case in &cases {
                let parts: Vec<_> =
                    Parts::new(case.state.clone(), case.deltas.clone(), room).collect();
                let room = room.max(PIECE_OVERHEAD + 1);
                for (index, part) in parts.iter().enumerate() {
                    // Besides what is counted, a part has its length, the number of its
                    // pieces and whether it is the last.
                    assert!(sent_length(part) <= room + 16, "{}", sent_length(part));
                    assert_eq!(part.last, index == parts.len() - 1);
                    // Only what a part had no room for is cut, and it ends the part.
                    for (at, piece) in part.pieces.iter().enumerate() {
                        assert!(piece.complete || at == part.pieces.len() - 1);
                    }
                }
                // No more parts than it takes: every part but the last is full.
                let counted = |part: &RestoredPart| {
                    let bytes: usize = part.pieces.iter().map(|piece| piece.bytes.len()).sum();
                    bytes + part.pieces.len() * PIECE_OVERHEAD
                };
                for part in &parts[..parts.len() - 1] {
                    assert!(counted(part) + PIECE_OVERHEAD >= room, "{}", counted(part));
                }
                assert_eq!(together(entity_ids, parts).unwrap(), *case);
            }
        }

        // The same with what goes into a message as it is served: a state and a delta
        // of several parts each.
        let large = restored(Some(3 * PART_BYTES + 5), &[3, 5 * PART_BYTES / 2, 0]);
        let parts: Vec<_> =
            Parts::new(large.state.clone(), large.deltas.clone(), PART_BYTES).collect();
        assert!(parts.len() > 5);
        for part in &parts {
            assert!(sent_length(part) <= PART_BYTES + 16);
        }
        assert_eq!(together(entity_ids, parts).unwrap(), large);
    }

    #[test]
    fn parts_that_do_not_fit_together_are_an_error() {
        let entity_ids = EntityIds::block(2).unwrap();
        let piece = |of, tick, complete| RestoredPiece {
            of,
            tick,
            bytes: vec![1, 2],
            complete,
        };
        let (state, delta) = (RestoredItem::State, RestoredItem::Delta);
        let disorders = [
            // A piece that goes on with another tick, or with a state as a delta.
            vec![piece(delta, 4, false), piece(delta, 5, true)],
            vec![piece(state, 4, false), piece(delta, 4, true)],
            // A second state, and a state after a delta.
            vec![piece(state, 4, true), piece(state, 5, true)],
            vec![piece(delta, 4, true), piece(state, 5, true)],
            // An end in the middle of a delta.
            vec![piece(delta, 4, true), piece(delta, 5, false)],
        ];
        for pieces in disorders {
            // Whether they come in one part or in one each.
            let one = vec![RestoredPart {
                pieces: pieces.clone(),
                last: true,
            }];
            let last = pieces.len() - 1;
            let each = pieces
                .into_iter()
                .enumerate()
                .map(|(index, piece)| RestoredPart {
                    pieces: vec![piece],
                    last: index == last,
                });
            for parts in [one, each.collect()] {
                let error = together(entity_ids, parts.clone()).unwrap_err();
                assert_eq!(error.kind(), ErrorKind::InvalidData, "{parts:?}");
            }
        }

        // In order, the same pieces are a state and a delta.
        let parts = vec![
            RestoredPart {
                pieces: vec![piece(state, 4, false)],
                last: false,
            },
            RestoredPart {
                pieces: vec![piece(state, 4, true), piece(delta, 5, false)],
                last: false,
            },
            RestoredPart {
                pieces: vec![piece(delta, 5, true)],
                last: true,
            },
        ];
        let expected = Restored {
            entity_ids,
            state: Some(TickState {
                tick: 4,
                state: vec![1, 2, 1, 2],
            }),
            deltas: vec![TickState {
                tick: 5,
                state: vec![1, 2, 1, 2],
            }],
        };
        assert_eq!(together(entity_ids, parts).unwrap(), expected);
    }

    #[test]
    fn a_region_restored_with_far_more_than_a_message_holds_is_opened_over_a_connection() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            // A state and a delta that are each more than a message holds, deltas that
            // are each more than a part holds, and thousands of next to nothing.
            let mut deltas = vec![LIMIT + 3];
            deltas.extend([MEBIBYTE * 3 / 2; 4]);
            deltas.extend((0..3000).map(|index| index % 40));
            let expected = filled(&store, 1, Some(LIMIT + MEBIBYTE), &deltas);
            let held: usize = expected.deltas.iter().map(|delta| delta.state.len()).sum();
            assert!(held > LIMIT + 6 * MEBIBYTE, "{held}");

            let (remote, restored) = StoreHandle::connect(&address, hello(1, 3)).unwrap();
            // Not `assert_eq`, which would print all of it.
            assert!(restored == expected);

            // The region is the owner's from then on, and nothing it was restored with
            // is taken for an answer.
            assert_eq!(remote.try_reply(), None);
            let tick = expected.tick() + 1;
            log(&remote, tick, &[]);
            remote.request(StoreRequest::Load { position: origin });
            remote.request(StoreRequest::Flush);
            let mut answers = vec![any_reply(&remote), any_reply(&remote), any_reply(&remote)];
            // The commit and the load are answered in whichever order they are done.
            let loaded = StoreReply::Loaded {
                position: origin,
                chunk: generator().generate(origin),
            };
            assert_eq!(answers.pop(), Some(StoreReply::Flushed));
            assert!(answers.contains(&StoreReply::Committed { tick }));
            assert!(answers.contains(&loaded));
            assert_eq!(remote.try_reply(), None);
            assert!(!remote.is_lost());
        }
    }

    #[test]
    fn a_region_restored_with_little_or_nothing_is_opened_over_a_connection() {
        // Nothing at all: a region that is opened for the first time.
        let directory = tempfile::tempdir().unwrap();
        for store in stores(directory.path()) {
            let (_server, address) = served(&store);
            let (remote, restored) = StoreHandle::connect(&address, hello(0, 1)).unwrap();
            assert_eq!((&restored.state, &restored.deltas), (&None, &Vec::new()));
            drop(remote);
            let (_local, locally) = store.open_region(hello(0, 2)).unwrap();
            assert_eq!(restored, locally);
        }

        // A few deltas, one of them without a byte: without a state, after one, and
        // after one that is empty itself.
        for state in [None, Some(4), Some(0)] {
            let directory = tempfile::tempdir().unwrap();
            for store in stores(directory.path()) {
                let (_server, address) = served(&store);
                let expected = filled(&store, 1, state, &[3, 0, 7]);
                let (remote, restored) = StoreHandle::connect(&address, hello(1, 3)).unwrap();
                assert_eq!(restored, expected);
                remote.flush();
                assert_eq!(remote.try_reply(), None);
            }
        }
    }

    #[test]
    fn connecting_fails_when_the_store_does_not_get_to_the_end_of_what_a_region_is_restored_with() {
        let first = || RestoredPart {
            pieces: vec![
                RestoredPiece {
                    of: RestoredItem::State,
                    tick: 3,
                    bytes: vec![7; 100],
                    complete: true,
                },
                RestoredPiece {
                    of: RestoredItem::Delta,
                    tick: 4,
                    bytes: vec![8; 100],
                    complete: false,
                },
            ],
            last: false,
        };
        let failed = |connected: Result<(StoreHandle, Restored), StoreError>, kind| {
            let Err(error) = connected else {
                panic!("a region was restored with a part of what it is restored with");
            };
            assert!(
                matches!(&error, StoreError::Io(error) if error.kind() == kind),
                "{error}"
            );
        };

        // A store that goes away after the first part, as one does whose process dies.
        let (address, store) = welcoming_store(move |mut connection| {
            wire::blocking::write(&mut connection, &first()).unwrap();
        });
        failed(
            StoreHandle::connect(&address, hello(1, 1)),
            ErrorKind::UnexpectedEof,
        );
        store.join().unwrap();

        // One that goes away in the middle of a part.
        let (address, store) = welcoming_store(move |mut connection| {
            wire::blocking::write(&mut connection, &first()).unwrap();
            let mut second = Vec::new();
            wire::blocking::write(&mut second, &first()).unwrap();
            connection.write_all(&second[..second.len() / 2]).unwrap();
        });
        failed(
            StoreHandle::connect(&address, hello(1, 1)),
            ErrorKind::UnexpectedEof,
        );
        store.join().unwrap();

        // One that says it is done in the middle of a delta, and one that sends
        // something that is no part.
        let (address, store) = welcoming_store(move |mut connection| {
            let last = RestoredPart {
                last: true,
                ..first()
            };
            wire::blocking::write(&mut connection, &last).unwrap();
            assert!(closed(&connection));
        });
        failed(
            StoreHandle::connect(&address, hello(1, 1)),
            ErrorKind::InvalidData,
        );
        store.join().unwrap();
        let (address, store) = welcoming_store(move |mut connection| {
            wire::blocking::write(&mut connection, &first()).unwrap();
            connection.write_all(&[0xFF; 4]).unwrap();
            assert!(closed(&connection));
        });
        failed(
            StoreHandle::connect(&address, hello(1, 1)),
            ErrorKind::InvalidData,
        );
        store.join().unwrap();

        // One that stops in the middle and stays: it is not waited for for ever.
        let (close, closing) = mpsc::channel::<()>();
        let (address, store) = welcoming_store(move |mut connection| {
            wire::blocking::write(&mut connection, &first()).unwrap();
            let _ = closing.recv();
        });
        failed(
            connect_within(&address, hello(1, 1), Duration::from_millis(300)),
            ErrorKind::TimedOut,
        );
        close.send(()).unwrap();
        store.join().unwrap();
    }

    #[test]
    fn a_region_whose_owner_went_away_while_it_was_restored_is_opened_again_with_everything() {
        let directory = tempfile::tempdir().unwrap();
        let origin = ChunkPos::new(0, 0);
        for store in stores(directory.path()) {
            let (server, address) = served(&store);
            // More than the system takes for a connection that nobody reads from.
            let expected = filled(&store, 1, Some(2 * MEBIBYTE), &[MEBIBYTE; 10]);

            // An owner that goes away with the first part, and one that goes away with
            // the welcome alone.
            for parts in [1, 0] {
                let mut connection = TcpStream::connect(&address).unwrap();
                wire::blocking::write(&mut connection, &hello(1, 3)).unwrap();
                let welcome = wire::blocking::read(&mut connection).unwrap();
                let accepted = StoreWelcome::Accepted {
                    entity_ids: expected.entity_ids,
                };
                assert_eq!(welcome, Some(accepted));
                for _ in 0..parts {
                    let first = part(&mut connection);
                    assert!(!first.last && !first.pieces.is_empty());
                }
                drop(connection);
                // The store gives up on it, and its region with it.
                quiet(&server);
            }

            // The same owner comes back with its epoch and is given all of it.
            let (remote, restored) = StoreHandle::connect(&address, hello(1, 3)).unwrap();
            assert!(restored == expected);
            assert_eq!(load(&remote, origin), generator().generate(origin));
            let tick = expected.tick() + 1;
            log(&remote, tick, &[]);
            committed(&remote, tick);
            assert!(!remote.is_lost());
        }
    }

    /// The store's side of a connection whose other side, which is returned with it,
    /// reads nothing.
    #[test]
    fn an_owner_that_stops_reading_what_its_region_is_restored_with_is_given_up_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let deaf = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        // Far more than the system takes for a connection that nobody reads from.
        let restored = Restored {
            entity_ids: EntityIds::block(0).unwrap(),
            state: None,
            deltas: vec![TickState {
                tick: 1,
                state: vec![0; 4 * LIMIT],
            }],
        };
        let error = welcome(&stream, Duration::from_millis(300), restored).unwrap_err();
        // Which of the two a write that ran out of time reports depends on the system.
        assert!(
            matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "{error}"
        );
        drop(deaf);
    }
}
