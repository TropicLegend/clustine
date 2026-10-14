//! A world in a directory of the local file system.
//!
//! ```text
//! meta                 what the world was made with
//! log/, regions/       the log and the files of the regions; see `lanes.rs`
//! chunks/              the stored chunks, one file each; see `chunks.rs`
//! ```
//!
//! A world of an earlier format is not opened and not made over: worlds are thrown
//! away until the first release, and the store says so of the directory before it
//! touches anything in it (`docs/adr/0022-what-a-chunk-carries.md`, section 9).

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clustine_data::DATA_VERSION;
use clustine_format::FORMAT_VERSION;
use clustine_world::ChunkGenerator;
use tracing::info;

use crate::StoreError;
use crate::disk::{Disk, replace};

/// The line of `meta` that names the highest version of the generator that made a
/// chunk the world stores (`docs/adr/0021-generation-is-a-function.md`, section 8).
const GENERATOR_VERSION: &str = "generator-version";

/// A world's `meta` as the store keeps it, to raise its `generator-version` when the
/// first chunk of a newer generator is about to be stored (ADR-0021, section 6).
pub(crate) struct Meta {
    path: PathBuf,
    /// The lines of the file as they are.
    lines: Vec<String>,
    /// The version the file names.
    pub(crate) version: u16,
}

impl Meta {
    /// Puts `version` in the file in place of the one there, durably: the file is
    /// replaced and synced, and so is its directory, as when the world was made.
    pub(crate) fn raise(&mut self, disk: &dyn Disk, version: u16) -> io::Result<()> {
        let mut lines = self.lines.clone();
        for line in &mut lines {
            if line
                .strip_prefix(GENERATOR_VERSION)
                .is_some_and(|rest| rest.starts_with('='))
            {
                *line = format!("{GENERATOR_VERSION}={version}");
            }
        }
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        replace(disk, &self.path, text.as_bytes())?;
        disk.sync_directory(parent_of(&self.path))?;
        info!(
            from = self.version,
            to = version,
            "the world's meta names the newer generator, which made a chunk it now stores"
        );
        self.lines = lines;
        self.version = version;
        Ok(())
    }
}

fn parent_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}

