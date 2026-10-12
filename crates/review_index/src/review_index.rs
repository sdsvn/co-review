//! Repository map: an outline of every source file (classes, functions, methods from Tree-sitter)
//! so an agent answering a question can go straight to the right files.
//!
//! Symbols are cached by content hash (git blob SHA for tracked files, computed hash for untracked)
//! in a store shared by every worktree of the repository, located via `git rev-parse --git-common-dir`.
//! Parsing runs in parallel via rayon.

mod symbol_extractor;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use rayon::prelude::*;
use review_model::{ReviewCoverage, ReviewCoverageArea};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use symbol_extractor::{DefaultSymbolExtractor, Symbol, SymbolExtractor, SymbolKind};

/// Extension-to-language-id mapping, matching the TypeScript app exactly.
static LANGUAGE_BY_EXTENSION: &[(&str, &str)] = &[
    (".go", "go"),
    (".ts", "typescript"),
    (".tsx", "typescriptreact"),
    (".js", "javascript"),
    (".jsx", "javascriptreact"),
    (".mjs", "javascript"),
    (".py", "python"),
    (".java", "java"),
    (".rs", "rust"),
    (".cs", "csharp"),
    (".cpp", "cpp"),
    (".cc", "cpp"),
    (".hpp", "cpp"),
    (".c", "c"),
    (".h", "c"),
    (".rb", "ruby"),
    (".php", "php"),
    (".sh", "shellscript"),
    (".ps1", "powershell"),
];

const MAX_FILES: usize = 5000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_CHILDREN: usize = 12;
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "dist",
    "out",
    "target",
    "__pycache__",
];
/// Cache format version; bump when the stored shape changes.
const CACHE_VERSION: u32 = 1;

/// Returns the language id for a file extension, or None if not supported.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    LANGUAGE_BY_EXTENSION
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, lang)| *lang)
}

fn language_of(file: &str) -> Option<&'static str> {
    let ext = Path::new(file).extension()?.to_str()?;
    language_for_extension(&format!(".{ext}"))
}

/// Symbols for one file, keyed by content hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedSymbols {
    symbols: Vec<(String, Vec<String>)>,
}

/// The on-disk symbol cache: content hash -> symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SymbolCache {
    version: u32,
    entries: HashMap<String, CachedSymbols>,
}

/// One source file in the map: its top-level definitions with their members.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// File modification time in milliseconds (from stat, for the current worktree only;
    /// not stored in the shared cache).
    pub mtime_ms: f64,
    /// `[(symbol_name, [member_names])]` — members exclude fields (matching TS behaviour).
    pub symbols: Vec<(String, Vec<String>)>,
}

/// The repository map: every source file and its symbols.
#[derive(Debug, Clone)]
pub struct RepoMap {
    pub files: HashMap<String, FileEntry>,
}

/// Options for rendering the map as text.
#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    /// Repository-relative file the question is about; files near it come first.
    pub focus: Option<String>,
    /// Only files under this repository-relative path.
    pub path: Option<String>,
    /// Only files whose path or symbols contain this (case-insensitive).
    pub query: Option<String>,
    pub max_chars: Option<usize>,
}

/// Builds and caches the repository map. Thread-safe, uses rayon for parallel parsing.
#[derive(Default)]
pub struct RepoIndex<E: SymbolExtractor = DefaultSymbolExtractor> {
    extractor: E,
}

impl RepoIndex<DefaultSymbolExtractor> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<E: SymbolExtractor> RepoIndex<E> {
    pub fn with_extractor(extractor: E) -> Self {
        Self { extractor }
    }

