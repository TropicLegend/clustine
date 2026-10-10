//! Generates the committed game data from the official server jar.
//!
//! The server jar is downloaded into `target/datagen/`, its data generator is run with
//! Java, a Java program of Clustine's own is compiled and run against its classes
//! without a server, and Rust tables and packed tables are written into `clustine-data`
//! and `clustine-protocol`. See `docs/adr/0004-game-data.md` and
//! `docs/adr/0019-data-made-from-mojangs-jar.md`.

mod dump;
mod emit;
mod extract;
mod jar;
mod model;
mod packed;
mod sha256;
mod sums;
mod tables;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

const USAGE: &str = "\
Usage: cargo datagen [--check] [--fresh] [--jar <path>]
       cargo datagen --dump <table>

  --check         Do not write anything; fail if the committed output is out of date,
                  byte for byte.
  --fresh         Run the jar's data generator and the extract program again instead of
                  using what an earlier run left in target/datagen.
  --jar <path>    Use this copy of the pinned server jar instead of downloading it.
  --dump <table>  Print a committed packed table as text, one row a line. <table> is
                  block_states, biome_parameters or the path of a file. Needs no jar.

Needs a JDK 25 or newer (javac and java) on the path.";

/// The Java program that asks the game for what its data files lack, relative to the
/// workspace root.
const EXTRACT_SOURCE: &str = "tools/datagen/java/Extract.java";

struct Args {
    check: bool,
    fresh: bool,
    jar: Option<PathBuf>,
    dump: Option<String>,
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
    if let Some(table) = args.dump {
        let text = dump::run(&root, &table)?;
        // Whoever reads the dump may stop early, as `head` does; that is no failure.
        let _ = std::io::stdout().lock().write_all(text.as_bytes());
        return Ok(ExitCode::SUCCESS);
    }

    let cache = root.join("target/datagen").join(jar::SERVER_JAR.version);
    let jar = match args.jar {
        Some(path) => {
            jar::verify(&path)?;
            path
        }
        None => jar::fetch(&cache)?,
    };
    let (outputs, version) = generate(&root, &cache, &jar, args.fresh)?;

