//! The files the store keeps things in, behind a trait: the local file system, or a
//! simulated one in memory that can be made to fail and to crash.
//!
//! Nothing written is durable until it is synced, and a file that was created, renamed
//! or removed is not durably so until its directory is synced too. The simulated disk
//! holds the store to exactly that, which is what lets the tests kill the store at every
//! point and look at what a machine that lost power would find.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// What the store does with files. Paths name files; directories are made with
/// [`Disk::create_dir_all`] and otherwise only synced and listed.
pub(crate) trait Disk: Send + Sync {
    /// The content of the file, or `None` if there is no such file.
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>>;
    /// `length` bytes of the file from `offset` on.
    fn read_at(&self, path: &Path, offset: u64, length: usize) -> io::Result<Vec<u8>>;
    fn exists(&self, path: &Path) -> io::Result<bool>;
    /// The names of the files in `directory`; none if there is no such directory.
    fn list(&self, directory: &Path) -> io::Result<Vec<String>>;
    fn create_dir_all(&self, directory: &Path) -> io::Result<()>;
    /// Creates the file, or replaces what is in it, with `contents`.
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
    /// Appends to the file, creating it if there is none.
    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
    fn truncate(&self, path: &Path, length: u64) -> io::Result<()>;
    /// Makes what was written to the file durable.
    fn sync(&self, path: &Path) -> io::Result<()>;
    /// Makes the files that were created, renamed or removed in `directory` durably so.
    fn sync_directory(&self, directory: &Path) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Removes the file; one that is not there is not an error.
    fn remove(&self, path: &Path) -> io::Result<()>;
}

/// Puts `contents` in the file at `path`, so that the file is either as it was or has
/// all of `contents`: it is written under another name, made durable and renamed. It is
/// durably there once its directory is synced.
pub(crate) fn replace(disk: &dyn Disk, path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    disk.write(&temporary, contents)?;
    // On disk before it takes the place of what was there, or a crash could leave an
    // empty file under the final name.
    disk.sync(&temporary)?;
    disk.rename(&temporary, path)
}

/// The directory a stored file is in.
pub(crate) fn parent(path: &Path) -> &Path {
    path.parent().expect("stored files are inside the world")
}

/// The local file system.
#[derive(Default)]
pub(crate) struct OsDisk {
    /// Files that are appended to, kept open so that a commit does not open its log.
    appending: Mutex<BTreeMap<PathBuf, Appended>>,
}

/// A file that is appended to.
#[derive(Clone)]
struct Appended {
    file: Arc<File>,
    /// Whether the file was cut back since it was last synced. Its length having shrunk
    /// is metadata that syncing its data need not write.
    truncated: bool,
}

impl OsDisk {
    fn appending(&self) -> MutexGuard<'_, BTreeMap<PathBuf, Appended>> {
        // Whoever holds the lock only adds, removes or marks a file, so the map is in
        // order even after a panic.
        self.appending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The open file at `path` if it is appended to.
    fn appended(&self, path: &Path) -> Option<Appended> {
        self.appending().get(path).cloned()
    }
}

impl Disk for OsDisk {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_at(&self, path: &Path, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        path.try_exists()
    }

    fn list(&self, directory: &Path) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    fn create_dir_all(&self, directory: &Path) -> io::Result<()> {
        fs::create_dir_all(directory)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.appending().remove(path);
        File::create(path)?.write_all(contents)
    }

    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let file = match self.appended(path) {
            Some(appended) => appended.file,
            None => {
                let file = OpenOptions::new().create(true).append(true).open(path)?;
                let file = Arc::new(file);
                let appended = Appended {
                    file: Arc::clone(&file),
                    truncated: false,
                };
                self.appending().insert(path.to_owned(), appended);
                file
            }
        };
        (&*file).write_all(contents)
    }

    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        let mut appending = self.appending();
        match appending.get_mut(path) {
            Some(appended) => {
                // Noted even if cutting fails: it may have been done in part.
                appended.truncated = true;
                appended.file.set_len(length)
            }
            None => OpenOptions::new().write(true).open(path)?.set_len(length),
        }
    }

    fn sync(&self, path: &Path) -> io::Result<()> {
        match self.appended(path) {
            // The length changes with every append, and is part of what is synced.
            Some(Appended {
                file,
                truncated: false,
            }) => file.sync_data(),
            // A length that has shrunk is not: all of the file's metadata is synced,
            // so that what was cut off is no part of the file after a crash.
            Some(Appended {
                file,
                truncated: true,
            }) => {
                file.sync_all()?;
                if let Some(appended) = self.appending().get_mut(path) {
                    appended.truncated = false;
                }
                Ok(())
            }
            None => File::open(path)?.sync_all(),
        }
    }

    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        File::open(directory)?.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        {
            let mut appending = self.appending();
            appending.remove(from);
            appending.remove(to);
        }
        fs::rename(from, to)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.appending().remove(path);
        match fs::remove_file(path) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// A file system in memory. It keeps apart what is written and what is durable, so that