    /// Build or refresh the repository map, using the content-hash cache.
    pub fn build(&self, root: &Path) -> Result<RepoMap> {
        let source_files = source_files(root)?;
        let cache_path = cache_path(root)?;
        let mut cache = load_cache(&cache_path);
        let tracked_hashes = git_blob_hashes(root)?;

        let mut files = HashMap::new();
        let mut to_parse: Vec<(String, PathBuf, String, f64)> = Vec::new();
        for file in source_files {
            let absolute = root.join(&file);
            let Ok(metadata) = fs::metadata(&absolute) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                continue;
            }
            let mtime_ms = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            // Untracked (or unmerged, absent from `ls-files -s`) files are hashed as git would.
            let Some(content_hash) = tracked_hashes
                .get(&file)
                .cloned()
                .or_else(|| fs::read(&absolute).ok().map(|content| git_hash_object(&content)))
            else {
                continue;
            };
            match cache.entries.get(&content_hash) {
                Some(cached) => {
                    files.insert(
                        file,
                        FileEntry {
                            mtime_ms,
                            symbols: cached.symbols.clone(),
                        },
                    );
                }
                None => to_parse.push((file, absolute, content_hash, mtime_ms)),
            }
        }

        let parsed: Vec<_> = to_parse
            .into_par_iter()
            .filter_map(|(file, absolute, content_hash, mtime_ms)| {
                let language_id = language_of(&file)?;
                let text = fs::read(absolute).ok()?;
                let symbols: Vec<(String, Vec<String>)> = self
                    .extractor
                    .extract(language_id, &text)
                    .into_iter()
                    .map(|symbol| {
                        let members = symbol
                            .children
                            .into_iter()
                            .filter(|child| child.kind != SymbolKind::Field)
                            .map(|child| child.name)
                            .collect();
                        (symbol.name, members)
                    })
                    .collect();
                Some((file, content_hash, mtime_ms, symbols))
            })
            .collect();

        for (file, content_hash, mtime_ms, symbols) in parsed {
            cache.entries.insert(
                content_hash,
                CachedSymbols {
                    symbols: symbols.clone(),
                },
            );
            files.insert(file, FileEntry { mtime_ms, symbols });
        }

