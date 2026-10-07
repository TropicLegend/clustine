//! Single-binary mode: runs every Clustine service in one process.

fn main() {
    eprintln!(
        "clustine {}: no services are implemented yet",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}