/// [`MemoryDisk::crashed`] can say what a crash would leave, and it can be told to fail.
#[derive(Default)]
pub(crate) struct MemoryDisk {
    state: Mutex<Memory>,
}

#[derive(Default)]
struct Memory {
    /// The content of each file, by a number that a rename keeps.
    files: BTreeMap<u64, Content>,
    /// The files by name, as seen now.
    names: BTreeMap<PathBuf, u64>,
    /// The files by name, as a crash would leave them: as of the last sync of each
    /// directory.
    durable: BTreeMap<PathBuf, u64>,
    next: u64,
    /// How many times something was changed or synced.
    operations: u64,
    fault: Option<Fault>,
}

#[derive(Default)]
struct Content {
    data: Vec<u8>,
    /// What a crash would leave of `data`.
    synced: Vec<u8>,
    /// What the file had before it was first cut back since it was last synced, if it
    /// was: what a crash leaves if cutting it back never reached the disk.
    untruncated: Option<Vec<u8>>,
}

/// Something going wrong at the `n`th change or sync, counted from 1.
// Only tests make faults happen.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// That one fails; an append or a write puts half of what it was given in place
    /// first. What follows works again.
    Fail(u64),
    /// As many as the second number says fail, from that one on, each as
    /// [`Fault::Fail`] makes one fail. What follows them works again.
    Fails(u64, u64),
    /// That one and everything after it fails without doing anything, as if the
    /// machine had stopped there.
    Stop(u64),
}

/// What a crash leaves of what was not durable yet.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Survival {
    /// Nothing.
    Nothing,
    /// Of every file, half of what was appended to it since it was synced; of names,
    /// nothing.
    Torn,
    /// All of it, as if the system had written everything out just before.
    Everything,
    /// All of it but that files were cut back: a file that was cut back and not synced
    /// since is as it was before. It is what a machine finds whose truncation never
    /// reached the disk.
    Untruncated,
}

impl MemoryDisk {
    /// A disk that gets the fault.
    #[cfg(test)]
    pub(crate) fn failing(fault: Fault) -> Self {
        let disk = Self::default();
        disk.memory().fault = Some(fault);
        disk
    }

    /// The disk with the fault, counted from what is changed or synced from now on.
    #[cfg(test)]
    pub(crate) fn with(self, fault: Fault) -> Self {
        {
            let mut memory = self.memory();
            memory.fault = Some(fault);
            memory.operations = 0;
        }
        self
    }

    fn memory(&self) -> MutexGuard<'_, Memory> {
        // Every change is made in full under the lock; a panic in a test elsewhere
        // leaves nothing half done.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many times something was changed or synced so far.
    #[cfg(test)]
    pub(crate) fn operations(&self) -> u64 {
        self.memory().operations
    }

    /// What a machine that crashed now would find on restarting.
    #[cfg(test)]
    pub(crate) fn crashed(&self, survival: Survival) -> MemoryDisk {
        let memory = self.memory();
        let names = match survival {
            Survival::Everything | Survival::Untruncated => &memory.names,
            Survival::Nothing | Survival::Torn => &memory.durable,
        };
        let mut left = Memory::default();
        for (path, file) in names {
            let content = &memory.files[file];
            let data = match survival {
                Survival::Everything => content.data.clone(),
                Survival::Untruncated => content
                    .untruncated
                    .clone()
                    .unwrap_or_else(|| content.data.clone()),
                Survival::Nothing => content.synced.clone(),
                Survival::Torn => {
                    let mut data = content.synced.clone();
                    if let Some(appended) = content.data.strip_prefix(&content.synced[..]) {
                        data.extend_from_slice(&appended[..appended.len() / 2]);
                    }
                    data
                }
            };
            left.next += 1;
            let number = left.next;
            left.files.insert(
                number,
                Content {
                    synced: data.clone(),
                    data,
                    untruncated: None,
                },
            );
            left.names.insert(path.clone(), number);
            left.durable.insert(path.clone(), number);
        }
        MemoryDisk {
            state: Mutex::new(left),
        }
    }
}