        save_cache(&cache_path, &cache);
        Ok(RepoMap { files })
    }

    /// Render the map as text for an agent.
    pub fn render(&self, root: &Path, map: &RepoMap, options: &RenderOptions) -> Result<String> {
        let query = options.query.as_deref().map(|q| q.to_lowercase());
        let prefix = options.path.as_deref().map(|p| {
            let p = p.strip_prefix("./").unwrap_or(p);
            p.strip_suffix('/').unwrap_or(p).to_string()
        });

        let mut file_entries: Vec<(&str, &FileEntry)> = map
            .files
            .iter()
            .filter(|(file, entry)| {
                if let Some(ref prefix) = prefix
                    && *file != prefix
                    && !file.starts_with(&format!("{prefix}/"))
                {
                    return false;
                }
                if let Some(ref query) = query {
                    let file_matches = file.to_lowercase().contains(query.as_str());
                    let symbol_matches = entry.symbols.iter().any(|(name, members)| {
                        name.to_lowercase().contains(query.as_str())
                            || members
                                .iter()
                                .any(|m| m.to_lowercase().contains(query.as_str()))
                    });
                    return file_matches || symbol_matches;
                }
                true
            })
            .map(|(f, e)| (f.as_str(), e))
            .collect();

        if let Some(ref focus) = options.focus {
            file_entries.sort_by(|(a, _), (b, _)| {
                let near_a = common_segments(a, focus);
                let near_b = common_segments(b, focus);
                near_b.cmp(&near_a).then_with(|| a.cmp(b))
            });
        } else {
            file_entries.sort_by_key(|(a, _)| *a);
        }

        let max_chars = options.max_chars.unwrap_or(20_000);
        let mut lines = vec![format!(
            "Repository map: {} source files, each with the classes, functions and methods it defines (Tree-sitter outline).",
            map.files.len()
        )];
        if root.join("graphify-out/GRAPH_REPORT.md").is_file() {
            lines.push(
                "A Graphify knowledge graph of this repository is in graphify-out/ (GRAPH_REPORT.md summarizes it)."
                    .to_string(),
            );
        }

        let mut size: usize = lines.iter().map(|l| l.len()).sum::<usize>() + lines.len() - 1;
        let mut shown = 0;
        for (file, entry) in &file_entries {
            let text = outline(entry, query.as_deref());
            let line = if text.is_empty() {
                file.to_string()
            } else {
                format!("{file}: {text}")
            };
            if size + line.len() + 1 > max_chars {
                break;
            }
            size += line.len() + 1;
            lines.push(line);
            shown += 1;
        }

        if shown < file_entries.len() {
            lines.push(format!(
                "\u{2026} and {} more files (narrow it down with a path or query).",
                file_entries.len() - shown
            ));
        } else if file_entries.is_empty() {
            if query.is_some() || prefix.is_some() {
                lines.push("No files match.".to_string());
            } else {
                lines.push("No source files found.".to_string());
            }
        }

        Ok(lines.join("\n"))
    }

    /// Coverage: how much of the repository the reviewer has seen, with areas.
    ///
    /// `viewed` maps file path to the time (ms since epoch) it was marked viewed,
    /// matching the TypeScript app's `review.viewed: Record<path, number>`.
    /// A file counts as viewed only if the viewed time is at or after the file's mtime,
    /// so a file that changed since it was viewed drops back to "not viewed".
    pub fn coverage(
        &self,
        map: &RepoMap,
        viewed: &HashMap<String, f64>,
    ) -> ReviewCoverage {
        // In path order, as the map is built, so each area's `next` is its first unviewed file.
        let mut files: Vec<(&str, &FileEntry)> =
            map.files.iter().map(|(f, e)| (f.as_str(), e)).collect();
        files.sort_unstable_by_key(|(file, _)| *file);

        let seen = |file: &str, entry: &FileEntry| {
            viewed
                .get(file)
                .map(|&viewed_at| viewed_at >= entry.mtime_ms)
                .unwrap_or(false)
        };

        fn folder_parts(file: &str) -> Vec<&str> {
            let parts: Vec<&str> = file.split('/').collect();
            if parts.len() > 1 {
                parts[..parts.len() - 1].to_vec()
            } else {
                Vec::new()
            }
        }

        fn area_of(file: &str, split: &HashSet<String>) -> String {
            let parts = folder_parts(file);
            let mut depth = 1usize;
            while depth < parts.len() && depth < 4 {
                let prefix: String = parts[..depth].join("/");
                if split.contains(&prefix) {
                    depth += 1;
                } else {
                    break;
                }
            }
            if depth <= parts.len() && !parts.is_empty() {
                parts[..depth.min(parts.len())].join("/")
            } else {
                ".".to_string()
            }
        }

        // Iteratively split folders holding >40% of files.
        let mut split = HashSet::new();
        loop {
            let mut changed = false;
            let mut counts: HashMap<String, usize> = HashMap::new();
            for (f, _) in &files {
                let area = area_of(f, &split);
                *counts.entry(area).or_default() += 1;
            }
            for (area, count) in &counts {
                if area == "." {
                    continue;
                }
                let deeper = files.iter().any(|(f, _)| {
                    area_of(f, &split) == *area && folder_parts(f).len() > area.split('/').count()
                });
                let threshold = 10usize.max((files.len() as f64 * 0.4) as usize);
                if *count > threshold && deeper && area.split('/').count() < 4 {
                    split.insert(area.clone());
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut areas: HashMap<String, ReviewCoverageArea> = HashMap::new();
        for (file, entry) in &files {
            let key = area_of(file, &split);
            let area = areas.entry(key.clone()).or_insert_with(|| ReviewCoverageArea {
                path: key,
                total: 0,
                viewed: 0,
                next: None,
            });
            area.total += 1;
            if seen(file, entry) {
                area.viewed += 1;
            } else if area.next.is_none() {
                area.next = Some(file.to_string());
            }
        }

        let mut list: Vec<ReviewCoverageArea> = areas.into_values().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));

        let total_viewed: u32 = list.iter().map(|a| a.viewed).sum();
        ReviewCoverage {
            total: files.len() as u32,
            viewed: total_viewed,
            areas: list,
        }
    }

}

