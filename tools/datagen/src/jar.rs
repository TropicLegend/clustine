//! Obtains the official server jar, runs its built-in data generator, and compiles and
//! runs the extract program against its classes.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};

use crate::sha256;

/// The server jar the committed tables are generated from.
pub struct ServerJar {
    pub version: &'static str,
    pub url: &'static str,
    pub sha1: &'static str,
}

pub const SERVER_JAR: ServerJar = ServerJar {
    version: "26.3",
    url: "https://piston-data.mojang.com/v1/objects/33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c/server.jar",
    sha1: "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c",
};

/// Marks a `generated` directory as the complete output of a successful run.
const STAMP: &str = ".clustine-complete";

/// Returns the path of the verified server jar in `cache`, downloading it if needed.
pub fn fetch(cache: &Path) -> Result<PathBuf> {
    let path = cache.join("server.jar");
    if path.exists() && sha1_of(&path)? == SERVER_JAR.sha1 {
        return Ok(path);
    }

    eprintln!("downloading {}", SERVER_JAR.url);
    fs::create_dir_all(cache)?;
    let partial = cache.join("server.jar.part");
    let mut response = ureq::get(SERVER_JAR.url)
        .call()
        .with_context(|| format!("downloading {}", SERVER_JAR.url))?;
    io::copy(
        &mut response.body_mut().as_reader(),
        &mut File::create(&partial)?,
    )?;
    verify(&partial)?;
    fs::rename(&partial, &path)?;
    Ok(path)
}

/// Fails unless `jar` is the pinned server jar.
pub fn verify(jar: &Path) -> Result<()> {
    let actual = sha1_of(jar)?;
    ensure!(
        actual == SERVER_JAR.sha1,
        "{} has sha1 {actual}, expected {} (Minecraft {})",
        jar.display(),
        SERVER_JAR.sha1,
        SERVER_JAR.version,
    );
    Ok(())
}

/// Runs the data generator of `jar` inside `cache` and returns the output directory.
///
/// The output of an earlier complete run is reused unless `fresh` is set. `jar` must
/// already be verified, so the output only depends on the pinned version.
pub fn run_data_generator(jar: &Path, cache: &Path, fresh: bool) -> Result<PathBuf> {
    let output = cache.join("generated");
    if !fresh && output.join(STAMP).exists() {
        return Ok(output);
    }
    // The generator misbehaves when run over the output of an earlier run.
    if output.exists() {
        fs::remove_dir_all(&output)?;
    }
    fs::create_dir_all(cache)?;

    eprintln!("running the data generator (needs Java 25 or newer)");
    let log_path = cache.join("datagen.log");
    let log = File::create(&log_path)?;
    let status = Command::new("java")
        .arg("-DbundlerMainClass=net.minecraft.data.Main")
        .arg("-jar")
        .arg(fs::canonicalize(jar)?)
        .args(["--all", "--output", "generated"])
        .current_dir(cache)
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()
        .context("starting `java`; is a Java runtime installed?")?;
    if !status.success() {
        bail!(
            "the data generator failed ({status}); see {}",
            log_path.display()
        );
    }
    File::create(output.join(STAMP))?;
    Ok(output)
}

/// Compiles `source` (the Java program of ADR-0019, section 5) against the classes of
/// `jar`, runs it without a server and returns the directory it wrote its dump into.
///
/// Everything happens in `cache/extract`, which is made anew each time: the jars the
/// program needs are unpacked from `jar` by the lists the jar itself carries, each
/// checked against the checksum beside its name, so nothing rests on what the data
/// generator's own start left in `cache`. The dump of an earlier run is reused only if
/// the stamp beside it holds the SHA-256 of this `source` and the SHA-1 of the pinned
/// jar, and `fresh` is not set.
pub fn run_extract(jar: &Path, cache: &Path, source: &Path, fresh: bool) -> Result<PathBuf> {
    let directory = cache.join("extract");
    let output = directory.join("out");
    let stamp_path = directory.join(STAMP);
    let source_bytes = fs::read(source).with_context(|| format!("reading {}", source.display()))?;
    let stamp = format!("{} {}\n", sha256::hex(&source_bytes), SERVER_JAR.sha1);
    if !fresh && fs::read_to_string(&stamp_path).is_ok_and(|found| found == stamp) {
        return Ok(output);
    }

    if directory.exists() {
        fs::remove_dir_all(&directory)?;
    }
    let classes = directory.join("classes");
    let working = directory.join("run");
    for made in [&classes, &working, &output] {
        fs::create_dir_all(made)?;
    }
    let mut class_path = unpack_class_path(jar, &directory.join("classpath"))?;

    eprintln!("compiling and running the extract program (needs a JDK 25 or newer)");
    let log_path = directory.join("extract.log");
    let log = File::create(&log_path)?;
    let status = Command::new("javac")
        .arg("-proc:none")
        .arg("-cp")
        .arg(std::env::join_paths(&class_path)?)
        .arg("-d")
        .arg(&classes)
        .arg(fs::canonicalize(source)?)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?)
        .status()
        .context("starting `javac`; is a JDK installed, and not only a Java runtime?")?;
    if !status.success() {
        bail!(
            "compiling {} failed ({status}); see {}",
            source.display(),
            log_path.display()
        );
    }

    class_path.insert(0, fs::canonicalize(&classes)?);
    let main_class = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("the Java program's file has no name")?;
    // The game's start writes a `logs` directory where it is run, so it gets a
    // directory of its own.
    let status = Command::new("java")
        .arg("-cp")
        .arg(std::env::join_paths(&class_path)?)
        .arg(main_class)
        .arg(fs::canonicalize(&output)?)
        .current_dir(&working)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .status()
        .context("starting `java`; is a Java runtime installed?")?;
    if !status.success() {
        bail!(
            "the extract program failed ({status}); see {}",
            log_path.display()
        );
    }
    fs::write(&stamp_path, stamp)?;
    Ok(output)
}