impl Memory {
    /// Counts a change or a sync and says whether it is to fail, and if so whether it
    /// is to do half of its work first.
    fn operate(&mut self) -> Result<(), Failure> {
        self.operations += 1;
        match self.fault {
            Some(Fault::Fail(n)) if self.operations == n => Err(Failure::Partly),
            Some(Fault::Fails(n, count)) if self.operations >= n && self.operations - n < count => {
                Err(Failure::Partly)
            }
            Some(Fault::Stop(n)) if self.operations >= n => Err(Failure::Entirely),
            _ => Ok(()),
        }
    }

    fn content(&mut self, path: &Path) -> io::Result<&mut Content> {
        let number = *self.names.get(path).ok_or_else(missing)?;
        Ok(self
            .files
            .get_mut(&number)
            .expect("every name is of a file"))
    }

    fn create(&mut self, path: &Path) -> &mut Content {
        self.next += 1;
        let number = self.next;
        self.names.insert(path.to_owned(), number);
        self.files.entry(number).or_default()
    }
}

/// How an operation of a [`MemoryDisk`] fails.
enum Failure {
    Partly,
    Entirely,
}

fn missing() -> io::Error {
    io::Error::new(ErrorKind::NotFound, "no such file")
}

fn injected() -> io::Error {
    io::Error::other("a fault was injected")
}

impl Disk for MemoryDisk {
    fn read(&self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        let mut memory = self.memory();
        match memory.content(path) {
            Ok(content) => Ok(Some(content.data.clone())),
            Err(_) => Ok(None),
        }
    }

