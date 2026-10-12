//! Repository map: an outline of every source file (classes, functions, methods from Tree-sitter)
//! so an agent answering a question can go straight to the right files.
//!
//! Symbols are cached by content hash (git blob SHA for tracked files, computed hash for untracked)
//! in a store shared by every worktree of the repository, located via `git rev-parse --git-common-dir`.
//! Parsing runs in parallel via rayon.

mod symbol_extractor;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use review_model::ReviewCoverage;
use review_model::ReviewCoverageArea;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use symbol_extractor::{DefaultSymbolExtractor, Symbol, SymbolExtractor, SymbolKind};

/// Extension-to-language-id mapping, matching the TypeScript app exactly.
pub static LANGUAGE_BY_EXTENSION: &[(&str, &str)] = &[
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

// ---------------------------------------------------------------------------
// Cache types — persisted to disk
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// RepoMap — the built map exposed to callers
// ---------------------------------------------------------------------------

/// One source file in the map: its top-level definitions with their members.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Content hash (git blob SHA or computed hash).
    pub content_hash: String,
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

// ---------------------------------------------------------------------------
// RepoIndex — the main entry point
// ---------------------------------------------------------------------------

/// Builds and caches the repository map. Thread-safe, uses rayon for parallel parsing.
pub struct RepoIndex<E: SymbolExtractor = DefaultSymbolExtractor> {
    extractor: E,
}

impl Default for RepoIndex<DefaultSymbolExtractor> {
    fn default() -> Self {
        Self {
            extractor: DefaultSymbolExtractor,
        }
    }
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
        let source_files = self.source_files(root)?;
        let cache_path = Self::cache_path(root)?;

        // Load existing cache.
        let mut cache = Self::load_cache(&cache_path);

        // Get content hashes: git ls-files -s for tracked, computed for untracked.
        let tracked_hashes = git_blob_hashes(root)?;
        let untracked = git_untracked_files(root)?;

        let mut files = HashMap::new();
        let mut to_parse: Vec<(String, PathBuf, String, f64)> = Vec::new();

        for file in &source_files {
            let absolute = root.join(file);

            // Check file size and get mtime.
            let metadata = match fs::metadata(&absolute) {
                Ok(m) => m,
                Err(_) => continue,
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

            // Get content hash.
            let content_hash = if let Some(hash) = tracked_hashes.get(file.as_str()) {
                hash.clone()
            } else if untracked.contains(file.as_str()) {
                // Compute git hash-object semantics: SHA-1("blob {len}\0{content}").
                match fs::read(&absolute) {
                    Ok(content) => git_hash_object(&content),
                    Err(_) => continue,
                }
            } else {
                // File listed by ls-files but not in -s output (e.g. unmerged).
                // Fall back to reading and hashing.
                match fs::read(&absolute) {
                    Ok(content) => git_hash_object(&content),
                    Err(_) => continue,
                }
            };

            // Check cache.
            if let Some(cached) = cache.entries.get(&content_hash) {
                files.insert(
                    file.clone(),
                    FileEntry {
                        content_hash: content_hash.clone(),
                        mtime_ms,
                        symbols: cached.symbols.clone(),
                    },
                );
            } else {
                to_parse.push((file.clone(), absolute, content_hash, mtime_ms));
            }
        }

        type SymbolTuples = Vec<(String, Vec<String>)>;
        // Parse uncached files in parallel.
        let parsed: Vec<(String, String, f64, SymbolTuples)> = to_parse
            .par_iter()
            .filter_map(|(file, absolute, content_hash, mtime_ms)| {
                let ext = Path::new(file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!(".{e}"))?;
                let language_id = language_for_extension(&ext)?;
                let text = fs::read(absolute).ok()?;
                let symbols = self.extractor.extract(language_id, &text);
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
                Some((file.clone(), content_hash.clone(), *mtime_ms, symbol_tuples))
            })
            .collect();

        for (file, content_hash, mtime_ms, symbol_tuples) in parsed {
            cache.entries.insert(
                content_hash.clone(),
                CachedSymbols {
                    symbols: symbol_tuples.clone(),
                },
            );
            files.insert(
                file,
                FileEntry {
                    content_hash,
                    mtime_ms,
                    symbols: symbol_tuples,
                },
            );
        }

        // Save cache.
        Self::save_cache(&cache_path, &cache);

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
        let total = map.files.len();

        let mut lines = vec![format!(
            "Repository map: {} source files, each with the classes, functions and methods it defines (Tree-sitter outline).",
            total
        )];

        // Check for graphify output.
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
            lines.push(line);
            size += lines.last().map_or(0, |l| l.len()) + 1;
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
        let files: Vec<(&str, &FileEntry)> =
            map.files.iter().map(|(f, e)| (f.as_str(), e)).collect();

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

    // -----------------------------------------------------------------------
    // Source file listing
    // -----------------------------------------------------------------------

    /// Source files, repository-relative: from git (tracked + untracked, not ignored), else directory walk.
    fn source_files(&self, root: &Path) -> Result<Vec<String>> {
        let from_git = git_ls_files(root);
        let all = match from_git {
            Some(files) => files,
            None => walk_directory(root, "")?,
        };
        let mut unique: Vec<String> = all
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|file| {
                Path::new(file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|ext| language_for_extension(&format!(".{ext}")).is_some())
                    .unwrap_or(false)
            })
            .collect();
        unique.sort();
        unique.truncate(MAX_FILES);
        Ok(unique)
    }

    // -----------------------------------------------------------------------
    // Cache location and persistence
    // -----------------------------------------------------------------------

    /// Cache path: `<CO_REVIEW_HOME or ~/.co-review>/repos/<sha256(common_dir)[0:16]>/symbols-cache.json`
    fn cache_path(root: &Path) -> Result<PathBuf> {
        let common_dir = git_common_dir(root)?;
        let mut hasher = Sha256::new();
        hasher.update(common_dir.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let prefix = &hash[..16];

        let home = std::env::var("CO_REVIEW_HOME")
            .unwrap_or_else(|_| {
                dirs_home().join(".co-review").to_string_lossy().to_string()
            });

        let cache_dir = PathBuf::from(&home).join("repos").join(prefix);
        Ok(cache_dir.join("symbols-cache.json"))
    }

    fn load_cache(path: &Path) -> SymbolCache {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                return SymbolCache {
                    version: CACHE_VERSION,
                    entries: HashMap::new(),
                }
            }
        };
        match serde_json::from_str::<SymbolCache>(&content) {
            Ok(cache) if cache.version == CACHE_VERSION => cache,
            _ => SymbolCache {
                version: CACHE_VERSION,
                entries: HashMap::new(),
            },
        }
    }

    fn save_cache(path: &Path, cache: &SymbolCache) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Atomic write: write to temp file then rename.
        let temp_path = path.with_extension("tmp");
        let result = (|| -> Result<()> {
            let mut file = fs::File::create(&temp_path)?;
            serde_json::to_writer(&mut file, cache)?;
            file.flush()?;
            drop(file);
            fs::rename(&temp_path, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
    }
}

// ---------------------------------------------------------------------------
// Git helpers
// ---------------------------------------------------------------------------

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
    let stdout = String::from_utf8_lossy(&output.stdout);
    Some(
        stdout
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = HashMap::new();
    for line in stdout.lines() {
        // Format: <mode> <sha> <stage>\t<path>
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() != 2 {
            continue;
        }
        let meta: Vec<&str> = parts[0].split_whitespace().collect();
        if meta.len() >= 2 {
            result.insert(parts[1].to_string(), meta[1].to_string());
        }
    }
    Ok(result)
}

/// `git ls-files --others --exclude-standard` — untracked files.
fn git_untracked_files(root: &Path) -> Result<HashSet<String>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .context("failed to run git ls-files --others")?;
    if !output.status.success() {
        return Ok(HashSet::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect())
}

/// `git rev-parse --git-common-dir` — the shared .git dir for worktrees.
fn git_common_dir(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output()
        .context("failed to run git rev-parse --git-common-dir")?;
    if !output.status.success() {
        anyhow::bail!("git rev-parse --git-common-dir failed");
    }
    let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Make it absolute.
    let path = if Path::new(&dir).is_absolute() {
        PathBuf::from(&dir)
    } else {
        root.join(&dir)
    };
    // Canonicalize to resolve symlinks and ..
    let canonical = path
        .canonicalize()
        .unwrap_or(path);
    Ok(canonical.to_string_lossy().to_string())
}

/// Compute git hash-object semantics: SHA-1("blob {len}\0{content}").
fn git_hash_object(content: &[u8]) -> String {
    use sha1_smol::Sha1;
    let mut hasher = Sha1::new();
    let header = format!("blob {}\0", content.len());
    hasher.update(header.as_bytes());
    hasher.update(content);
    let digest = hasher.digest();
    let b = digest.bytes();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9],
        b[10], b[11], b[12], b[13], b[14], b[15], b[16], b[17], b[18], b[19],
    )
}