    if args.check {
        let stale = stale_paths(&root, &outputs)?;
        if stale.is_empty() {
            println!("generated output is up to date");
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
        let path = root.join(&output.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &output.content)
            .with_context(|| format!("writing {}", output.path.display()))?;
    }
    println!("wrote {} files for Minecraft {version}", outputs.len());
    Ok(ExitCode::SUCCESS)
}

/// Every committed output as the verified `jar` gives it, and the game's version.
/// What the data generator and the extract program write is kept in `cache`, and with
/// `fresh` nothing that an earlier run left there is used.
fn generate(
    root: &Path,
    cache: &Path,
    jar: &Path,
    fresh: bool,
) -> Result<(Vec<emit::Output>, String)> {
    let generated = jar::run_data_generator(jar, cache, fresh)?;
    let extracted = jar::run_extract(jar, cache, &root.join(EXTRACT_SOURCE), fresh)?;
    let data = model::GameData::load(jar, &generated, &extracted)?;
    Ok((emit::all(&data, root)?, data.version.id))
}

fn parse_args() -> Result<Option<Args>> {
    let mut args = Args {
        check: false,
        fresh: false,
        jar: None,
        dump: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--check" => args.check = true,
            "--fresh" => args.fresh = true,
            "--jar" => {
                let path = raw.next().context("--jar needs a path")?;
                args.jar = Some(PathBuf::from(path));
            }
            "--dump" => {
                args.dump = Some(raw.next().context("--dump needs the name of a table")?);
            }
            "-h" | "--help" => return Ok(None),
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

/// Generated files that differ from `outputs` by a byte, are missing, or should not
/// exist: every file under a generated directory, its sub-directories included, that
/// is not an output.
fn stale_paths(root: &Path, outputs: &[emit::Output]) -> Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    let mut expected = BTreeSet::new();
    for output in outputs {
        let on_disk = fs::read(root.join(&output.path)).ok();
        if on_disk.as_deref() != Some(output.content.as_slice()) {
            stale.push(output.path.clone());
        }
        expected.insert(sums::slashed(&output.path)?);
    }

    for directory in emit::DIRECTORIES {
        for path in sums::files_under(root, directory)? {
            if !expected.contains(&path) {
                stale.push(PathBuf::from(path));
            }
        }
    }
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = "crates/clustine-data/src/generated/blocks.rs";
    const TABLE: &str = "crates/clustine-data/src/generated/block_states.bin";
    const PROTOCOL: &str = "crates/clustine-protocol/src/generated/packet_ids.rs";

    fn outputs() -> Vec<emit::Output> {
        [
            (RUST, &b"// blocks\n"[..]),
            (TABLE, &[b'C', b'L', b'T', b'1', 0, 255, 13, 10][..]),
            (PROTOCOL, &b"// ids\n"[..]),
            (sums::PATH, &b"jar abc\n"[..]),
        ]
        .into_iter()
        .map(|(path, content)| emit::Output {
            path: PathBuf::from(path),
            content: content.to_vec(),
        })
        .collect()
    }

    /// A tree that holds exactly `outputs`.
    fn written(outputs: &[emit::Output]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for output in outputs {
            let path = root.path().join(&output.path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, &output.content).unwrap();
        }
        root
    }

    fn stale(root: &Path, outputs: &[emit::Output]) -> Vec<String> {
        stale_paths(root, outputs)
            .unwrap()
            .iter()
            .map(|path| sums::slashed(path).unwrap())
            .collect()
    }

    #[test]
    fn the_check_passes_on_output_that_is_what_was_made() {
        let outputs = outputs();
        let root = written(&outputs);
        assert!(stale(root.path(), &outputs).is_empty());
    }

    #[test]
    fn the_check_names_the_file_of_which_one_byte_was_changed() {
        let outputs = outputs();
        for changed in [RUST, TABLE, sums::PATH] {
            let root = written(&outputs);
            let path = root.path().join(changed);
            let mut bytes = fs::read(&path).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 1;
            fs::write(&path, bytes).unwrap();
            assert_eq!(stale(root.path(), &outputs), [changed]);
        }
    }

    #[test]
    fn the_check_compares_bytes_and_not_text() {
        // A table is not text, and a line end that a checkout changed is a difference.
        let outputs = outputs();
        let root = written(&outputs);
        fs::write(root.path().join(RUST), b"// blocks\r\n").unwrap();
        assert_eq!(stale(root.path(), &outputs), [RUST]);
    }

    #[test]
    fn the_check_names_an_output_that_was_removed() {
        let outputs = outputs();
        for removed in [RUST, TABLE, PROTOCOL, sums::PATH] {
            let root = written(&outputs);
            fs::remove_file(root.path().join(removed)).unwrap();
            assert_eq!(stale(root.path(), &outputs), [removed]);
        }
    }

    #[test]
    fn the_check_names_a_file_that_is_no_output_also_in_a_sub_directory() {
        let outputs = outputs();
        let root = written(&outputs);
        let directory = root.path().join("crates/clustine-data/src/generated");
        fs::write(directory.join("stray.rs"), "").unwrap();
        fs::create_dir_all(directory.join("tables/deeper")).unwrap();
        fs::write(directory.join("tables/deeper/stray.bin"), "x").unwrap();
        assert_eq!(
            stale(root.path(), &outputs),
            [
                "crates/clustine-data/src/generated/stray.rs",
                "crates/clustine-data/src/generated/tables/deeper/stray.bin",
            ]
        );
    }

    #[test]
    fn output_over_its_budget_fails_before_anything_is_written() {
        let outputs = outputs();
        let total: usize = outputs.iter().map(|output| output.content.len()).sum();
        assert!(emit::check_budget(&outputs, total).is_ok());
        let error = emit::check_budget(&outputs, total - 1).unwrap_err();
        assert!(format!("{error}").contains("over the budget"));
    }

    /// Needs the pinned jar where `cargo datagen` keeps it and a JDK; takes about a
    /// minute. Run it with `cargo test -p clustine-datagen -- --ignored pinned_jar`.
    #[test]
    #[ignore = "needs the pinned server jar in target/datagen and a JDK"]
    fn two_fresh_runs_on_the_pinned_jar_give_the_same_bytes_and_the_committed_output() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let jar = root
            .join("target/datagen")
            .join(jar::SERVER_JAR.version)
            .join("server.jar");
        jar::verify(&jar).unwrap();

        // Each run has a cache of its own, so neither can use what the other left.
        let mut runs = Vec::new();
        for _ in 0..2 {
            let cache = tempfile::tempdir().unwrap();
            let (outputs, _) = generate(&root, cache.path(), &jar, true).unwrap();
            let mut dump = Vec::new();
            for name in sums::files_under(cache.path(), "extract/out").unwrap() {
                dump.push((name.clone(), fs::read(cache.path().join(name)).unwrap()));
            }
            runs.push((outputs, dump));
        }
        let (first, second) = (&runs[0], &runs[1]);
        assert!(
            first.1.len() >= 6,
            "the extract program wrote too few files"
        );
        assert!(
            first.1 == second.1,
            "the extract program wrote other bytes the second time"
        );
        assert_eq!(first.0.len(), second.0.len());
        for (a, b) in first.0.iter().zip(&second.0) {
            assert_eq!(a.path, b.path);
            assert!(
                a.content == b.content,
                "{} differs between two runs",
                a.path.display()
            );
        }
        assert_eq!(stale(&root, &first.0), Vec::<String>::new());
    }
}
