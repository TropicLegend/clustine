//! `tools/datagen/generated.sums`: what the committed output was made from, and what
//! it is (ADR-0019, section 3).
//!
//! The file holds the jar's SHA-1, then a line with the BLAKE3 hash of each input that
//! is in the repository (every file under `tools/datagen/src` and `tools/datagen/java`,
//! datagen's manifest and the lock file), then a line for each output. It is an output
//! itself, so `--check` covers it. A test here makes it again from the files on disk
//! and needs no jar for that: it fails when a generated file was edited, and when the
//! emitter, the Java program or the lock file changed and datagen was not run again.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};

/// Where the sums are, relative to the workspace root.
pub const PATH: &str = "tools/datagen/generated.sums";

/// The directories whose every file is an input, and the single files that are.
const INPUT_DIRECTORIES: [&str; 2] = ["tools/datagen/java", "tools/datagen/src"];
const INPUT_FILES: [&str; 2] = ["Cargo.lock", "tools/datagen/Cargo.toml"];

/// The inputs under `root` with their hashes, sorted by path.
pub fn inputs(root: &Path) -> Result<Vec<(String, String)>> {
    let mut paths: Vec<String> = INPUT_FILES.iter().map(|path| (*path).to_owned()).collect();
    for directory in INPUT_DIRECTORIES {
        paths.extend(files_under(root, directory)?);
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let bytes =
                fs::read(root.join(&path)).with_context(|| format!("reading the input {path}"))?;
            Ok((path, hash(&bytes)))
        })
        .collect()
}

/// The files below `directory` of `root`, as paths from `root` with `/` between their
/// parts, sorted. A directory that does not exist has none.
pub fn files_under(root: &Path, directory: &str) -> Result<Vec<String>> {
    fn walk(at: &Path, prefix: &str, out: &mut Vec<String>) -> Result<()> {
        let Ok(entries) = fs::read_dir(at) else {
            return Ok(());
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .with_context(|| format!("a name in {} is not UTF-8", at.display()))?;
            let path = format!("{prefix}/{name}");
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), &path, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    walk(&root.join(directory), directory, &mut files)?;
    files.sort();
    Ok(files)
}

/// The text of the sums for the jar with `jar_sha1`, these `inputs` and these outputs,
/// each a path from the workspace root and its content.
pub fn render<'a>(
    jar_sha1: &str,
    inputs: &[(String, String)],
    outputs: impl Iterator<Item = (&'a Path, &'a [u8])>,
) -> Result<String> {
    let mut text = format!("jar {jar_sha1}\n");
    for (path, hash) in inputs {
        text.push_str(&format!("input {hash} {path}\n"));
    }
    let mut hashed = Vec::new();
    for (path, content) in outputs {
        let path = slashed(path)?;
        ensure!(path != PATH, "the sums cannot hold a hash of themselves");
        hashed.push((path, hash(content)));
    }
    hashed.sort();
    for (path, hash) in hashed {
        text.push_str(&format!("output {hash} {path}\n"));
    }
    Ok(text)
}

/// The sums as the files under `root` make them: the inputs, and as outputs every file
/// in the generated `directories`. The tests make the committed sums again with it.
#[cfg(test)]
pub fn from_disk(root: &Path, jar_sha1: &str, directories: &[&str]) -> Result<String> {
    let mut outputs = Vec::new();
    for directory in directories {
        for path in files_under(root, directory)? {
            let content = fs::read(root.join(&path)).with_context(|| format!("reading {path}"))?;
            outputs.push((std::path::PathBuf::from(path), content));
        }
    }
    render(
        jar_sha1,
        &inputs(root)?,
        outputs
            .iter()
            .map(|(path, content)| (path.as_path(), content.as_slice())),
    )
}