    fn read_at(&self, path: &Path, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let mut memory = self.memory();
        let data = &memory.content(path)?.data;
        let start = offset as usize;
        data.get(start..start + length)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "beyond the end"))
    }

    fn exists(&self, path: &Path) -> io::Result<bool> {
        Ok(self.memory().names.contains_key(path))
    }

    fn list(&self, directory: &Path) -> io::Result<Vec<String>> {
        let memory = self.memory();
        Ok(memory
            .names
            .keys()
            .filter(|path| path.parent() == Some(directory))
            .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
            .collect())
    }

    fn create_dir_all(&self, _directory: &Path) -> io::Result<()> {
        // Directories are there for any file that is in them.
        Ok(())
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut memory = self.memory();
        let failure = memory.operate();
        if let Err(Failure::Entirely) = failure {
            return Err(injected());
        }
        let content = memory.create(path);
        match failure {
            Ok(()) => content.data = contents.to_vec(),
            Err(_) => {
                content.data = contents[..contents.len() / 2].to_vec();
                return Err(injected());
            }
        }
        Ok(())
    }

    fn append(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut memory = self.memory();
        let failure = memory.operate();
        if let Err(Failure::Entirely) = failure {
            return Err(injected());
        }
        let content = match memory.names.contains_key(path) {
            true => memory.content(path)?,
            false => memory.create(path),
        };
        match failure {
            Ok(()) => content.data.extend_from_slice(contents),
            Err(_) => {
                content
                    .data
                    .extend_from_slice(&contents[..contents.len() / 2]);
                return Err(injected());
            }
        }
        Ok(())
    }

    fn truncate(&self, path: &Path, length: u64) -> io::Result<()> {
        let mut memory = self.memory();
        memory.operate().map_err(|_| injected())?;
        let content = memory.content(path)?;
        let length = length as usize;
        if length < content.data.len() && content.untruncated.is_none() {
            content.untruncated = Some(content.data.clone());
        }
        content.data.truncate(length);
        Ok(())
    }

    fn sync(&self, path: &Path) -> io::Result<()> {
        let mut memory = self.memory();
        memory.operate().map_err(|_| injected())?;
        let content = memory.content(path)?;
        content.synced = content.data.clone();
        content.untruncated = None;
        Ok(())
    }

    fn sync_directory(&self, directory: &Path) -> io::Result<()> {
        let mut memory = self.memory();
        memory.operate().map_err(|_| injected())?;
        let Memory { names, durable, .. } = &mut *memory;
        durable.retain(|path, _| path.parent() != Some(directory));
        for (path, number) in names.iter() {
            if path.parent() == Some(directory) {
                durable.insert(path.clone(), *number);
            }
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let mut memory = self.memory();
        memory.operate().map_err(|_| injected())?;
        let number = memory.names.remove(from).ok_or_else(missing)?;
        memory.names.insert(to.to_owned(), number);
        Ok(())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        let mut memory = self.memory();
        memory.operate().map_err(|_| injected())?;
        memory.names.remove(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        Path::new("/world").join(name)
    }

    #[test]
    fn what_was_not_synced_is_lost_in_a_crash() {
        let disk = MemoryDisk::default();
        disk.append(&path("log"), b"abcd").unwrap();
        disk.sync(&path("log")).unwrap();
        disk.sync_directory(Path::new("/world")).unwrap();
        disk.append(&path("log"), b"efgh").unwrap();
        // Created and synced, but its name was never made durable.
        disk.write(&path("other"), b"x").unwrap();
        disk.sync(&path("other")).unwrap();

        let read = |disk: &MemoryDisk, name| disk.read(&path(name)).unwrap();
        let nothing = disk.crashed(Survival::Nothing);
        assert_eq!(read(&nothing, "log").unwrap(), b"abcd");
        assert_eq!(read(&nothing, "other"), None);
        let torn = disk.crashed(Survival::Torn);
        assert_eq!(read(&torn, "log").unwrap(), b"abcdef");
        let everything = disk.crashed(Survival::Everything);
        assert_eq!(read(&everything, "log").unwrap(), b"abcdefgh");
        assert_eq!(read(&everything, "other").unwrap(), b"x");
    }

    #[test]
    fn a_rename_is_durable_once_its_directory_is_synced() {
        let disk = MemoryDisk::default();
        replace(&disk, &path("state"), b"old").unwrap();
        disk.sync_directory(Path::new("/world")).unwrap();
        replace(&disk, &path("state"), b"new").unwrap();
        let read = |disk: &MemoryDisk| disk.read(&path("state")).unwrap().unwrap();
        assert_eq!(read(&disk), b"new");
        assert_eq!(read(&disk.crashed(Survival::Nothing)), b"old");
        disk.sync_directory(Path::new("/world")).unwrap();
        assert_eq!(read(&disk.crashed(Survival::Nothing)), b"new");
    }

    #[test]
    fn faults_come_at_the_operation_they_were_set_for() {
        let disk = MemoryDisk::failing(Fault::Fail(2));
        disk.append(&path("log"), b"ab").unwrap();
        assert!(disk.append(&path("log"), b"cdef").is_err());
        disk.append(&path("log"), b"g").unwrap();
        assert_eq!(disk.read(&path("log")).unwrap().unwrap(), b"abcdg");

        let disk = MemoryDisk::failing(Fault::Stop(2));
        disk.append(&path("log"), b"ab").unwrap();
        assert!(disk.append(&path("log"), b"cd").is_err());
        assert!(disk.sync(&path("log")).is_err());
        assert_eq!(disk.read(&path("log")).unwrap().unwrap(), b"ab");
        assert_eq!(disk.operations(), 3);
    }

    /// Cutting a file back is not durable before the file is synced: until then a crash
    /// can leave it as it was before, with whatever was cut off.
    #[test]
    fn a_truncation_is_durable_once_the_file_is_synced() {
        let disk = MemoryDisk::default();
        let log = path("log");
        let read = |disk: &MemoryDisk| disk.read(&path("log")).unwrap().unwrap();
        disk.append(&log, b"abcd").unwrap();
        disk.sync(&log).unwrap();
        disk.sync_directory(Path::new("/world")).unwrap();
        disk.append(&log, b"efgh").unwrap();
        disk.truncate(&log, 4).unwrap();
        assert_eq!(read(&disk), b"abcd");
        assert_eq!(read(&disk.crashed(Survival::Everything)), b"abcd");
        assert_eq!(read(&disk.crashed(Survival::Untruncated)), b"abcdefgh");
        assert_eq!(read(&disk.crashed(Survival::Nothing)), b"abcd");

        // What was there before the first cut counts, not what a second one found.
        disk.truncate(&log, 2).unwrap();
        assert_eq!(read(&disk.crashed(Survival::Untruncated)), b"abcdefgh");
        disk.sync(&log).unwrap();
        for survival in [
            Survival::Nothing,
            Survival::Torn,
            Survival::Everything,
            Survival::Untruncated,
        ] {
            assert_eq!(read(&disk.crashed(survival)), b"ab", "{survival:?}");
        }
        // Cutting to the length the file has, or a greater one, notes nothing.
        disk.truncate(&log, 2).unwrap();
        disk.truncate(&log, 9).unwrap();
        disk.append(&log, b"x").unwrap();
        assert_eq!(read(&disk.crashed(Survival::Untruncated)), b"abx");
        // A file that was never cut back is as with everything kept, name and all.
        disk.write(&path("other"), b"y").unwrap();
        let left = disk.crashed(Survival::Untruncated);
        assert_eq!(left.read(&path("other")).unwrap().unwrap(), b"y");
        // What a crash left has nothing cut back that could come back.
        left.truncate(&log, 1).unwrap();
        left.sync(&log).unwrap();
        assert_eq!(read(&left.crashed(Survival::Untruncated)), b"a");
    }

    #[test]
    fn several_faults_in_a_row_come_where_they_were_set_and_then_end() {
        let disk = MemoryDisk::failing(Fault::Fails(2, 3));
        let log = path("log");
        disk.append(&log, b"ab").unwrap();
        // Each fails as a single fault does: an append puts half in place first.
        assert!(disk.append(&log, b"cdef").is_err());
        assert!(disk.truncate(&log, 2).is_err());
        assert!(disk.sync(&log).is_err());
        assert_eq!(disk.read(&log).unwrap().unwrap(), b"abcd");
        assert_eq!(disk.crashed(Survival::Nothing).read(&log).unwrap(), None);
        disk.truncate(&log, 2).unwrap();
        disk.sync(&log).unwrap();
        assert_eq!(disk.read(&log).unwrap().unwrap(), b"ab");
        assert_eq!(disk.operations(), 6);

        // One fault is several of which there is one.
        for fault in [Fault::Fail(2), Fault::Fails(2, 1)] {
            let disk = MemoryDisk::failing(fault);
            disk.append(&log, b"ab").unwrap();
            assert!(disk.append(&log, b"cdef").is_err());
            disk.append(&log, b"g").unwrap();
            assert_eq!(disk.read(&log).unwrap().unwrap(), b"abcdg");
        }
        let disk = MemoryDisk::failing(Fault::Fails(1, 0));
        disk.append(&log, b"ab").unwrap();
    }

    /// That the length of a file that was cut back is durable after a sync cannot be
    /// seen on a real file system without taking its power away. What can be seen is
    /// that a file that is appended to is cut back and synced, more than once, in the
    /// way that writes its length out, and that appending goes on after it.
    #[test]
    fn the_local_file_system_cuts_back_and_syncs_a_file_it_appends_to() {
        let directory = tempfile::tempdir().unwrap();
        let disk = OsDisk::default();
        let log = directory.path().join("log");
        disk.append(&log, b"abcdef").unwrap();
        disk.sync(&log).unwrap();
        for length in [4, 2] {
            disk.truncate(&log, length).unwrap();
            assert!(disk.appended(&log).unwrap().truncated);
            disk.sync(&log).unwrap();
            assert!(!disk.appended(&log).unwrap().truncated);
            assert_eq!(fs::metadata(&log).unwrap().len(), length);
        }
        disk.append(&log, b"x").unwrap();
        disk.sync(&log).unwrap();
        assert_eq!(disk.read(&log).unwrap().unwrap(), b"abx");
    }

    #[test]
    fn the_local_file_system_does_what_the_memory_does() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let disks: [(Box<dyn Disk>, &Path); 2] = [
            (Box::new(OsDisk::default()), root),
            (Box::new(MemoryDisk::default()), Path::new("/world")),
        ];
        for (disk, root) in disks {
            let log = root.join("log");
            disk.create_dir_all(root).unwrap();
            assert_eq!(disk.read(&log).unwrap(), None);
            disk.append(&log, b"abc").unwrap();
            disk.append(&log, b"def").unwrap();
            disk.sync(&log).unwrap();
            disk.truncate(&log, 4).unwrap();
            disk.append(&log, b"x").unwrap();
            assert_eq!(disk.read(&log).unwrap().unwrap(), b"abcdx");
            assert_eq!(disk.read_at(&log, 1, 3).unwrap(), b"bcd");
            replace(disk.as_ref(), &root.join("state"), b"s").unwrap();
            disk.sync_directory(root).unwrap();
            assert_eq!(disk.list(root).unwrap(), ["log", "state"]);
            disk.rename(&log, &root.join("moved")).unwrap();
            assert!(!disk.exists(&log).unwrap());
            disk.remove(&root.join("moved")).unwrap();
            disk.remove(&root.join("moved")).unwrap();
            assert_eq!(disk.list(root).unwrap(), ["state"]);
            assert_eq!(disk.list(&root.join("none")).unwrap(), Vec::<String>::new());
        }
    }
}