/// Source files, repository-relative: from git (tracked + untracked, not ignored), else directory walk.
fn source_files(root: &Path) -> Result<Vec<String>> {
    let all = match git_ls_files(root) {
        Some(files) => files,
        None => {
            let mut found = Vec::new();
            walk_directory(root, "", &mut found)?;
            found
        }
    };
    let mut unique: Vec<String> = all
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|file| language_of(file).is_some())
        .collect();
    unique.sort();
    unique.truncate(MAX_FILES);
    Ok(unique)
}

/// Cache path: `<CO_REVIEW_HOME or ~/.co-review>/repos/<sha256(common_dir)[0:16]>/symbols-cache.json`
fn cache_path(root: &Path) -> Result<PathBuf> {
    let hash = format!("{:x}", Sha256::digest(git_common_dir(root)?.as_bytes()));
    Ok(review_store::co_review_home()
        .join("repos")
        .join(&hash[..16])
        .join("symbols-cache.json"))
}

fn load_cache(path: &Path) -> SymbolCache {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<SymbolCache>(&content).ok())
        .filter(|cache| cache.version == CACHE_VERSION)
        .unwrap_or_else(|| SymbolCache {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        })
}

/// Written to a temporary file and renamed, so a concurrent build never reads half a cache.
fn save_cache(path: &Path, cache: &SymbolCache) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let temp_path = path.with_extension("tmp");
    let result = serde_json::to_vec(cache)
        .map_err(anyhow::Error::from)
        .and_then(|json| Ok(fs::write(&temp_path, json)?))
        .and_then(|()| Ok(fs::rename(&temp_path, path)?));
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
}

/// `git ls-files -z --cached --others --exclude-standard`
fn git_ls_files(root: &Path) -> Option<Vec<String>> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// `git ls-files -s` — returns `{relative_path: blob_sha}` for tracked files.
fn git_blob_hashes(root: &Path) -> Result<HashMap<String, String>> {
    let output = Command::new("git")
        .args(["ls-files", "-s"])
        .current_dir(root)
        .output()
        .context("failed to run git ls-files -s")?;
    if !output.status.success() {
        return Ok(HashMap::new());
    }
    // Format: <mode> <sha> <stage>\t<path>
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let sha = meta.split_whitespace().nth(1)?;
            Some((path.to_string(), sha.to_string()))
        })
        .collect())
}

/// `git rev-parse --git-common-dir` — the shared .git dir for worktrees, absolute and canonical.
fn git_common_dir(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output()
        .context("failed to run git rev-parse --git-common-dir")?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse --git-common-dir failed");
    }
    let path = root.join(String::from_utf8_lossy(&output.stdout).trim());
    let canonical = path.canonicalize().unwrap_or(path);
    Ok(canonical.to_string_lossy().to_string())
}

/// Compute git hash-object semantics: SHA-1("blob {len}\0{content}").
fn git_hash_object(content: &[u8]) -> String {
    let mut hasher = sha1_smol::Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hasher.digest().to_string()
}

