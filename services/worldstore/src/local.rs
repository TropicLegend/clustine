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
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use clustine_data::DATA_VERSION;
use clustine_format::FORMAT_VERSION;
use clustine_world::ChunkGenerator;

use crate::StoreError;
use crate::disk::{Disk, replace};

/// Creates the world in `root` if there is none, and checks that the one there was
/// made with what this server uses.
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
    Ok(())
}