/// Creates the world in `root` if there is none, and checks that the one there was
/// made with what this server uses. Returns its `meta`.
pub(crate) fn prepare(
    disk: &Arc<dyn Disk>,
    root: &Path,
    generator: &dyn ChunkGenerator,
) -> Result<Meta, StoreError> {
    fs::create_dir_all(root)?;
    let settings = generator.settings();
    let current = [
        ("format", FORMAT_VERSION.to_string()),
        ("data-version", DATA_VERSION.to_string()),
        ("generator", settings.identity),
    ];

    let path = root.join("meta");
    match fs::read_to_string(&path) {
        Ok(text) => {
            let stored = |setting: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(setting)?.strip_prefix('='))
                    .ok_or_else(|| StoreError::MalformedMeta(format!("no {setting}")))
            };
            // Before anything else is looked at: whoever kept a world from an earlier
            // build is told what to do with the directory, and not that some other
            // setting of it differs.
            if let Ok(earlier) = stored("format")?.parse::<u8>()
                && earlier < FORMAT_VERSION
            {
                return Err(StoreError::EarlierFormat {
                    stored: earlier,
                    current: FORMAT_VERSION,
                    directory: root.to_owned(),
                });
            }
            for (setting, current) in current {
                let stored = stored(setting)?;
                if stored != current {
                    return Err(StoreError::Incompatible {
                        setting,
                        stored: stored.to_owned(),
                        current,
                    });
                }
            }
            let version = stored(GENERATOR_VERSION)?.parse::<u16>().map_err(|_| {
                StoreError::MalformedMeta(format!("{GENERATOR_VERSION} is not a version"))
            })?;
            if version > settings.version {
                return Err(StoreError::OlderGenerator {
                    world: version,
                    server: settings.version,
                });
            }
            if version < settings.version {
                // Nothing is changed for it: going back to the older server is
                // possible until a chunk of this one is stored.
                info!(
                    world = version,
                    server = settings.version,
                    "this world's chunks were made by an older version of the generator; chunks made from now on are this version's"
                );
            }
            Ok(Meta {
                path,
                lines: text.lines().map(str::to_owned).collect(),
                version,
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut lines: Vec<String> = current
                .iter()
                .map(|(setting, value)| format!("{setting}={value}"))
                .collect();
            lines.push(format!("{GENERATOR_VERSION}={}", settings.version));
            let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
            replace(disk.as_ref(), &path, text.as_bytes())?;
            disk.sync_directory(root)?;
            Ok(Meta {
                path,
                lines,
                version: settings.version,
            })
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use clustine_rpc::{StoreReply, StoreRequest};
    use clustine_world::generation::GeneratorSettings;
    use clustine_world::{Chunk, ChunkPos};
    use clustine_worldgen::FlatGenerator;

    use super::*;
    use crate::tests::{any_reply, division, hello, open};
    use crate::{Store, StoreHandle};

    /// What the flat generator makes, said to be made by `version`.
    struct Versioned {
        version: u16,
        /// Whether its chunks are marked as made with a failure.
        with_failure: bool,
        /// Where it fails outright.
        fails_at: Option<ChunkPos>,
    }

    impl Versioned {
        fn new(version: u16) -> Arc<Self> {
            Arc::new(Self {
                version,
                with_failure: false,
                fails_at: None,
            })
        }
    }

    impl ChunkGenerator for Versioned {
        fn generate(&self, position: ChunkPos) -> Chunk {
            assert_ne!(Some(position), self.fails_at, "this generator fails here");
            let mut chunk = FlatGenerator::classic().generate(position);
            chunk.mark_generated(self.version, self.with_failure);
            chunk
        }

        fn settings(&self) -> GeneratorSettings {
            GeneratorSettings {
                identity: FlatGenerator::classic().settings().identity,
                version: self.version,
            }
        }
    }

    fn meta(directory: &Path) -> String {
        fs::read_to_string(directory.join("meta")).unwrap()
    }

    fn version_in(directory: &Path) -> String {
        let text = meta(directory);
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix("generator-version="));
        line.expect("meta names the generator's version").to_owned()
    }

    /// Loads a chunk that is not stored, and waits for it.
    fn generated(handle: &StoreHandle, position: ChunkPos) -> Chunk {
        handle.request(StoreRequest::Load { position });
        assert_eq!(any_reply(handle), StoreReply::Generating { position });
        match any_reply(handle) {
            StoreReply::Loaded { chunk, .. } => chunk,
            other => panic!("expected the chunk, got {other:?}"),
        }
    }

    fn local(directory: &Path, generator: Arc<Versioned>) -> Result<Store, StoreError> {
        Store::local_divided(directory, generator, division())
    }

    #[test]
    fn a_new_world_names_the_version_of_the_generator_that_made_it() {
        let directory = tempfile::tempdir().unwrap();
        drop(local(directory.path(), Versioned::new(3)).unwrap());
        assert_eq!(version_in(directory.path()), "3");
        // A line of its own, apart from the generator's identity.
        let identity = FlatGenerator::classic().settings().identity;
        assert!(meta(directory.path()).contains(&format!("\ngenerator={identity}\n")));
    }

    #[test]
    fn a_server_with_an_older_generator_is_refused_with_the_records_sentence() {
        let directory = tempfile::tempdir().unwrap();
        drop(local(directory.path(), Versioned::new(7)).unwrap());
        let before = meta(directory.path());
        let Err(error) = local(directory.path(), Versioned::new(5)) else {
            panic!("the world was opened with an older generator");
        };
        assert_eq!(
            error.to_string(),
            "This world holds chunks made by generator version 7, and this server has \
             version 5. Start it with a server that is at least as new."
        );
        assert_eq!(meta(directory.path()), before);
    }

    /// A newer generator opens the world and changes nothing until a chunk it made is
    /// stored: until then the older one can open the world again.
    #[test]
    fn the_version_in_meta_is_raised_by_the_sync_that_first_stores_a_chunk_of_a_newer_one() {
        let directory = tempfile::tempdir().unwrap();
        let (a, b) = (ChunkPos::new(3, 3), ChunkPos::new(4, 3));
        {
            let store = local(directory.path(), Versioned::new(1)).unwrap();
            let east = open(&store, hello(1, 1));
            assert_eq!(generated(&east, a).generated(), 1);
            east.flush();
        }
        {
            let store = local(directory.path(), Versioned::new(2)).unwrap();
            assert_eq!(version_in(directory.path()), "1");
            let east = open(&store, hello(1, 2));
            assert_eq!(generated(&east, b).generated(), 2);
            // Handed out, not stored: nothing has synced it.
            store.flush().unwrap();
            assert_eq!(version_in(directory.path()), "1");
        }
        // So the older one opens the world, and its chunk is as it was.
        {
            let store = local(directory.path(), Versioned::new(1)).unwrap();
            let east = open(&store, hello(1, 3));
            east.request(StoreRequest::Load { position: a });
            let StoreReply::Loaded { chunk, .. } = any_reply(&east) else {
                panic!("the stored chunk is not answered at once");
            };
            assert_eq!(chunk.generated(), 1);
        }
        {
            let store = local(directory.path(), Versioned::new(2)).unwrap();
            let east = open(&store, hello(1, 4));
            assert_eq!(generated(&east, b).generated(), 2);
            store.close().unwrap();
            assert_eq!(version_in(directory.path()), "2");
            assert!(
                directory
                    .path()
                    .join("chunks/overworld/0.0/4.3.chunk")
                    .exists()
            );
        }
        let Err(error) = local(directory.path(), Versioned::new(1)) else {
            panic!("the world was opened with an older generator after a newer one stored");
        };
        assert!(
            matches!(
                error,
                StoreError::OlderGenerator {
                    world: 2,
                    server: 1
                }
            ),
            "{error}"
        );
    }

    /// Wherever a sync stops, and whatever a crash then keeps, a file that holds a chunk
    /// of the newer version is never there without `meta` naming that version.
    #[test]
    fn no_crash_leaves_a_chunk_of_a_version_above_the_one_meta_names() {
        use crate::chunks::{Chunks, FileChunks};
        use crate::disk::{Fault, MemoryDisk};
        use crate::regions::SURVIVALS;

        let root = Path::new("/world");
        let position = ChunkPos::new(3, 3);
        let text = "format=2\ngenerator=flat\ngenerator-version=1\n";
        let mut synced = false;
        for stop in 1.. {
            let disk = MemoryDisk::default();
            replace(&disk, &root.join("meta"), text.as_bytes()).unwrap();
            disk.sync_directory(root).unwrap();
            let disk = Arc::new(disk.with(Fault::Stop(stop)));
            let meta = Meta {
                path: root.join("meta"),
                lines: text.lines().map(str::to_owned).collect(),
                version: 1,
            };
            let mut chunks = FileChunks::new(disk.clone(), root).with_meta(meta);
            let chunk = Versioned::new(2).generate(position);
            chunks.handed(chunk.generated());
            chunks.note(position, &chunk).unwrap();
            let done = chunks.sync().is_ok();
            for survival in SURVIVALS {
                let left: Arc<dyn Disk> = Arc::new(disk.crashed(survival));
                let mut read = FileChunks::new(Arc::clone(&left), root);
                if read.load(position).unwrap().is_some() {
                    let meta = left.read(&root.join("meta")).unwrap().unwrap();
                    let meta = String::from_utf8(meta).unwrap();
                    assert!(
                        meta.contains("generator-version=2\n"),
                        "stopped at {stop}, {survival:?}: {meta}"
                    );
                }
            }
            if done {
                synced = true;
                break;
            }
        }
        assert!(synced);
    }

    /// S8: a chunk stored as made with a failure, and never changed, is made again by a
    /// newer generator, answered and stored; one that was changed is not; and where the
    /// newer generator fails outright, the stored chunk is answered.
    #[test]
    fn a_chunk_stored_with_a_failure_is_made_again_by_a_newer_generator() {
        let directory = tempfile::tempdir().unwrap();
        let failed = ChunkPos::new(3, 3);
        let changed = ChunkPos::new(4, 3);
        let again = ChunkPos::new(5, 3);
        let older = Arc::new(Versioned {
            version: 1,
            with_failure: true,
            fails_at: None,
        });
        {
            let store = local(directory.path(), older).unwrap();
            let east = open(&store, hello(1, 1));
            assert!(generated(&east, failed).with_failure());
            assert!(generated(&east, again).with_failure());
            let mut chunk = generated(&east, changed);
            chunk.set(1, 100, 1, clustine_data::blocks::GLASS);
            east.request(StoreRequest::Save {
                position: changed,
                tick: 0,
                chunk,
            });
            east.flush();
        }
        let newer = Arc::new(Versioned {
            version: 2,
            with_failure: false,
            fails_at: Some(again),
        });
        let store = local(directory.path(), newer).unwrap();
        let east = open(&store, hello(1, 2));
        let made = generated(&east, failed);
        assert_eq!((made.generated(), made.with_failure()), (2, false));
        // Stored in the old one's place.
        east.request(StoreRequest::Load { position: failed });
        let stored = StoreReply::Loaded {
            position: failed,
            chunk: made,
        };
        assert_eq!(any_reply(&east), stored);

        east.request(StoreRequest::Load { position: changed });
        let StoreReply::Loaded { chunk, .. } = any_reply(&east) else {
            panic!("a changed chunk is made again");
        };
        assert_eq!(chunk.generated(), 0);

        let kept = generated(&east, again);
        assert_eq!((kept.generated(), kept.with_failure()), (1, true));
        store.close().unwrap();
        assert_eq!(version_in(directory.path()), "2");
    }
}
