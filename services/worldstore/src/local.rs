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
use clustine_world::{BlockPos, ChunkGenerator};
use tracing::info;

use crate::StoreError;
use crate::disk::{Disk, replace};

/// The line of `meta` that names the highest version of the generator that made a
/// chunk the world stores (`docs/adr/0021-generation-is-a-function.md`, section 8).
const GENERATOR_VERSION: &str = "generator-version";

/// The line of `meta` that holds the world's spawn, `x,y,z`: where the generator put it
/// when the world was made (`docs/groundwork/t8-plan.md`, T8c).
const SPAWN: &str = "spawn";

/// A world's `meta` as the store keeps it, to raise its `generator-version` when the
/// first chunk of a newer generator is about to be stored (ADR-0021, section 6).
pub(crate) struct Meta {
    path: PathBuf,
    /// The lines of the file as they are.
    lines: Vec<String>,
    /// The version the file names.
    pub(crate) version: u16,
    /// The world's spawn, which the file holds.
    pub(crate) spawn: BlockPos,
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

/// Whether two identities of generators are land of one seed made by different
/// generators of Clustine (the preview and the land with features, or the land with
/// features at two steps of phase F), rather than worlds of two seeds.
fn same_seed(stored: &str, current: &str) -> bool {
    let seed = |identity: &str| {
        identity
            .starts_with("terrain;")
            .then(|| identity.split(';').find(|part| part.starts_with("seed ")))
            .flatten()
            .map(str::to_owned)
    };
    seed(stored).is_some() && seed(stored) == seed(current)
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
                if setting == "generator" && stored != current && same_seed(stored, &current) {
                    return Err(StoreError::EarlierGenerator {
                        stored: stored.to_owned(),
                        current,
                        directory: root.to_owned(),
                    });
                }
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
            let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
            let spawn = match text
                .lines()
                .find_map(|line| line.strip_prefix(SPAWN)?.strip_prefix('='))
            {
                Some(spawn) => parse_spawn(spawn).ok_or_else(|| {
                    StoreError::MalformedMeta(format!("{SPAWN} is not three whole numbers"))
                })?,
                None => {
                    // A world made before a world kept its spawn, whose players entered on
                    // the column at the origin. Where the game's rule leaves them there it
                    // is kept and the spawn written down, by the same durable replacement
                    // as a raised version: a crash before it leaves the file as it was,
                    // and the rule gives the same spawn again.
                    let spawn = generator.spawn();
                    if (spawn.x, spawn.z) != (0, 0) {
                        return Err(StoreError::SpawnWouldMove {
                            spawn,
                            directory: root.to_owned(),
                        });
                    }
                    lines.push(spawn_line(spawn));
                    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
                    replace(disk.as_ref(), &path, text.as_bytes())?;
                    disk.sync_directory(root)?;
                    info!(
                        x = spawn.x,
                        y = spawn.y,
                        z = spawn.z,
                        "the world's meta now holds its spawn, which the game's rule puts where its players entered"
                    );
                    spawn
                }
            };
            Ok(Meta {
                path,
                lines,
                version,
                spawn,
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            // Before anything is written: a crash while the generator works leaves no
            // `meta`, so the world is new again, and its spawn is the same.
            let spawn = generator.spawn();
            let mut lines: Vec<String> = current
                .iter()
                .map(|(setting, value)| format!("{setting}={value}"))
                .collect();
            lines.push(format!("{GENERATOR_VERSION}={}", settings.version));
            lines.push(spawn_line(spawn));
            let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
            replace(disk.as_ref(), &path, text.as_bytes())?;
            disk.sync_directory(root)?;
            Ok(Meta {
                path,
                lines,
                version: settings.version,
                spawn,
            })
        }
        Err(error) => Err(error.into()),
    }
}

fn spawn_line(spawn: BlockPos) -> String {
    format!("{SPAWN}={},{},{}", spawn.x, spawn.y, spawn.z)
}

fn parse_spawn(text: &str) -> Option<BlockPos> {
    let mut numbers = text.split(',').map(|number| number.parse::<i32>().ok());
    let spawn = BlockPos::new(numbers.next()??, numbers.next()??, numbers.next()??);
    numbers.next().is_none().then_some(spawn)
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
        Store::local_told(directory, generator, division())
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
    fn a_new_world_keeps_its_spawn_in_meta_and_a_newer_generator_keeps_it() {
        let directory = tempfile::tempdir().unwrap();
        let somewhere = BlockPos::new(-37, 70, 1204);
        let generator = Arc::new(FlatGenerator::classic().with_spawn(somewhere));
        let store = Store::local_divided(directory.path(), generator, crate::Pins::none());
        let store = store.unwrap();
        assert_eq!(store.regions().unwrap().spawn, somewhere);
        drop(store);
        assert!(meta(directory.path()).contains("\nspawn=-37,70,1204\n"));

        // A server whose generator would put the spawn elsewhere opens the world with
        // the spawn it was made with, and the home chunk is that spawn's.
        let elsewhere = Arc::new(FlatGenerator::classic());
        let store = Store::local_divided(directory.path(), elsewhere, crate::Pins::none());
        let list = store.unwrap().regions().unwrap();
        assert_eq!(list.spawn, somewhere);
    }

    #[test]
    fn a_world_from_before_the_spawn_was_kept_is_given_it_where_its_players_entered() {
        let directory = tempfile::tempdir().unwrap();
        drop(local(directory.path(), Versioned::new(1)).unwrap());
        // As the world's meta was before it held a spawn.
        let earlier: String = meta(directory.path())
            .lines()
            .filter(|line| !line.starts_with("spawn="))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(directory.path().join("meta"), &earlier).unwrap();

        // The flat world's spawn is on the column at the origin, where its players
        // entered: it is written down and the world opens.
        let store = local(directory.path(), Versioned::new(1)).unwrap();
        assert_eq!(store.regions().unwrap().spawn, BlockPos::new(0, -60, 0));
        drop(store);
        assert!(meta(directory.path()).ends_with("spawn=0,-60,0\n"));

        // A world whose spawn the game puts elsewhere is refused, and nothing in it
        // is changed.
        fs::write(directory.path().join("meta"), &earlier).unwrap();
        let away = BlockPos::new(-96, 66, 32);
        let generator = Arc::new(FlatGenerator::classic().with_spawn(away));
        let Err(error) = Store::local_divided(directory.path(), generator, crate::Pins::none())
        else {
            panic!("a world whose spawn would move was opened");
        };
        assert!(matches!(error, StoreError::SpawnWouldMove { spawn, .. } if spawn == away));
        assert!(error.to_string().contains("-96, 66, 32"), "{error}");
        assert_eq!(meta(directory.path()), earlier);
    }

    #[test]
    fn a_spawn_in_meta_that_is_not_three_numbers_is_malformed() {
        let directory = tempfile::tempdir().unwrap();
        drop(local(directory.path(), Versioned::new(1)).unwrap());
        for broken in ["0,-60", "0,-60,0,1", "0,high,0", ""] {
            let text: String = meta(directory.path())
                .lines()
                .map(|line| match line.starts_with("spawn=") {
                    true => format!("spawn={broken}\n"),
                    false => format!("{line}\n"),
                })
                .collect();
            fs::write(directory.path().join("meta"), text).unwrap();
            let Err(error) = local(directory.path(), Versioned::new(1)) else {
                panic!("{broken:?} was read as a spawn");
            };
            assert!(
                matches!(error, StoreError::MalformedMeta(_)),
                "{broken:?}: {error}"
            );
        }
    }

    /// The flat world, which notes what a store asks it to make ahead.
    #[derive(Default)]
    struct Preparing {
        asked: std::sync::Mutex<Vec<Vec<ChunkPos>>>,
    }

    impl ChunkGenerator for Preparing {
        fn generate(&self, position: ChunkPos) -> Chunk {
            FlatGenerator::classic().generate(position)
        }

        fn settings(&self) -> GeneratorSettings {
            FlatGenerator::classic().settings()
        }

        fn prepare(&self, positions: &[ChunkPos]) {
            self.asked.lock().unwrap().push(positions.to_vec());
        }
    }

    #[test]
    fn a_store_that_starts_has_the_chunks_around_the_spawn_made_ahead_unless_stored() {
        let directory = tempfile::tempdir().unwrap();
        let generator = Arc::new(Preparing::default());
        let start = || {
            let store =
                Store::local_divided(directory.path(), generator.clone(), crate::Pins::none());
            let store = store.unwrap();
            // The chunk thread takes the store's jobs in order, so what it was asked
            // when the store started is done once the store is at rest.
            store.flush().unwrap();
            store
        };
        let distance = |chunk: &ChunkPos| i64::from(chunk.x).pow(2) + i64::from(chunk.z).pow(2);

        let store = start();
        let first = generator.asked.lock().unwrap().concat();
        // A view of 8, the chunk beyond it a client is sent, and a region's ring.
        assert_eq!(
            first.len(),
            21 * 21,
            "a view of 8 and two around the spawn's chunk"
        );
        assert_eq!(first[0], ChunkPos::new(0, 0));
        assert!(
            first
                .windows(2)
                .all(|pair| distance(&pair[0]) <= distance(&pair[1]))
        );
        assert!(
            first
                .iter()
                .all(|chunk| chunk.x.abs() <= 10 && chunk.z.abs() <= 10)
        );

        // The home chunk is stored, and is read from its file from then on: the next
        // start does not have it made.
        let handle = open(&store, hello(0, 1));
        crate::tests::save(&handle, ChunkPos::new(0, 0), &crate::tests::edited());
        handle.flush();
        drop(handle);
        store.flush().unwrap();
        drop(store);
        generator.asked.lock().unwrap().clear();
        drop(start());
        let second = generator.asked.lock().unwrap().concat();
        assert_eq!(second.len(), 21 * 21 - 1);
        assert!(!second.contains(&ChunkPos::new(0, 0)));
    }

    /// The flat generator's chunks under another identity.
    struct Named(&'static str);

    impl ChunkGenerator for Named {
        fn generate(&self, position: ChunkPos) -> Chunk {
            FlatGenerator::classic().generate(position)
        }

        fn settings(&self) -> GeneratorSettings {
            GeneratorSettings {
                identity: self.0.to_owned(),
                version: 1,
            }
        }
    }

    #[test]
    fn land_of_its_seed_made_by_another_generator_of_clustine_is_refused_with_a_sentence_of_its_own()
     {
        let directory = tempfile::tempdir().unwrap();
        let preview = "terrain;preview;minecraft:overworld;seed 5";
        drop(Store::local(directory.path(), Arc::new(Named(preview))).unwrap());
        let before = meta(directory.path());
        let features = "terrain;features 1;minecraft:overworld;seed 5";
        let Err(error) = Store::local(directory.path(), Arc::new(Named(features))) else {
            panic!("the preview's world was opened by the land with features");
        };
        assert_eq!(
            error.to_string(),
            format!(
                "the world in {} was made by another generator of Clustine (`{preview}`) than \
                 this server's (`{features}`), and worlds are not carried over before the first \
                 release: move the directory away, or name another with --world, and a new world \
                 is made",
                directory.path().display()
            )
        );
        assert_eq!(meta(directory.path()), before);

        // Another seed is another world, and is told so as before.
        let other = "terrain;preview;minecraft:overworld;seed 6";
        let Err(error) = Store::local(directory.path(), Arc::new(Named(other))) else {
            panic!("the world was opened with another seed");
        };
        assert!(matches!(error, StoreError::Incompatible { .. }), "{error}");
    }

    #[test]
    fn identities_are_of_one_seed_only_where_both_are_land_and_name_the_same_seed() {
        assert!(same_seed(
            "terrain;features 1;minecraft:overworld;seed 5",
            "terrain;features 2;minecraft:overworld;seed 5;no structures"
        ));
        assert!(!same_seed(
            "terrain;preview;minecraft:overworld;seed 5",
            "terrain;preview;minecraft:overworld;seed 50"
        ));
        assert!(!same_seed("flat;classic", "flat;classic"));
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
                spawn: crate::tests::SPAWN,
                path: root.join("meta"),
                lines: text.lines().map(str::to_owned).collect(),
                version: 1,
            };
            let mut chunks = FileChunks::new(disk.clone(), root).with_meta(meta);
            let chunk = Versioned::new(2).generate(position);
            chunks.handed(chunk.generated());
            chunks
                .note(position, &chunk, clustine_rpc::Parts::ALL)
                .unwrap();
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
                parts: clustine_rpc::Parts::ALL,
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
