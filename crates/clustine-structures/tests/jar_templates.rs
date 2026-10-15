//! Every structure template of the pinned jar, read as the server will read them: from
//! the operator's own jar, which is not in the repository.
//!
//! ```text
//! CLUSTINE_SERVER_JAR=target/datagen/26.3/server.jar \
//!     cargo test -p clustine-structures --test jar_templates -- --ignored
//! ```
//!
//! The jar `cargo datagen` downloads is the bundler; the server's own jar is inside it
//! (`META-INF/versions/<version>/server-<version>.jar`), and the templates inside that.
//! Each file must be read, and agree with the table of templates that
//! `clustine-worldgen-data` keeps (their names and sizes), and every jigsaw must name a
//! final state that can be read.

use std::io::{Cursor, Read};

use clustine_protocol::nbt::Nbt;
use clustine_structures::processors::parse_block_state;
use clustine_structures::template_blocks::TemplateLibrary;

fn read_entry(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
#[ignore = "needs the server jar that `cargo datagen` downloads, named by CLUSTINE_SERVER_JAR"]
fn every_template_of_the_jar_is_read_and_agrees_with_the_table() {
    let path = std::env::var("CLUSTINE_SERVER_JAR").expect("CLUSTINE_SERVER_JAR names the jar");
    let mut bundler = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let inner_name = bundler
        .file_names()
        .find(|name| name.starts_with("META-INF/versions/") && name.ends_with(".jar"))
        .expect("the bundler holds the server's jar")
        .to_string();
    let inner = read_entry(&mut bundler, &inner_name);
    let mut server = zip::ZipArchive::new(Cursor::new(inner)).unwrap();
    let names: Vec<String> = server
        .file_names()
        .filter(|name| name.starts_with("data/minecraft/structure/") && name.ends_with(".nbt"))
        .map(str::to_string)
        .collect();
    let files: Vec<(String, Vec<u8>)> = names
        .iter()
        .map(|path| {
            let name = path
                .strip_prefix("data/minecraft/structure/")
                .and_then(|name| name.strip_suffix(".nbt"))
                .unwrap()
                .to_string();
            (name, read_entry(&mut server, path))
        })
        .collect();
    let library = TemplateLibrary::from_files(files).unwrap();

    let table = clustine_worldgen_data::templates();
    let in_table = (0..table.len())
        .map(|index| table.get(index))
        .filter(|template| template.exists)
        .count();
    assert_eq!(library.len(), in_table);
    let mut blocks = 0;
    let mut jigsaws = 0;
    for name in library.names() {
        let template = library.get(name).unwrap();
        let index = table
            .find(name)
            .unwrap_or_else(|| panic!("{name} is in the table"));
        assert_eq!(template.size, table.get(index).size, "{name}");
        for palette in &template.palettes {
            for block in palette {
                blocks += 1;
                let Some(nbt) = &block.nbt else { continue };
                if block.state.block().unwrap().name == "minecraft:jigsaw" {
                    jigsaws += 1;
                    let text = match nbt.get("final_state") {
                        Some(Nbt::String(text)) => text.as_str(),
                        _ => "minecraft:air",
                    };
                    assert!(
                        parse_block_state(text).is_some(),
                        "{name}: a jigsaw's final state {text}"
                    );
                }
            }
        }
    }
    eprintln!(
        "{} templates, {blocks} blocks, {jigsaws} jigsaws",
        library.len()
    );
}
