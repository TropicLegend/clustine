//! Obtains the official server jar and runs its built-in data generator.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};

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
/// The output of an earlier complete run is reused. `jar` must already be verified, so
/// the output only depends on the pinned version.
pub fn run_data_generator(jar: &Path, cache: &Path) -> Result<PathBuf> {
    let output = cache.join("generated");
    if output.join(STAMP).exists() {
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
