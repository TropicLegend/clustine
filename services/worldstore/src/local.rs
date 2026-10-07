//! A world in a directory of the local file system.
//!
//! ```text
//! meta                                       what the world was made with
//! blobs/ab/abcdef…                           sections, by the hash of their content
//! manifests/overworld/<rx>.<rz>/<x>.<z>.manifest
//!                                            chunks, grouped by 32×32 chunks
//! ```
//!
//! Files are written under a temporary name and renamed, so a reader never sees half a
//! file. Section files are never changed once written.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use clustine_data::DATA_VERSION;
use clustine_format::{ChunkManifest, FORMAT_VERSION, Hash, pack, unpack};
use clustine_world::{Chunk, ChunkPos};

use crate::{Backend, StoreError};

pub(crate) struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    /// Opens the world in `root`, or creates one made with `generator_settings`.
    pub(crate) fn open(root: &Path, generator_settings: &str) -> Result<Self, StoreError> {
        fs::create_dir_all(root.join("blobs"))?;
        fs::create_dir_all(root.join("manifests"))?;
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
        })
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
}

/// Writes a file so that it is either there in full or not at all.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let directory = path.parent().expect("stored files are inside the world");
    fs::create_dir_all(directory)?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, contents)?;
    fs::rename(&temporary, path)
}
