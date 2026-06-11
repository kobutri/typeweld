//! Macro/extractor parity: every trybuild UI fixture must be accepted or
//! rejected identically by the static extractor.
//!
//! The fixtures live in `typeweld-macros/tests/ui/{pass,fail}` and are
//! compiled by trybuild in that crate; here each one becomes a single-crate
//! workspace fed through the extractor.

use std::path::{Path, PathBuf};

use typeweld_engine::extract::extract;
use typeweld_engine::vfs::MemoryFileProvider;
use typeweld_engine::workspace;

fn fixtures_dir(kind: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../typeweld-macros/tests/ui")
        .join(kind)
}

fn extract_fixture(source: &str) -> typeweld_engine::extract::Extraction {
    let mut provider = MemoryFileProvider::default();
    provider.insert(
        PathBuf::from("/ws/Cargo.toml"),
        "[workspace]\nmembers = [\"fixture\"]\n".to_owned(),
    );
    provider.insert(
        PathBuf::from("/ws/fixture/Cargo.toml"),
        "[package]\nname = \"fixture\"\n".to_owned(),
    );
    provider.insert(PathBuf::from("/ws/fixture/src/lib.rs"), source.to_owned());

    let (workspace, diagnostics) =
        workspace::discover(Path::new("/ws"), &provider).expect("discover");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    extract(&workspace, "fixture", "@workspace/fixture", &provider).expect("extract")
}

#[test]
fn pass_fixtures_extract_without_errors() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir("pass")).expect("read pass fixtures") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read fixture");
        let extraction = extract_fixture(&source);
        assert!(
            !extraction.has_errors(),
            "extractor rejected pass fixture {}: {:?}",
            path.display(),
            extraction.diagnostics
        );
        checked += 1;
    }
    assert!(checked > 0, "no pass fixtures found");
}

#[test]
fn fail_fixtures_extract_with_errors() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixtures_dir("fail")).expect("read fail fixtures") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read fixture");
        let extraction = extract_fixture(&source);
        assert!(
            extraction.has_errors(),
            "extractor accepted fail fixture {}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no fail fixtures found");
}