/// A relative path with `/` between its parts, whatever the platform writes.
pub fn slashed(path: &Path) -> Result<String> {
    let parts = path
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<Vec<_>>>()
        .with_context(|| format!("{} is not UTF-8", path.display()))?;
    Ok(parts.join("/"))
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::emit;
    use crate::jar::SERVER_JAR;

    fn workspace() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// A small tree with every kind of input and two outputs.
    fn tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for (path, content) in [
            ("Cargo.lock", "lock"),
            ("tools/datagen/Cargo.toml", "manifest"),
            ("tools/datagen/src/main.rs", "fn main() {}"),
            ("tools/datagen/src/deeper/emit.rs", "// emit"),
            ("tools/datagen/java/Extract.java", "class Extract {}"),
            ("out/a.rs", "// a"),
            ("out/tables/b.bin", "b"),
        ] {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        root
    }

    #[test]
    fn the_committed_sums_are_those_of_the_files_in_the_repository() {
        let root = workspace();
        let committed = fs::read_to_string(root.join(PATH)).unwrap();
        let made = from_disk(&root, SERVER_JAR.sha1, &emit::DIRECTORIES).unwrap();
        assert!(
            committed == made,
            "tools/datagen/generated.sums is not what the files in the repository give: a \
             generated file was edited, or datagen's sources, the Java program or Cargo.lock \
             changed since `cargo datagen` last ran. Run `cargo datagen` and commit the \
             result.\n--- committed\n{committed}--- from the files\n{made}"
        );
    }

    #[test]
    fn the_sums_name_the_jar_then_every_input_then_every_output() {
        let root = tree();
        let sums = from_disk(root.path(), "abc123", &["out"]).unwrap();
        let lines: Vec<&str> = sums.lines().collect();
        assert_eq!(lines[0], "jar abc123");
        let named: Vec<(&str, &str)> = lines[1..]
            .iter()
            .map(|line| {
                let mut parts = line.split(' ');
                let kind = parts.next().unwrap();
                assert_eq!(
                    parts.next().unwrap().len(),
                    64,
                    "a BLAKE3 hash in hexadecimal"
                );
                (kind, parts.next().unwrap())
            })
            .collect();
        assert_eq!(
            named,
            [
                ("input", "Cargo.lock"),
                ("input", "tools/datagen/Cargo.toml"),
                ("input", "tools/datagen/java/Extract.java"),
                ("input", "tools/datagen/src/deeper/emit.rs"),
                ("input", "tools/datagen/src/main.rs"),
                ("output", "out/a.rs"),
                ("output", "out/tables/b.bin"),
            ]
        );
        let expected = blake3::hash(b"lock").to_hex().to_string();
        assert_eq!(lines[1], format!("input {expected} Cargo.lock"));
    }

    #[test]
    fn a_changed_byte_in_any_input_or_output_changes_the_sums() {
        let root = tree();
        let before = from_disk(root.path(), "abc123", &["out"]).unwrap();
        assert_eq!(before, from_disk(root.path(), "abc123", &["out"]).unwrap());
        for path in [
            "Cargo.lock",
            "tools/datagen/Cargo.toml",
            "tools/datagen/src/main.rs",
            "tools/datagen/src/deeper/emit.rs",
            "tools/datagen/java/Extract.java",
            "out/a.rs",
            "out/tables/b.bin",
        ] {
            let file = root.path().join(path);
            let original = fs::read(&file).unwrap();
            let mut changed = original.clone();
            changed[0] ^= 1;
            fs::write(&file, changed).unwrap();
            let after = from_disk(root.path(), "abc123", &["out"]).unwrap();
            assert_ne!(before, after, "{path}");
            assert_eq!(before.lines().count(), after.lines().count());
            fs::write(&file, original).unwrap();
        }
        assert_ne!(before, from_disk(root.path(), "abc124", &["out"]).unwrap());
    }

    #[test]
    fn a_file_added_or_removed_changes_the_sums() {
        let root = tree();
        let before = from_disk(root.path(), "abc123", &["out"]).unwrap();
        fs::write(root.path().join("out/tables/extra.bin"), "x").unwrap();
        assert_ne!(before, from_disk(root.path(), "abc123", &["out"]).unwrap());
        fs::remove_file(root.path().join("out/tables/extra.bin")).unwrap();
        fs::remove_file(root.path().join("out/a.rs")).unwrap();
        assert_ne!(before, from_disk(root.path(), "abc123", &["out"]).unwrap());
    }

    #[test]
    fn rendering_sorts_outputs_by_path_and_refuses_the_sums_themselves() {
        let outputs = [
            (PathBuf::from("b/z.rs"), b"z".to_vec()),
            (PathBuf::from("a/y.rs"), b"y".to_vec()),
        ];
        let give = || outputs.iter().map(|(p, c)| (p.as_path(), c.as_slice()));
        let text = render("abc", &[], give()).unwrap();
        let paths: Vec<&str> = text
            .lines()
            .skip(1)
            .map(|line| line.rsplit(' ').next().unwrap())
            .collect();
        assert_eq!(paths, ["a/y.rs", "b/z.rs"]);

        let itself = [(PathBuf::from(PATH), Vec::new())];
        assert!(
            render(
                "abc",
                &[],
                itself.iter().map(|(p, c)| (p.as_path(), c.as_slice()))
            )
            .is_err()
        );
    }

    #[test]
    fn every_generated_rust_file_starts_with_the_line_that_says_whose_the_data_is() {
        let root = workspace();
        let head = emit::head(SERVER_JAR.version);
        assert!(head.contains("The data in this file is Mojang's and not under the AGPL"));
        let mut rust_files = 0;
        for directory in emit::DIRECTORIES {
            for path in files_under(&root, directory).unwrap() {
                if !path.ends_with(".rs") {
                    continue;
                }
                rust_files += 1;
                let text = fs::read_to_string(root.join(&path)).unwrap();
                assert_eq!(text.lines().next(), Some(head.as_str()), "{path}");
            }
        }
        assert!(
            rust_files >= 10,
            "only {rust_files} generated Rust files were found"
        );
    }

    #[test]
    fn every_generated_directory_has_a_notice_that_lists_its_files() {
        let root = workspace();
        for directory in emit::DIRECTORIES {
            let notice = fs::read_to_string(root.join(directory).join(emit::NOTICE_FILE))
                .unwrap_or_else(|_| panic!("{directory} has no NOTICE"));
            assert!(
                notice.contains("is Mojang's and not under the AGPL"),
                "{directory}"
            );
            assert!(notice.contains("NOTICE.md"), "{directory}");
            for path in files_under(&root, directory).unwrap() {
                let name = path
                    .strip_prefix(directory)
                    .unwrap()
                    .trim_start_matches('/');
                if name != emit::NOTICE_FILE {
                    assert!(
                        notice.lines().any(|line| line.trim() == name),
                        "the NOTICE of {directory} does not list {name}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_committed_generated_output_is_within_its_budget() {
        let root = workspace();
        let mut total = fs::metadata(root.join(PATH)).unwrap().len();
        for directory in emit::DIRECTORIES {
            for path in files_under(&root, directory).unwrap() {
                total += fs::metadata(root.join(path)).unwrap().len();
            }
        }
        assert!(
            total <= emit::TOTAL_BUDGET as u64,
            "{total} bytes of generated output, over the budget of {}",
            emit::TOTAL_BUDGET
        );
    }
}
