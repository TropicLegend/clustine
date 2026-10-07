//! World store service: serves and persists chunks, snapshots and write-ahead logs.
//!
//! The store runs on its own thread and is spoken to through messages, so that slow
//! storage can never delay a tick and so that it can later live in another process.
//!
//! Only chunks that were changed are stored. Any other chunk is generated again when it
//! is needed, which is why a world is tied to the generator settings it was created with.

mod local;

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clustine_format::FormatError;
use clustine_rpc::{StoreReply, StoreRequest};
use clustine_world::{Chunk, ChunkGenerator, ChunkPos};
use tracing::error;

use crate::local::LocalFs;

/// Why a world could not be opened, read or written.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Format(#[from] FormatError),
    #[error("the world's metadata is malformed: {0}")]
    MalformedMeta(String),
    #[error("the world was made with {setting} `{stored}`, but this server uses `{current}`")]
    Incompatible {
        setting: &'static str,
        stored: String,
        current: String,
    },
}

/// Where chunks are kept.
trait Backend: Send {
    /// The stored chunk, or `None` if it was never stored.
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError>;
    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError>;
}

/// Keeps chunks in memory: they last as long as the store.
#[derive(Default)]
struct Memory(BTreeMap<ChunkPos, Chunk>);

impl Backend for Memory {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        Ok(self.0.get(&position).cloned())
    }

    fn save(&mut self, position: ChunkPos, _tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        self.0.insert(position, chunk.clone());
        Ok(())
    }
}

/// A running store. It stops when the handle is dropped.
pub struct StoreHandle {
    requests: Sender<StoreRequest>,
    replies: Receiver<StoreReply>,
}

impl StoreHandle {
    /// Queues a request. An answer, if the request has one, arrives later through
    /// [`StoreHandle::try_reply`].
    pub fn request(&self, request: StoreRequest) {
        // The store only stops when this handle is dropped, so it is still there.
        let _ = self.requests.send(request);
    }

    /// Returns the next answer if one is ready, without blocking.
    pub fn try_reply(&self) -> Option<StoreReply> {
        self.replies.try_recv().ok()
    }

    /// Waits until everything requested so far has been done. Answers to earlier load
    /// requests that are still unread are discarded, so this is for shutting down.
    pub fn flush(&self) {
        self.request(StoreRequest::Flush);
        while let Ok(reply) = self.replies.recv() {
            if reply == StoreReply::Flushed {
                return;
            }
        }
    }
}

/// Starts a store that keeps changed chunks in memory only.
pub fn spawn(generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    start(Box::new(Memory::default()), generator)
}

/// Starts a store that keeps changed chunks in the directory `root`, creating the world
/// there if there is none.
pub fn spawn_local(
    root: &Path,
    generator: Arc<dyn ChunkGenerator>,
) -> Result<StoreHandle, StoreError> {
    let backend = LocalFs::open(root, &generator.settings())?;
    Ok(start(Box::new(backend), generator))
}

