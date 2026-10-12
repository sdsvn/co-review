//! Conformance test: compare Rust symbol extraction against the TypeScript reference output.
//!
//! Expected output was recorded from the Theia app and is stored in `tests/gen/expected.json`.
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use review_index::{language_for_extension, DefaultSymbolExtractor, SymbolExtractor, SymbolKind};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    crate_root().join("tests/fixtures")
}

fn expected_json() -> HashMap<String, Vec<(String, Vec<String>)>> {
    let path = crate_root().join("tests/gen/expected.json");
    let content = fs::read_to_string(&path).expect("tests/gen/expected.json must exist");
    serde_json::from_str(&content).expect("expected.json is valid JSON")
}

#[test]
fn fixture_symbols_match_typescript() {
    let expected = expected_json();
    let extractor = DefaultSymbolExtractor;
    let mut pass = 0;
    let mut fail = 0;

    for (file, expected_symbols) in &expected {
        let fixture_path = fixtures_dir().join(file);
        let source = fs::read(&fixture_path).unwrap_or_else(|_| panic!("fixture {file} not found"));

        let ext = Path::new(file)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .expect("fixture has extension");

        let language_id = language_for_extension(&ext).unwrap_or_else(|| panic!("no language for {ext}"));
        let symbols = extractor.extract(language_id, &source);

        // Convert to the same tuples shape: [(name, [member_names_excluding_fields])]
        let actual: Vec<(String, Vec<String>)> = symbols
            .iter()
            .map(|s| {
                let members: Vec<String> = s
                    .children
                    .iter()
                    .filter(|c| c.kind != SymbolKind::Field)
                    .map(|c| c.name.clone())
                    .collect();
                (s.name.clone(), members)
            })
            .collect();

        if actual == *expected_symbols {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("MISMATCH in {file}:");
            eprintln!("  expected: {expected_symbols:?}");
            eprintln!("  actual:   {actual:?}");
        }
    }

    eprintln!("\nFixture conformance: {pass} pass, {fail} fail out of {} files", expected.len());
    assert_eq!(fail, 0, "{fail} fixture files differ from TypeScript output");
}

#[test]
fn render_fixtures_directory() {
    // Build a map from the fixtures directory (treated as a tiny repo) and render it.
    // This tests the render path without depending on git.
    let fixtures = fixtures_dir();

    // Manually build a RepoMap from fixtures.
    let extractor = DefaultSymbolExtractor;
    let mut files = HashMap::new();

    for entry in fs::read_dir(&fixtures).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"));
        let ext = match ext {
            Some(e) => e,
            None => continue,
        };
        let language_id = match language_for_extension(&ext) {
            Some(l) => l,
            None => continue,
        };

        let source = fs::read(entry.path()).unwrap();
        let symbols = extractor.extract(language_id, &source);
        let symbol_tuples: Vec<(String, Vec<String>)> = symbols
            .iter()
            .map(|s| {
                let members: Vec<String> = s
                    .children
                    .iter()
                    .filter(|c| c.kind != SymbolKind::Field)
                    .map(|c| c.name.clone())
                    .collect();
                (s.name.clone(), members)
            })
            .collect();

        files.insert(
            name,
            review_index::FileEntry {
                mtime_ms: 0.0,
                symbols: symbol_tuples,
            },
        );
    }

    let map = review_index::RepoMap { files };
    let index = review_index::RepoIndex::new();
    let text = index
        .render(&fixtures, &map, &review_index::RenderOptions::default())
        .unwrap();

    eprintln!("Rendered fixtures:\n{text}");

    assert!(text.starts_with("Repository map:"));
    assert!(text.contains("source files"));
    // Verify some expected content.
    assert!(text.contains("sample.go"), "should list Go file");
    assert!(text.contains("sample.rs"), "should list Rust file");
    assert!(text.contains("OrderService"), "should show Go symbols");
}
