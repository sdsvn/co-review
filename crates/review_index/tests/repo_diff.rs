//! Compare Rust and TypeScript symbol extraction on the full repository.
//! Run with: cargo test --test repo_diff -- --ignored --nocapture
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use review_index::{language_for_extension, DefaultSymbolExtractor, SymbolExtractor, SymbolKind};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn rust_symbols(root: &Path) -> HashMap<String, Vec<(String, Vec<String>)>> {
    let output = std::process::Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: Vec<String> = stdout
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .filter(|f| {
            Path::new(f)
                .extension()
                .and_then(|e| e.to_str())
                .map(|ext| language_for_extension(&format!(".{ext}")).is_some())
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files.truncate(5000);

    let extractor = DefaultSymbolExtractor;
    let max_bytes: u64 = 256 * 1024;
    let mut results = HashMap::new();

    for file in &files {
        let absolute = root.join(file);
        let metadata = match fs::metadata(&absolute) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_file() || metadata.len() > max_bytes {
            continue;
        }
        let ext = Path::new(file)
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
        let source = match fs::read(&absolute) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let symbols = extractor.extract(language_id, &source);
        let tuples: Vec<(String, Vec<String>)> = symbols
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
        results.insert(file.clone(), tuples);
    }
    results
}

#[test]
#[ignore]
fn full_repo_symbol_diff() {
    let root = repo_root();
    let ts_path = root.join("crates/review_index/tests/gen/ts_repo_symbols.json");

    // Generate TS symbols if not already done.
    if !ts_path.exists() {
        let output = std::process::Command::new("node")
            .args(["tests/gen/repo_compare.mjs"])
            .current_dir(root.join("crates/review_index"))
            .output()
            .expect("run repo_compare.mjs");
        assert!(
            output.status.success(),
            "repo_compare.mjs failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(&ts_path, &output.stdout).expect("write ts symbols");
    }

    let ts_content = fs::read_to_string(&ts_path).expect("read ts symbols");
    let ts: HashMap<String, Vec<(String, Vec<String>)>> =
        serde_json::from_str(&ts_content).expect("parse ts symbols");

    let rs = rust_symbols(&root);

    let mut match_count = 0;
    let mut diff_count = 0;
    let mut diffs = Vec::new();

    for (file, ts_syms) in &ts {
        match rs.get(file) {
            None => {
                diffs.push(format!("{file}: missing in Rust"));
                diff_count += 1;
            }
            Some(rs_syms) => {
                if ts_syms == rs_syms {
                    match_count += 1;
                } else {
                    diffs.push(format!(
                        "{file}:\n  TS: {ts_syms:?}\n  RS: {rs_syms:?}"
                    ));
                    diff_count += 1;
                }
            }
        }
    }

    for file in rs.keys() {
        if !ts.contains_key(file) {
            diffs.push(format!("{file}: extra in Rust (not in TS)"));
            diff_count += 1;
        }
    }

    eprintln!("\nFull repository symbol comparison:");
    eprintln!("  Match: {match_count}");
    eprintln!("  Diff:  {diff_count}");
    eprintln!("  Total TS: {}, Total RS: {}", ts.len(), rs.len());

    if !diffs.is_empty() {
        eprintln!("\nDifferences (first 30):");
        for d in diffs.iter().take(30) {
            eprintln!("  {d}");
        }
        if diffs.len() > 30 {
            eprintln!("  ... and {} more", diffs.len() - 30);
        }
    }

    // Clean up temp file.
    let _ = fs::remove_file(&ts_path);
}
