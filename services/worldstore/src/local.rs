//! A world in a directory of the local file system.
//!
//! ```text
//! meta                                       what the world was made with
//! logs/<region>.wal                          per region, block changes not yet in a
//!                                            saved chunk
//! blobs/ab/abcdef…                           sections, by the hash of their content
//! manifests/overworld/<rx>.<rz>/<x>.<z>.manifest
//!                                            chunks, grouped by 32×32 chunks
//! ```
//!
//! Files are written under a temporary name and renamed, so a reader never sees half a
//! file. Section files are never changed once written. Only the logs are appended to.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use clustine_data::DATA_VERSION;
use clustine_format::{BlockChanges, ChunkManifest, FORMAT_VERSION, Hash, pack, read_log, unpack};
use clustine_region::RegionId;
use clustine_world::{Chunk, ChunkPos};

use crate::{Backend, StoreError};

pub(crate) struct LocalFs {
    root: PathBuf,
    /// The write-ahead logs of the regions that have been opened.
    logs: BTreeMap<RegionId, RegionLog>,
}

/// The write-ahead log of a region.
struct RegionLog {
    /// Opened for appending.
    file: File,
    /// Whether something has been appended to the log since it was last made durable.
    unsynced: bool,
}

impl LocalFs {
    /// Opens the world in `root`, or creates one made with `generator_settings`.
    pub(crate) fn open(root: &Path, generator_settings: &str) -> Result<Self, StoreError> {
        fs::create_dir_all(root.join("blobs"))?;
        fs::create_dir_all(root.join("manifests"))?;
        fs::create_dir_all(root.join("logs"))?;
        let current = [
            ("format", FORMAT_VERSION.to_string()),
            ("data-version", DATA_VERSION.to_string()),
            ("generator", generator_settings.to_owned()),
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
                write_atomically(&meta, text.as_bytes())?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            root: root.to_owned(),
            logs: BTreeMap::new(),
        })
    }

    /// Hands the records of every log the world was left with to `apply`, which is to
    /// put their changes into saved chunks, and empties the log once that is done.
    ///
    /// The logs are those of the regions and `wal` in the root, which is the one log of
    /// a world that was last opened before it could have several regions. That file is
    /// removed.
    pub(crate) fn drain_logs(
        &mut self,
        mut apply: impl FnMut(&mut Self, &[BlockChanges]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let old = self.root.join("wal");
        if old.try_exists()? {
            self.drain_log(&old, &mut apply)?;
            fs::remove_file(&old)?;
        }

        let mut logs = Vec::new();
        for entry in fs::read_dir(self.root.join("logs"))? {
            let entry = entry?;
            let path = entry.path();
            let is_log = path.extension().is_some_and(|extension| extension == "wal");
            if is_log && entry.file_type()?.is_file() {
                logs.push(path);
            }
        }
        logs.sort();
        for log in logs {
            self.drain_log(&log, &mut apply)?;
        }
        Ok(())
    }

    /// Hands the records of the log at `path` to `apply` and then empties the log. What
    /// is left of a record that was only written in part, because the process died while
    /// appending it, goes with the rest.
    fn drain_log(
        &mut self,
        path: &Path,
        apply: &mut impl FnMut(&mut Self, &[BlockChanges]) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let bytes = fs::read(path)?;
        if bytes.is_empty() {
            return Ok(());
        }
        let (records, _) = read_log(&bytes)?;
        if !records.is_empty() {
            apply(self, &records)?;
        }
        let log = OpenOptions::new().write(true).open(path)?;
        log.set_len(0)?;
        // On disk before anything else happens to the chunks, so that the changes are
        // never applied again on top of later ones.
        log.sync_data()?;
        Ok(())
    }

    fn log_path(&self, region: RegionId) -> PathBuf {
        self.root.join("logs").join(format!("{region}.wal"))
    }

    /// The log of `region`, which is created if the region has none yet.
    fn region_log(&mut self, region: RegionId) -> Result<&mut RegionLog, StoreError> {
        let path = self.log_path(region);
        match self.logs.entry(region) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let file = OpenOptions::new().create(true).append(true).open(path)?;
                Ok(entry.insert(RegionLog {
                    file,
                    unsynced: false,
                }))
            }
        }
    }

    fn blob_path(&self, hash: &Hash) -> PathBuf {
        let name = hash.to_string();
        self.root.join("blobs").join(&name[..2]).join(name)
    }

    fn manifest_path(&self, position: ChunkPos) -> PathBuf {
        self.root
            .join("manifests/overworld")
            .join(format!("{}.{}", position.x >> 5, position.z >> 5))
            .join(format!("{}.{}.manifest", position.x, position.z))
    }
}

impl Backend for LocalFs {
    fn load(&mut self, position: ChunkPos) -> Result<Option<Chunk>, StoreError> {
        let bytes = match fs::read(self.manifest_path(position)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let manifest = ChunkManifest::decode(&bytes)?;
        let chunk = manifest.restore(|hash| -> Result<Vec<u8>, StoreError> {
            Ok(unpack(&fs::read(self.blob_path(hash))?)?)
        })?;
        Ok(Some(chunk))
    }

    fn save(&mut self, position: ChunkPos, tick: u64, chunk: &Chunk) -> Result<(), StoreError> {
        let (manifest, sections) = ChunkManifest::describe(position, chunk, tick);
        // Sections first: a manifest must never point at a section that is not there.
        for (hash, canonical) in sections {
            let path = self.blob_path(&hash);
            if !path.exists() {
                write_atomically(&path, &pack(&canonical))?;
            }
        }
        write_atomically(&self.manifest_path(position), &manifest.encode())?;
        Ok(())
    }

    fn log(&mut self, region: RegionId, changes: &BlockChanges) -> Result<(), StoreError> {
        let log = self.region_log(region)?;
        log.file.write_all(&changes.encode())?;
        log.unsynced = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), StoreError> {
        for log in self.logs.values_mut() {
            if log.unsynced {
                log.file.sync_data()?;
                log.unsynced = false;
            }
        }
        Ok(())
    }

    fn checkpoint(&mut self, region: RegionId) -> Result<(), StoreError> {
        let log = self.region_log(region)?;
        log.file.set_len(0)?;
        log.file.sync_data()?;
        log.unsynced = false;
        Ok(())
    }

    /// A record that was only written in part is cut off.
    fn pending(&mut self, region: RegionId) -> Result<Vec<BlockChanges>, StoreError> {
        let path = self.log_path(region);
        let log = self.region_log(region)?;
        let bytes = fs::read(path)?;
        let (records, valid) = read_log(&bytes)?;
        if valid < bytes.len() {
            // So that what is appended from now on follows a complete record.
            log.file.set_len(valid as u64)?;
            log.file.sync_data()?;
        }
        Ok(records)
    }
}

/// Writes a file so that it is either there in full or not at all.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let directory = path.parent().expect("stored files are inside the world");
    fs::create_dir_all(directory)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let mut file = File::create(&temporary)?;
    file.write_all(contents)?;
    // On disk before it takes the place of what was there, or a crash of the machine
    // could leave an empty file under the final name.
    file.sync_all()?;
    fs::rename(&temporary, path)
}
