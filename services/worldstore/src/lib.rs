//! World store service: serves and persists chunks, snapshots and write-ahead logs.
//!
//! Nothing is persisted yet: every chunk comes from the generator. The store already
//! runs on its own thread and is spoken to through messages, so that slow storage can
//! never delay a tick and so that it can later live in another process.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use clustine_rpc::{StoreReply, StoreRequest};
use clustine_world::ChunkGenerator;

/// A running store. It stops when the handle is dropped.
pub struct StoreHandle {
    requests: Sender<StoreRequest>,
    replies: Receiver<StoreReply>,
}

impl StoreHandle {
    /// Queues a request. The answer arrives later through [`StoreHandle::try_reply`].
    pub fn request(&self, request: StoreRequest) {
        // The store only stops when this handle is dropped, so it is still there.
        let _ = self.requests.send(request);
    }

    /// Returns the next answer if one is ready, without blocking.
    pub fn try_reply(&self) -> Option<StoreReply> {
        self.replies.try_recv().ok()
    }
}

/// Starts a store that serves every chunk from `generator`.
pub fn spawn(generator: Arc<dyn ChunkGenerator>) -> StoreHandle {
    let (request_sender, request_receiver) = mpsc::channel();
    let (reply_sender, reply_receiver) = mpsc::channel();
    thread::Builder::new()
        .name("worldstore".to_owned())
        .spawn(move || {
            for request in request_receiver {
                let reply = match request {
                    StoreRequest::Load { position } => StoreReply::Loaded {
                        position,
                        chunk: generator.generate(position),
                    },
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
