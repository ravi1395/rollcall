//! CI entry points for P40 (perf gate) and P42 (demo): `ROLLCALL_MATERIALIZE_DIR=<dir> cargo test -p rollcall-cli
//! --test it --locked -- --ignored materialize_perf` writes `<dir>/perf` (likewise `demo`).
use super::build_sessions::{build_demo_root, build_perf_root};
use super::support::manifest;
use std::path::PathBuf;
use std::time::Instant;

const MATERIALIZE_DIR_VAR: &str = "ROLLCALL_MATERIALIZE_DIR";

/// `<dir>/<name>` for a root that must not exist yet: materialising over leftovers would mix two corpora.
fn target(name: &str) -> PathBuf {
    let dir = std::env::var_os(MATERIALIZE_DIR_VAR)
        .unwrap_or_else(|| panic!("{MATERIALIZE_DIR_VAR} must name the output directory"));
    let root = PathBuf::from(dir).join(name);
    assert!(!root.exists(), "{} already exists", root.display());
    std::fs::create_dir_all(&root).unwrap_or_else(|e| panic!("mkdir {}: {e}", root.display()));
    std::fs::canonicalize(&root).expect("canonicalize output root")
}

#[test]
#[ignore = "writes the 500 MB perf corpus; run by CI's perf job"]
fn materialize_perf() {
    let root = target("perf");
    let start = Instant::now();
    let bytes = build_perf_root(&root, &manifest().perf);
    eprintln!(
        "perf corpus: {} ({bytes} bytes of logs, {:?})",
        root.display(),
        start.elapsed()
    );
}

#[test]
#[ignore = "writes the demo root; run by CI's demo job"]
fn materialize_demo() {
    let root = target("demo");
    build_demo_root(&root);
    eprintln!("demo root: {}", root.display());
}
