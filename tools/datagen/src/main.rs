//! Generates the committed game data tables from the official server jar's data generator.
//!
//! The server jar is downloaded into `target/datagen/`, its data generator is run with
//! Java, and Rust tables of ids and names are written into `clustine-data` and
//! `clustine-protocol`. See `docs/adr/0004-game-data.md`.

mod emit;
mod jar;
mod model;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

const USAGE: &str = "\
Usage: cargo datagen [--check] [--jar <path>]

  --check       Do not write anything; fail if the committed tables are out of date.
  --jar <path>  Use this copy of the pinned server jar instead of downloading it.";

struct Args {
    check: bool,
    jar: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let Some(args) = parse_args()? else {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    };

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cache = root.join("target/datagen").join(jar::SERVER_JAR.version);
    let jar = match args.jar {
        Some(path) => {
            jar::verify(&path)?;
            path
        }
        None => jar::fetch(&cache)?,
    };
    let generated = jar::run_data_generator(&jar, &cache)?;
    let data = model::GameData::load(&jar, &generated)?;
    let outputs = emit::all(&data)?;

    if args.check {
        let stale = stale_paths(&root, &outputs)?;
        if stale.is_empty() {
            println!("generated tables are up to date");
            return Ok(ExitCode::SUCCESS);
        }
        for path in stale {
            eprintln!("out of date: {}", path.display());
        }
        eprintln!("run `cargo datagen` and commit the result");
        return Ok(ExitCode::FAILURE);
    }

    for directory in emit::DIRECTORIES {
        let directory = root.join(directory);
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(&directory)?;
    }
    for output in &outputs {
        fs::write(root.join(&output.path), &output.content)
            .with_context(|| format!("writing {}", output.path.display()))?;
    }
    println!(
        "wrote {} files for Minecraft {}",
        outputs.len(),
        data.version.id
    );
    Ok(ExitCode::SUCCESS)
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args {
        check: false,
        jar: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--check" => args.check = true,
            "--jar" => {
                let path = raw.next().context("--jar needs a path")?;
                args.jar = Some(PathBuf::from(path));
            }
            "-h" | "--help" => return Ok(None),
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

/// Generated files that differ from `outputs`, are missing, or should not exist.
fn stale_paths(root: &Path, outputs: &[emit::Output]) -> Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    for output in outputs {
        let on_disk = fs::read_to_string(root.join(&output.path)).ok();
        if on_disk.as_deref() != Some(output.content.as_str()) {
            stale.push(output.path.clone());
        }
    }

    let expected: BTreeSet<&Path> = outputs.iter().map(|output| output.path.as_path()).collect();
    for directory in emit::DIRECTORIES {
        let Ok(entries) = fs::read_dir(root.join(directory)) else {
            continue;
        };
        for entry in entries {
            let path = Path::new(directory).join(entry?.file_name());
            if !expected.contains(path.as_path()) {
                stale.push(path);
            }
        }
    }
    Ok(stale)
}