fn start(mut backend: Box<dyn Backend>, generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    let (request_sender, request_receiver) = mpsc::channel();
    let (reply_sender, reply_receiver) = mpsc::channel();
    thread::Builder::new()
        .name("worldstore".to_owned())
        .spawn(move || {
            for request in request_receiver {
                let reply = match request {
                    StoreRequest::Load { position } => match backend.load(position) {
                        Ok(stored) => StoreReply::Loaded {
                            position,
                            chunk: stored.unwrap_or_else(|| generator.generate(position)),
                        },
                        // Generating the chunk instead would look fine at first and
                        // then overwrite what players built once it is saved. Not
                        // answering leaves a hole in the world that someone can look
                        // into.
                        Err(error) => {
                            error!(?position, %error, "a stored chunk cannot be read");
                            continue;
                        }
                    },
                    StoreRequest::Save {
                        position,
                        tick,
                        chunk,
                    } => {
                        if let Err(error) = backend.save(position, tick, &chunk) {
                            error!(?position, %error, "a chunk could not be stored");
                        }
                        continue;
                    }
                    StoreRequest::Flush => StoreReply::Flushed,
                };
                if reply_sender.send(reply).is_err() {
                    break;
                }
            }
        })
        .expect("spawning the worldstore thread");
    StoreHandle {
        requests: request_sender,
        replies: reply_receiver,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use clustine_data::blocks;
    use clustine_worldgen::FlatGenerator;

    use super::*;

    fn generator() -> Arc<dyn ChunkGenerator> {
        Arc::new(FlatGenerator::classic())
    }

    /// Asks for a chunk and waits for it.
    fn load(store: &StoreHandle, position: ChunkPos) -> Chunk {
        store.request(StoreRequest::Load { position });
        for _ in 0..5000 {
            match store.try_reply() {
                Some(StoreReply::Loaded {
                    position: loaded,
                    chunk,
                }) => {
                    assert_eq!(loaded, position);
                    return chunk;
                }
                Some(StoreReply::Flushed) => panic!("unexpected flush"),
                None => thread::sleep(Duration::from_millis(1)),
            }
        }
        panic!("the store did not answer");
    }

    fn save(store: &StoreHandle, position: ChunkPos, chunk: &Chunk) {
        store.request(StoreRequest::Save {
            position,
            tick: 5,
            chunk: chunk.clone(),
        });
    }

    fn edited() -> Chunk {
        let mut chunk = generator().generate(ChunkPos::new(0, 0));
        chunk.set(3, -61, 4, blocks::AIR);
        chunk.set(3, 100, 4, blocks::GLASS);
        chunk
    }

    #[test]
    fn chunks_that_were_never_stored_are_generated() {
        let directory = tempfile::tempdir().unwrap();
        let stores = [
            spawn(generator()),
            spawn_local(directory.path(), generator()).unwrap(),
        ];
        for store in stores {
            let position = ChunkPos::new(7, -9);
            assert_eq!(load(&store, position), generator().generate(position));
        }
    }

    #[test]
    fn a_saved_chunk_is_what_is_loaded_from_then_on() {
        let directory = tempfile::tempdir().unwrap();
        let stores = [
            spawn(generator()),
            spawn_local(directory.path(), generator()).unwrap(),
        ];
        for store in stores {
            let position = ChunkPos::new(-2, 5);
            save(&store, position, &edited());
            assert_eq!(load(&store, position), edited());
            // Other chunks are not affected.
            let other = ChunkPos::new(-2, 6);
            assert_eq!(load(&store, other), generator().generate(other));
        }
    }

    #[test]
    fn a_world_on_disk_outlives_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let position = ChunkPos::new(100, -100);
        {
            let store = spawn_local(directory.path(), generator()).unwrap();
            save(&store, position, &edited());
            store.flush();
        }
        let store = spawn_local(directory.path(), generator()).unwrap();
        assert_eq!(load(&store, position), edited());
    }

    #[test]
    fn flush_waits_for_everything_before_it() {
        let directory = tempfile::tempdir().unwrap();
        let store = spawn_local(directory.path(), generator()).unwrap();
        for x in 0..50 {
            save(&store, ChunkPos::new(x, 0), &edited());
        }
        // A load that is still unanswered does not confuse the flush.
        store.request(StoreRequest::Load {
            position: ChunkPos::new(0, 0),
        });
        store.flush();
        let manifests = directory.path().join("manifests/overworld");
        let stored: usize = fs::read_dir(manifests)
            .unwrap()
            .map(|region| fs::read_dir(region.unwrap().path()).unwrap().count())
            .sum();
        assert_eq!(stored, 50);
    }

    #[test]
    fn equal_sections_are_stored_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = spawn_local(directory.path(), generator()).unwrap();
        for x in 0..20 {
            save(&store, ChunkPos::new(x, 3), &edited());
        }
        store.flush();
        // The edited chunk has two sections that are not plain air.
        let blobs: usize = fs::read_dir(directory.path().join("blobs"))
            .unwrap()
            .map(|prefix| fs::read_dir(prefix.unwrap().path()).unwrap().count())
            .sum();
        assert_eq!(blobs, 2);
    }

    #[test]
    fn a_world_is_tied_to_its_generator() {
        struct Other;
        impl ChunkGenerator for Other {
            fn generate(&self, position: ChunkPos) -> Chunk {
                FlatGenerator::classic().generate(position)
            }
            fn settings(&self) -> String {
                "something else".to_owned()
            }
        }

        let directory = tempfile::tempdir().unwrap();
        drop(spawn_local(directory.path(), generator()).unwrap());
        // Opening it again with the same generator is fine.
        drop(spawn_local(directory.path(), generator()).unwrap());
        let Err(error) = spawn_local(directory.path(), Arc::new(Other)) else {
            panic!("the world was opened with another generator");
        };
        assert!(
            matches!(
                error,
                StoreError::Incompatible {
                    setting: "generator",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_damaged_chunk_is_not_replaced_by_a_generated_one() {
        let directory = tempfile::tempdir().unwrap();
        let position = ChunkPos::new(1, 1);
        let store = spawn_local(directory.path(), generator()).unwrap();
        save(&store, position, &edited());
        store.flush();

        let manifest = directory
            .path()
            .join("manifests/overworld/0.0/1.1.manifest");
        let mut bytes = fs::read(&manifest).unwrap();
        bytes[10] ^= 0xFF;
        fs::write(&manifest, bytes).unwrap();

        store.request(StoreRequest::Load { position });
        store.flush();
        assert_eq!(store.try_reply(), None, "the damaged chunk was answered");
        // The store carries on with other chunks.
        let other = ChunkPos::new(2, 2);
        assert_eq!(load(&store, other), generator().generate(other));
    }
}