/// Directory walk fallback when git is not available.
fn walk_directory(root: &Path, dir: &str, found: &mut Vec<String>) -> Result<()> {
    let Ok(entries) = fs::read_dir(root.join(dir)) else {
        return Ok(());
    };
    for entry in entries {
        if found.len() >= MAX_FILES {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let relative = if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() && !SKIPPED_DIRS.contains(&name.as_ref()) {
            walk_directory(root, &relative, found)?;
        } else if file_type.is_file() {
            found.push(relative);
        }
    }
    Ok(())
}

/// A file's outline line: `Class { method, ... }, function`. A long member list is cut short,
/// except for members matching `query` (lowercase).
pub fn outline(entry: &FileEntry, query: Option<&str>) -> String {
    entry
        .symbols
        .iter()
        .map(|(name, members)| {
            if members.is_empty() {
                return name.clone();
            }
            let shown: Vec<&str> = members
                .iter()
                .enumerate()
                .filter(|(i, member)| {
                    *i < MAX_CHILDREN || query.is_some_and(|q| member.to_lowercase().contains(q))
                })
                .map(|(_, m)| m.as_str())
                .collect();
            let ellipsis = if shown.len() < members.len() {
                ", \u{2026}"
            } else {
                ""
            };
            format!("{name} {{ {}{ellipsis} }}", shown.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Count how many path segments two paths share from the left.
fn common_segments(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(a, b)| a == b)
        .count()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_language_for_extension() {
        assert_eq!(language_for_extension(".rs"), Some("rust"));
        assert_eq!(language_for_extension(".ts"), Some("typescript"));
        assert_eq!(language_for_extension(".xyz"), None);
    }

    #[test]
    fn test_common_segments() {
        assert_eq!(common_segments("src/api/handler.rs", "src/api/model.rs"), 2);
        assert_eq!(common_segments("src/api/handler.rs", "tests/main.rs"), 0);
        assert_eq!(common_segments("a/b/c", "a/b/d"), 2);
    }

    #[test]
    fn test_outline_formatting() {
        let entry = FileEntry {
            mtime_ms: 0.0,
            symbols: vec![
                ("MyClass".to_string(), vec!["method_a".to_string(), "method_b".to_string()]),
                ("standalone_fn".to_string(), vec![]),
            ],
        };
        let text = outline(&entry, None);
        assert_eq!(text, "MyClass { method_a, method_b }, standalone_fn");
    }

    #[test]
    fn test_outline_truncation() {
        let members: Vec<String> = (0..20).map(|i| format!("member_{i}")).collect();
        let entry = FileEntry {
            mtime_ms: 0.0,
            symbols: vec![("BigClass".to_string(), members)],
        };
        let text = outline(&entry, None);
        assert!(text.contains("\u{2026}"));
        // Should show MAX_CHILDREN (12) members.
        assert!(text.contains("member_11"));
        assert!(!text.contains("member_12"));
    }

    #[test]
    fn test_outline_query_bypass_truncation() {
        let members: Vec<String> = (0..20).map(|i| format!("member_{i}")).collect();
        let entry = FileEntry {
            mtime_ms: 0.0,
            symbols: vec![("BigClass".to_string(), members)],
        };
        // Query for member_15 should include it even though it's past MAX_CHILDREN.
        let text = outline(&entry, Some("member_15"));
        assert!(text.contains("member_15"));
    }

    #[test]
    fn test_git_hash_object() {
        // Known value: echo -n "hello" | git hash-object --stdin
        let hash = git_hash_object(b"hello");
        assert_eq!(hash, "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");
    }

    #[test]
    fn test_build_on_this_repo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let index = RepoIndex::new();
        let map = index.build(root).expect("build should succeed");
        assert!(!map.files.is_empty(), "should find source files");
        // Should find Rust files at minimum.
        assert!(
            map.files.keys().any(|f| f.ends_with(".rs")),
            "should find .rs files"
        );
    }

    #[test]
    fn test_render_basic() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let index = RepoIndex::new();
        let map = index.build(root).expect("build should succeed");
        let text = index
            .render(root, &map, &RenderOptions::default())
            .expect("render should succeed");
        assert!(text.starts_with("Repository map:"));
        assert!(text.contains("source files"));
    }

    #[test]
    fn test_render_with_focus() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let index = RepoIndex::new();
        let map = index.build(root).expect("build should succeed");
        let text = index
            .render(
                root,
                &map,
                &RenderOptions {
                    focus: Some("crates/review_index/src/review_index.rs".to_string()),
                    ..Default::default()
                },
            )
            .expect("render should succeed");
        assert!(text.starts_with("Repository map:"));
    }

    #[test]
    fn test_coverage() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let index = RepoIndex::new();
        let map = index.build(root).expect("build should succeed");
        let coverage = index.coverage(&map, &HashMap::<String, f64>::new());
        assert!(coverage.total > 0);
        assert_eq!(coverage.viewed, 0);
        assert!(!coverage.areas.is_empty());
    }

    #[test]
    fn test_coverage_viewed_before_and_after_mtime() {
        let mut files = HashMap::new();
        files.insert(
            "src/old.rs".to_string(),
            FileEntry {
                mtime_ms: 1000.0,
                symbols: vec![("func_old".to_string(), vec![])],
            },
        );
        files.insert(
            "src/new.rs".to_string(),
            FileEntry {
                mtime_ms: 5000.0,
                symbols: vec![("func_new".to_string(), vec![])],
            },
        );
        let map = RepoMap { files };
        let index = RepoIndex::new();

        // Viewed at time 3000: old.rs (mtime 1000) is viewed, new.rs (mtime 5000) is not.
        let mut viewed = HashMap::new();
        viewed.insert("src/old.rs".to_string(), 3000.0);
        viewed.insert("src/new.rs".to_string(), 3000.0);
        let coverage = index.coverage(&map, &viewed);
        assert_eq!(coverage.total, 2);
        assert_eq!(coverage.viewed, 1, "only old.rs should count as viewed");

        // Viewed at time 5000: both viewed (>= mtime).
        viewed.insert("src/new.rs".to_string(), 5000.0);
        let coverage = index.coverage(&map, &viewed);
        assert_eq!(coverage.viewed, 2, "both should count as viewed");

        // No viewed entries: nothing viewed.
        let coverage = index.coverage(&map, &HashMap::new());
        assert_eq!(coverage.viewed, 0);
    }

    #[test]
    fn test_coverage_next_is_the_first_unviewed_file_in_path_order() {
        let entry = || FileEntry {
            mtime_ms: 1000.0,
            symbols: Vec::new(),
        };
        let names = ["src/k.rs", "src/c.rs", "src/z.rs", "src/a.rs", "src/q.rs", "src/b.rs"];
        let map = RepoMap {
            files: names.iter().map(|name| (name.to_string(), entry())).collect(),
        };
        let viewed = HashMap::from([("src/a.rs".to_string(), 2000.0)]);
        let coverage = RepoIndex::new().coverage(&map, &viewed);
        assert_eq!(coverage.areas.len(), 1);
        assert_eq!(coverage.areas[0].next.as_deref(), Some("src/b.rs"));
    }

    #[test]
    #[ignore]
    fn benchmark_cold_warm() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let index = RepoIndex::new();
        let _ = fs::remove_file(cache_path(root).unwrap());
        let start = Instant::now();
        let map = index.build(root).expect("cold build");
        let cold = start.elapsed().as_millis();
        let start = Instant::now();
        index.build(root).expect("warm build");
        let warm = start.elapsed().as_millis();
        eprintln!("Repository map: {} files, cold: {cold}ms, warm: {warm}ms", map.files.len());
    }

    #[test]
    #[ignore]
    fn benchmark_second_worktree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();

        // Build in the main worktree first (fills cache).
        let index = RepoIndex::new();
        let _map = index.build(root).expect("build should succeed");

        // Create a temporary worktree.
        let temp = tempfile::TempDir::new().expect("create temp dir");
        let worktree_path = temp.path().join("worktree");
        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                &worktree_path.to_string_lossy(),
                "HEAD",
            ])
            .current_dir(root)
            .output()
            .expect("git worktree add");
        assert!(output.status.success(), "git worktree add failed");

        // Build in the second worktree — should reuse the cache.
        let start = Instant::now();
        let map2 = index.build(&worktree_path).expect("build in worktree");
        let elapsed = start.elapsed().as_millis();
        eprintln!(
            "Second worktree: {} files in {}ms",
            map2.files.len(),
            elapsed
        );

        // Clean up.
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", &worktree_path.to_string_lossy()])
            .current_dir(root)
            .output();
    }
}