/// Unpacks the game's own jar and its libraries from the bundle `jar` into `directory`
/// and returns their paths, the game's jar first and the libraries in the order of the
/// bundle's list.
fn unpack_class_path(jar: &Path, directory: &Path) -> Result<Vec<PathBuf>> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)
        .with_context(|| format!("opening {} as a zip file", jar.display()))?;
    let mut paths = Vec::new();
    for (list, prefix) in [
        ("META-INF/versions.list", "META-INF/versions/"),
        ("META-INF/libraries.list", "META-INF/libraries/"),
    ] {
        let mut text = String::new();
        archive
            .by_name(list)
            .with_context(|| format!("the server jar has no {list}"))?
            .read_to_string(&mut text)?;
        for entry in bundled_entries(&text).with_context(|| format!("reading {list}"))? {
            let mut bytes = Vec::new();
            archive
                .by_name(&format!("{prefix}{}", entry.path))
                .with_context(|| format!("the server jar has no {prefix}{}", entry.path))?
                .read_to_end(&mut bytes)?;
            let actual = sha256::hex(&bytes);
            ensure!(
                actual == entry.sha256,
                "{} in the server jar has SHA-256 {actual}, its list says {}",
                entry.path,
                entry.sha256
            );
            let path = directory.join(&entry.path);
            let parent = path.parent().context("a bundled jar has no directory")?;
            fs::create_dir_all(parent)?;
            fs::write(&path, bytes)?;
            paths.push(fs::canonicalize(&path)?);
        }
    }
    ensure!(!paths.is_empty(), "the server jar bundles no jars");
    Ok(paths)
}

/// The bytes of the game's own jar, which the bundle `jar` holds as one of its entries:
/// the one `META-INF/versions.list` names, checked against the checksum beside its
/// name. The game's data files are entries of that inner jar.
pub fn game_jar(jar: &Path) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)
        .with_context(|| format!("opening {} as a zip file", jar.display()))?;
    let list = "META-INF/versions.list";
    let mut text = String::new();
    archive
        .by_name(list)
        .with_context(|| format!("the server jar has no {list}"))?
        .read_to_string(&mut text)?;
    let entries = bundled_entries(&text).with_context(|| format!("reading {list}"))?;
    let [entry] = &entries[..] else {
        bail!("{list} names {} jars where one is expected", entries.len());
    };
    let name = format!("META-INF/versions/{}", entry.path);
    let mut bytes = Vec::new();
    archive
        .by_name(&name)
        .with_context(|| format!("the server jar has no {name}"))?
        .read_to_end(&mut bytes)?;
    let actual = sha256::hex(&bytes);
    ensure!(
        actual == entry.sha256,
        "{} in the server jar has SHA-256 {actual}, its list says {}",
        entry.path,
        entry.sha256
    );
    Ok(bytes)
}

/// The files below `prefix` in a jar given as bytes, by their path after the prefix,
/// sorted. Directories are left out.
pub fn files_below(
    jar: &[u8],
    prefix: &str,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(jar))
        .context("opening the game's jar as a zip file")?;
    let mut files = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(rest) = entry.name().strip_prefix(prefix).map(str::to_owned) else {
            continue;
        };
        if rest.is_empty() || entry.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("unpacking {prefix}{rest}"))?;
        ensure!(
            files.insert(rest.clone(), bytes).is_none(),
            "the game's jar has {prefix}{rest} twice"
        );
    }
    Ok(files)
}

/// A jar inside the bundle, as one line of a list names it.
#[derive(Debug, PartialEq, Eq)]
struct BundledEntry {
    sha256: String,
    path: String,
}

/// Reads a list of the bundle: one line a jar, with its SHA-256, its name and its path
/// separated by tabs.
fn bundled_entries(list: &str) -> Result<Vec<BundledEntry>> {
    let mut entries = Vec::new();
    for line in list.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        let [sha256, _name, path] = fields[..] else {
            bail!(
                "a line has {} fields where three are expected",
                fields.len()
            );
        };
        // The path is joined to a directory, so it must not lead out of it.
        ensure!(
            !path.starts_with('/')
                && !path.contains('\\')
                && path.split('/').all(|part| !part.is_empty() && part != ".."),
            "the path {path:?} of a bundled jar cannot be unpacked"
        );
        entries.push(BundledEntry {
            sha256: sha256.to_owned(),
            path: path.to_owned(),
        });
    }
    Ok(entries)
}

fn sha1_of(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha1::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_list_gives_checksum_and_path_of_each_jar() {
        let list = "aa11\tcom.example:one:1.0\tcom/example/one/1.0/one-1.0.jar\n\
                    bb22\t26.3\t26.3/server-26.3.jar\n";
        assert_eq!(
            bundled_entries(list).unwrap(),
            [
                BundledEntry {
                    sha256: "aa11".to_owned(),
                    path: "com/example/one/1.0/one-1.0.jar".to_owned(),
                },
                BundledEntry {
                    sha256: "bb22".to_owned(),
                    path: "26.3/server-26.3.jar".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_bundle_list_whose_path_leads_out_of_the_directory_is_refused() {
        for path in [
            "../evil.jar",
            "/etc/evil.jar",
            "a/../../evil.jar",
            "a//b.jar",
            "",
        ] {
            let list = format!("aa11\tname\t{path}\n");
            assert!(bundled_entries(&list).is_err(), "{path}");
        }
        assert!(bundled_entries("aa11\tonly-two-fields\n").is_err());
    }
}