/// Home directory fallback.
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Directory walk fallback when git is not available.
fn walk_directory(root: &Path, dir: &str) -> Result<Vec<String>> {
    let mut found = Vec::new();
    walk_recursive(root, dir, &mut found)?;
    Ok(found)
}

fn walk_recursive(root: &Path, dir: &str, found: &mut Vec<String>) -> Result<()> {
    let full_path = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let entries = match fs::read_dir(&full_path) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        if found.len() >= MAX_FILES {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let relative = if dir.is_empty() {
            name_str.to_string()
        } else {
            format!("{dir}/{name_str}")
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() && !SKIPPED_DIRS.contains(&name_str.as_ref()) {
            walk_recursive(root, &relative, found)?;
        } else if file_type.is_file() {
            found.push(relative);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// Render symbols for one file: `Class { method, ... }, function`.
/// A long member list is cut short, except for members matching `query`.
fn outline(entry: &FileEntry, query: Option<&str>) -> String {
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
                    *i < MAX_CHILDREN
                        || query
                            .map(|q| member.to_lowercase().contains(q))
                            .unwrap_or(false)
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
    let x: Vec<&str> = a.split('/').collect();
    let y: Vec<&str> = b.split('/').collect();
    x.iter().zip(y.iter()).take_while(|(a, b)| a == b).count()
}

// ---------------------------------------------------------------------------
// Timing helper for benchmarks
// ---------------------------------------------------------------------------

/// Times a cold build and a warm (cached) rebuild, printing the results.
/// Returns (cold_ms, warm_ms).
pub fn benchmark_build(root: &Path) -> Result<(u128, u128)> {
    let index = RepoIndex::new();

    // Cold: clear cache first.
    if let Ok(cache_path) = RepoIndex::<DefaultSymbolExtractor>::cache_path(root) {
        let _ = fs::remove_file(&cache_path);
    }
    let start = Instant::now();
    let map = index.build(root)?;
    let cold = start.elapsed().as_millis();

    // Warm: rebuild with cache.
    let start = Instant::now();
    let _map2 = index.build(root)?;
    let warm = start.elapsed().as_millis();

    eprintln!(
        "Repository map: {} files, cold: {}ms, warm: {}ms",
        map.files.len(),
        cold,
        warm
    );

    Ok((cold, warm))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
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
            content_hash: String::new(),
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
            content_hash: String::new(),
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
            content_hash: String::new(),
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
                content_hash: "aaa".to_string(),
                mtime_ms: 1000.0,
                symbols: vec![("func_old".to_string(), vec![])],
            },
        );
        files.insert(
            "src/new.rs".to_string(),
            FileEntry {
                content_hash: "bbb".to_string(),
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
    #[ignore]
    fn benchmark_cold_warm() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let (cold, warm) = benchmark_build(root).expect("benchmark should succeed");
        eprintln!("Cold build: {}ms", cold);
        eprintln!("Warm build: {}ms", warm);
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
