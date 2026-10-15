//! The blocks of the structure templates, read from the operator's own copy of the
//! game's server jar when the server starts (the terrain plan's question 3: they are
//! Mojang's buildings and are not in the repository). Features built of templates
//! (fossils, the desert well, the sulfur springs) and the structures of phase S
//! place them.

use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use clustine_structures::template_blocks::TemplateLibrary;

/// Why the templates could not be read.
#[derive(Debug, thiserror::Error)]
pub enum TemplatesError {
    #[error("reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} is not a jar of the game's server: {why}")]
    NotTheJar { path: String, why: String },
    #[error("a template of {path} cannot be read: {why}")]
    Template { path: String, why: String },
}

/// Reads every template of the jar at `path`: the server's own jar, or the bundler the
/// game is distributed as, which holds it under `META-INF/versions/`.
pub fn read_jar(path: &Path) -> Result<TemplateLibrary, TemplatesError> {
    let shown = path.display().to_string();
    let bytes = fs::read(path).map_err(|source| TemplatesError::Io {
        path: shown.clone(),
        source,
    })?;
    let not_the_jar = |why: String| TemplatesError::NotTheJar {
        path: shown.clone(),
        why,
    };
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| not_the_jar(error.to_string()))?;
    let inner = archive
        .file_names()
        .find(|name| name.starts_with("META-INF/versions/") && name.ends_with(".jar"))
        .map(str::to_owned);
    if let Some(inner) = inner {
        let bytes = entry(&mut archive, &inner).map_err(|error| not_the_jar(error.to_string()))?;
        archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| not_the_jar(error.to_string()))?;
    }
    let names: Vec<String> = archive
        .file_names()
        .filter(|name| name.starts_with(PREFIX) && name.ends_with(".nbt"))
        .map(str::to_owned)
        .collect();
    if names.is_empty() {
        return Err(not_the_jar("it holds no structure templates".to_owned()));
    }
    let mut files = Vec::with_capacity(names.len());
    for file in &names {
        let name = file
            .strip_prefix(PREFIX)
            .and_then(|name| name.strip_suffix(".nbt"))
            .expect("filtered by both");
        let bytes = entry(&mut archive, file).map_err(|error| not_the_jar(error.to_string()))?;
        files.push((name.to_owned(), bytes));
    }
    TemplateLibrary::from_files(files).map_err(|error| TemplatesError::Template {
        path: shown,
        why: error.to_string(),
    })
}

const PREFIX: &str = "data/minecraft/structure/";

fn entry(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    name: &str,
) -> Result<Vec<u8>, zip::result::ZipError> {
    let mut bytes = Vec::new();
    archive.by_name(name)?.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn a_file_that_is_not_a_jar_is_refused_with_its_name() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.jar");
        fs::write(&path, b"not a zip").unwrap();
        let error = read_jar(&path).unwrap_err();
        assert!(matches!(error, TemplatesError::NotTheJar { .. }), "{error}");
        assert!(error.to_string().contains("server.jar"), "{error}");
    }

    #[test]
    fn a_jar_without_templates_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("other.jar");
        let mut writer = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        writer
            .start_file(
                "data/minecraft/other.txt",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"nothing").unwrap();
        writer.finish().unwrap();
        let error = read_jar(&path).unwrap_err();
        assert!(
            error.to_string().contains("no structure templates"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_jar_is_an_error_of_reading() {
        let error = read_jar(Path::new("/no/such/server.jar")).unwrap_err();
        assert!(matches!(error, TemplatesError::Io { .. }), "{error}");
    }
}
