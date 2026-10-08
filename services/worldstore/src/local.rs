//! A world in a directory of the local file system.
//!
//! ```text
//! meta                 what the world was made with
//! layout, log/, regions/
//!                      the log and the files of the regions; see `lanes.rs`
//! blobs/, manifests/   the stored chunks; see `chunks.rs`
//! ```
//!
//! A world from before regions had a state has a log per region in `logs/`, or a single
//! one, `wal`, from before there were regions at all. Those hold block changes alone,
//! which are put into the stored chunks once when such a world is opened, after which
//! the old logs are removed.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::sync::Arc;

use clustine_data::DATA_VERSION;
use clustine_format::{FORMAT_VERSION, LogRecord, read_log};
use clustine_world::ChunkGenerator;
use tracing::info;

use crate::StoreError;
use crate::chunks::{Chunks, FileChunks, apply};
use crate::disk::{Disk, replace};

/// Creates the world in `root` if there is none, checks that the one there was made
/// with what this server uses, and carries a world from before regions had a state
/// over.
pub(crate) fn prepare(
    disk: &Arc<dyn Disk>,
    root: &Path,
    generator: &dyn ChunkGenerator,
) -> Result<(), StoreError> {
    fs::create_dir_all(root)?;
    let current = [
        ("format", FORMAT_VERSION.to_string()),
        ("data-version", DATA_VERSION.to_string()),
        ("generator", generator.settings()),
    ];

    let meta = root.join("meta");
    match fs::read_to_string(&meta) {
        Ok(text) => {
            for (setting, current) in current {
                let stored = text
                    .lines()
                    .find_map(|line| line.strip_prefix(setting)?.strip_prefix('='))
                    .ok_or_else(|| StoreError::MalformedMeta(format!("no {setting}")))?;
                if stored != current {
                    return Err(StoreError::Incompatible {
                        setting,
                        stored: stored.to_owned(),
                        current,
                    });
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let text: String = current
                .iter()
                .map(|(setting, value)| format!("{setting}={value}\n"))
                .collect();
            replace(disk.as_ref(), &meta, text.as_bytes())?;
            disk.sync_directory(root)?;
        }
        Err(error) => return Err(error.into()),
    }
    carry_over(disk, root, generator)
}

/// Puts what the logs of a world from before regions had a state hold into the stored
/// chunks, and removes those logs.
fn carry_over(
    disk: &Arc<dyn Disk>,
    root: &Path,
    generator: &dyn ChunkGenerator,
) -> Result<(), StoreError> {
    let single = root.join("wal");
    let directory = root.join("logs");
    let mut logs = Vec::new();
    if single.try_exists()? {
        logs.push(single);
    }
    let mut per_region: Vec<_> = disk
        .list(&directory)?
        .into_iter()
        .filter(|name| name.ends_with(".wal"))
        .map(|name| directory.join(name))
        .collect();
    per_region.sort();
    logs.extend(per_region);
    if logs.is_empty() && !directory.try_exists()? {
        return Ok(());
    }

    let mut chunks = FileChunks::new(Arc::clone(disk), root);
    let mut changes = Vec::new();
    for log in &logs {
        let bytes = disk.read(log)?.unwrap_or_default();
        // What follows the records that could be read was being written when the
        // process died.
        let (records, _) = read_log(&bytes)?;
        for record in records {
            if let LogRecord::Changes {
                changes: changed, ..
            } = record
            {
                changes.extend(changed);
            }
        }
    }
    apply(&mut chunks, generator, &changes, 0)?;
    // In the stored chunks for good before the logs go, or a crash in between would
    // lose them.
    chunks.sync()?;
    for log in &logs {
        disk.remove(log)?;
    }
    disk.sync_directory(root)?;
    match fs::remove_dir_all(&directory) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    info!(
        logs = logs.len(),
        changes = changes.len(),
        "carried a world from before regions had a state over"
    );
    Ok(())
}
