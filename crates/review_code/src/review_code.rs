//! The code a change review shows around its diff, so the editor (project panel, search, go to
//! definition, hover) works on real files.
//!
//! For the reviewer's own change that is the repository itself. For a pull request (a head that
//! isn't what the repository has checked out) it is a git worktree of the head under
//! `~/.co-review/worktrees/`, which shares the repository's objects. The worktree checks out the
//! change's neighbourhood (the folders of the changed and related files) first, so the code is
//! there in seconds, and the rest in the background. It is removed when the review is archived or
//! deleted, and created again when the review is unarchived. The review stays keyed by its review
//! directory; `bundle.code` is where its window opens, and `ReviewStore::list_for_window` lists
//! the review for a window on that folder.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use anyhow::{Context as _, Result, bail};
use parking_lot::Mutex;
use review_bundle::patch::parse_patch;
use review_bundle::service::patches_of;
use review_model::{
    CheckoutKind, CodeAnchor, CodeLocation, LocationKind, PatchAnchor, PatchAnchorType, PatchSide,
    Review, ReviewBundle,
};
use review_store::{ReviewChange, ReviewStore, file_uri_to_path, path_to_file_uri, real_path};
use sha2::{Digest, Sha256};

/// Runs a job off the caller's thread: the rest of a worktree's checkout. Lets the host use its
/// own executor; [`CodeFolders::new`] uses a std thread per job.
pub type Spawner = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

/// The two sides of a revision range as `git diff` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeSides {
    pub base: String,
    /// `None`: the working tree.
    pub head: Option<String>,
    /// `base...head`: the diff starts at the merge base.
    pub merge_base: bool,
}

/// `main...feature`, `main..feature` (an empty side is `HEAD`), or a single revision diffed
/// against the working tree.
pub fn sides_of(range: &str) -> RangeSides {
    let either_or_head = |side: &str| {
        if side.is_empty() {
            "HEAD".to_string()
        } else {
            side.to_string()
        }
    };
    for (separator, merge_base) in [("...", true), ("..", false)] {
        let parts: Vec<&str> = range.split(separator).collect();
        if let [base, head] = parts.as_slice() {
            return RangeSides {
                base: either_or_head(base),
                head: Some(either_or_head(head)),
                merge_base,
            };
        }
    }
    RangeSides {
        base: range.to_string(),
        head: None,
        merge_base: false,
    }
}

struct Worktree {
    repo: PathBuf,
    code: PathBuf,
}

/// Gives change reviews their code folder and keeps their worktrees in step with the reviews.
pub struct CodeFolders {
    store: Arc<ReviewStore>,
    worktrees_dir: PathBuf,
    spawn: Spawner,
    /// Worktrees by review, so a deleted review's worktree can be removed (a deletion carries no
    /// review).
    worktrees: Mutex<HashMap<String, Worktree>>,
    /// Reviews whose worktree is being created, so changes arriving meanwhile don't start a
    /// second one.
    creating: Mutex<HashSet<String>>,
}

impl CodeFolders {
    /// Worktrees under `~/.co-review/worktrees`, filled in on std threads.
    pub fn new(store: Arc<ReviewStore>) -> Self {
        let spawn: Spawner = Arc::new(|job| {
            std::thread::spawn(job);
        });
        Self::with_options(
            store,
            review_store::co_review_home().join("worktrees"),
            spawn,
        )
    }

    pub fn with_options(store: Arc<ReviewStore>, worktrees_dir: PathBuf, spawn: Spawner) -> Self {
        Self {
            store,
            worktrees_dir,
            spawn,
            worktrees: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashSet::new()),
        }
    }

    /// Follows the store's changes on a thread of its own (see [`Self::handle_change`]) until the
    /// store or these code folders are dropped.
    pub fn start_watching(self: &Arc<Self>) -> JoinHandle<()> {
        let changes = self.store.subscribe_changes();
        let this: Weak<Self> = Arc::downgrade(self);
        std::thread::spawn(move || {
            while let Ok(change) = changes.recv_blocking() {
                let Some(this) = this.upgrade() else {
                    break;
                };
                this.handle_change(&change);
            }
        })
    }

    /// Removes the worktree of a review that was archived or deleted, and checks it out again
    /// when the review is unarchived. Runs git, so call it off the foreground.
    pub fn handle_change(&self, change: &ReviewChange) {
        let review = match change {
            ReviewChange::Deleted { review_id, .. } => {
                let known = self.worktrees.lock().remove(review_id);
                if let Some(known) = known {
                    remove_worktree(&known.repo, &known.code).unwrap_or_else(|error| {
                        log::error!("removing a worktree failed: {error:#}")
                    });
                }
                return;
            }
            ReviewChange::Changed { review } => review,
        };
        let Some(bundle) = &review.bundle else {
            return;
        };
        let (Some(true), Some(repo), Some(code)) = (bundle.worktree, &bundle.repo, &bundle.code)
        else {
            return;
        };
        let (repo, code) = (PathBuf::from(repo), PathBuf::from(code));
        self.worktrees.lock().insert(
            review.id.clone(),
            Worktree {
                repo: repo.clone(),
                code: code.clone(),
            },
        );
        if review.archived_at.is_some() {
            if code.exists() {
                remove_worktree(&repo, &code)
                    .unwrap_or_else(|error| log::error!("removing a worktree failed: {error:#}"));
            }
        } else if let Some(head) = &bundle.head
            && !code.exists()
            && !self.creating.lock().contains(&review.id)
        {
            // Unarchived: the worktree again, at the same head and base.
            if let Err(error) = self.checkout(review, &repo, head, &[], bundle.base.as_deref()) {
                log::error!("recreating a worktree failed: {error:#}");
            }
        }
    }

    /// Gives a change review (the `range` diffed in `repo`) its code folder and records it on the
    /// review. `changed` and `related` (the blast radius) are repository-relative files: their
    /// folders are checked out first. Returns the code folder.
    pub fn prepare(
        &self,
        review: &Review,
        repo: &Path,
        range: &str,
        changed: &[String],
        related: &[String],
    ) -> Result<PathBuf> {
        let sides = sides_of(range);
        let commit_of = |revision: &str| {
            git(
                repo,
                ["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
            )
            .ok()
            .filter(|sha| !sha.is_empty())
        };
        let head = sides.head.as_deref().and_then(commit_of);
        let current = commit_of("HEAD");
        let mut base = commit_of(&sides.base);
        if sides.merge_base
            && let (Some(base_commit), Some(head_commit)) = (&base, &head)
            && let Ok(merge_base) = git(repo, ["merge-base", base_commit, head_commit])
            && !merge_base.is_empty()
        {
            base = Some(merge_base);
        }
        let related = related.to_vec();
        let head = match head {
            Some(head) if Some(&head) != current.as_ref() => head,
            // The reviewer's own change (the working tree, or what the repository has checked
            // out): the repository itself.
            _ => {
                let code = real_path(repo);
                self.record(&review.id, |bundle| {
                    bundle.repo = Some(path_string(repo));
                    bundle.code = Some(path_string(&code));
                    bundle.base = base;
                    bundle.head = None;
                    bundle.worktree = Some(false);
                    bundle.checkout = Some(CheckoutKind::Full);
                    bundle.related = Some(related);
                });
                return Ok(code);
            }
        };
        let files: Vec<String> = changed.iter().chain(&related).cloned().collect();
        let code = self.checkout(review, repo, &head, &files, base.as_deref())?;
        self.record(&review.id, |bundle| bundle.related = Some(related));
        Ok(code)
    }

    /// Where a review's worktree lives: per repository and review, outside the repository.
    fn folder_of(&self, repo: &Path, review_id: &str) -> PathBuf {
        let hash = format!("{:x}", Sha256::digest(path_string(&real_path(repo)).as_bytes()));
        let name = repo
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let review: String = review_id.chars().take(8).collect();
        self.worktrees_dir
            .join(format!("{name}-{}", &hash[..8]))
            .join(review)
    }

    fn checkout(
        &self,
        review: &Review,
        repo: &Path,
        head: &str,
        files: &[String],
        base: Option<&str>,
    ) -> Result<PathBuf> {
        self.creating.lock().insert(review.id.clone());
        let result = self.checkout_worktree(review, repo, head, files, base);
        self.creating.lock().remove(&review.id);
        result
    }

    fn checkout_worktree(
        &self,
        review: &Review,
        repo: &Path,
        head: &str,
        files: &[String],
        base: Option<&str>,
    ) -> Result<PathBuf> {
        let code = self.folder_of(repo, &review.id);
        let (checkout, fill) = if code.join(".git").exists() {
            // Opened again (new commits, or the same): move to the head; a full checkout stays
            // full.
            git(&code, ["checkout", "--quiet", "--detach", "--force", head])
                .with_context(|| format!("git checkout {head} in {}", code.display()))?;
            let checkout = review
                .bundle
                .as_ref()
                .and_then(|bundle| bundle.checkout)
                .unwrap_or(CheckoutKind::Full);
            (checkout, true)
        } else {
            if let Some(parent) = code.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            git(repo, ["worktree", "prune"])
                .map(drop)
                .unwrap_or_else(|error| log::warn!("git worktree prune: {error:#}"));
            git(
                repo,
                [
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--quiet"),
                    OsStr::new("--no-checkout"),
                    OsStr::new("--detach"),
                    code.as_os_str(),
                    OsStr::new(head),
                ],
            )
            .context("git worktree add")?;
            // The neighbourhood first: the folders of the changed and related files (cone mode
            // also has the top-level files and those of each folder's parents).
            let folders: BTreeSet<&str> = files
                .iter()
                .filter_map(|file| file.rsplit_once('/').map(|(folder, _)| folder))
                .filter(|folder| !folder.is_empty() && *folder != ".")
                .collect();
            if !folders.is_empty() {
                let mut arguments = vec!["sparse-checkout", "set", "--cone"];
                arguments.extend(&folders);
                git(&code, arguments).context("git sparse-checkout set")?;
            }
            git(&code, ["checkout", "--quiet", "--detach", head])
                .with_context(|| format!("git checkout {head}"))?;
            if folders.is_empty() {
                (CheckoutKind::Full, false)
            } else {
                (CheckoutKind::Neighbourhood, true)
            }
        };
        let real_code = real_path(&code);
        self.record(&review.id, |bundle| {
            bundle.repo = Some(path_string(repo));
            bundle.code = Some(path_string(&real_code));
            bundle.base = base.map(str::to_string);
            bundle.head = Some(head.to_string());
            bundle.worktree = Some(true);
            bundle.checkout = Some(checkout);
        });
        if fill {
            self.fill(&review.id, &code);
        }
        Ok(real_code)
    }

    /// The rest of the code, in the background, once the neighbourhood is there.
    fn fill(&self, review_id: &str, code: &Path) {
        let store = Arc::clone(&self.store);
        let review_id = review_id.to_string();
        let code = code.to_path_buf();
        (self.spawn)(Box::new(move || {
            if let Err(error) = fill_worktree(&store, &review_id, &code) {
                log::error!(
                    "checking out the rest of {} failed: {error:#}",
                    code.display()
                );
            }
        }));
    }

    /// Updates the latest bundle of a review (no-op when the review has none).
    fn record(&self, review_id: &str, update: impl FnOnce(&mut ReviewBundle)) {
        record_bundle(&self.store, review_id, update)
            .unwrap_or_else(|error| log::error!("recording a review's code failed: {error:#}"));
    }

    /// A comment written in the editor on lines the diff shows (added or context) becomes a
    /// comment on the diff, so it is on the diff page and goes to GitHub like one made there;
    /// anywhere else (`None`) it stays a comment on the file. Only when the lines read as they do
    /// in the diff: a file edited since the diff isn't mapped onto it.
    pub fn on_diff(&self, review: &Review, location: &CodeLocation) -> Option<CodeLocation> {
        let bundle = review.bundle.as_ref()?;
        let code = bundle.code.as_deref()?;
        let range = location.range.as_ref()?;
        let uri = location
            .uri
            .as_deref()
            .filter(|uri| uri.starts_with("file:"))?;
        let start = range.start.line + 1;
        let end = range.end.line + 1;
        let source = location.anchor.as_ref().map(|anchor| anchor.text.clone());
        let text: Option<Vec<&str>> = location
            .anchor
            .as_ref()
            .map(|anchor| anchor.text.split('\n').collect());
        for patch in patches_of(Path::new(&bundle.dir)) {
            let content = fs::read_to_string(&patch).unwrap_or_default();
            let Some(file) = parse_patch(&content).into_iter().find(|file| {
                !file.path.starts_with("..")
                    && file_uri_to_path(uri).is_some_and(|path| Path::new(&path) == Path::new(code).join(&file.path))
            }) else {
                continue;
            };
            let shown: Vec<_> = file
                .rows
                .iter()
                .filter(|row| row.t != "hunk" && row.t != "del" && row.n.is_some())
                .collect();
            let rows: Option<Vec<_>> = (start..=end)
                .map(|line| shown.iter().find(|row| row.n == Some(line)))
                .collect();
            let Some(rows) = rows else {
                continue;
            };
            if let Some(text) = &text
                && rows
                    .iter()
                    .zip(text)
                    .any(|(row, expected)| row.s != *expected)
            {
                continue;
            }
            let mut anchor = PatchAnchor {
                anchor_type: PatchAnchorType::CodeLine,
                path: Some(file.path.clone()),
                line: Some(start),
                start_line: None,
                end_line: None,
                side: Some(PatchSide::New),
                source: source.clone(),
                extra: serde_json::Map::new(),
            };
            if start != end {
                anchor.anchor_type = PatchAnchorType::CodeRange;
                anchor.line = None;
                anchor.start_line = Some(start);
                anchor.end_line = Some(end);
            }
            return Some(CodeLocation {
                kind: LocationKind::Patch,
                uri: Some(path_to_file_uri(&path_string(&patch))),
                range: None,
                symbol: None,
                anchor: Some(CodeAnchor {
                    text: source.unwrap_or_default(),
                    before: None,
                    after: None,
                    syntax: None,
                }),
                doc_anchor: None,
                patch_anchor: Some(anchor),
                tracked: None,
                extra: serde_json::Map::new(),
            });
        }
        None
    }

    /// The base version of a repository-relative file (for the old side and Compare); `None` when
    /// it didn't exist there.
    pub fn base_file(&self, review: &Review, file: &str) -> Option<String> {
        let bundle = review.bundle.as_ref()?;
        let repo = bundle.repo.as_deref()?;
        let base = bundle.base.as_deref()?;
        let file = if cfg!(windows) {
            file.replace('\\', "/")
        } else {
            file.to_string()
        };
        let absolute = file.starts_with('/') || (cfg!(windows) && file.get(1..2) == Some(":"));
        if absolute || file.split('/').any(|segment| segment == "..") {
            return None;
        }
        git_raw(Path::new(repo), ["show", &format!("{base}:{file}")]).ok()
    }
}

fn fill_worktree(store: &ReviewStore, review_id: &str, code: &Path) -> Result<()> {
    let sparse = git(code, ["config", "--get", "core.sparseCheckout"]).unwrap_or_default();
    if sparse == "true" {
        git(code, ["sparse-checkout", "disable"]).context("git sparse-checkout disable")?;
    }
    let full = store
        .get(review_id)
        .and_then(|review| review.bundle)
        .is_none_or(|bundle| bundle.checkout == Some(CheckoutKind::Full));
    if !full {
        record_bundle(store, review_id, |bundle| {
            bundle.checkout = Some(CheckoutKind::Full);
        })?;
    }
    Ok(())
}

fn record_bundle(
    store: &ReviewStore,
    review_id: &str,
    update: impl FnOnce(&mut ReviewBundle),
) -> Result<()> {
    let Some(mut bundle) = store.get(review_id).and_then(|review| review.bundle) else {
        return Ok(());
    };
    update(&mut bundle);
    store.set_bundle(review_id, Some(bundle))?;
    Ok(())
}

fn remove_worktree(repo: &Path, code: &Path) -> Result<()> {
    if !code.exists() {
        return Ok(());
    }
    let removed = git(
        repo,
        [
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--force"),
            code.as_os_str(),
        ],
    );
    if removed.is_err() {
        fs::remove_dir_all(code).with_context(|| format!("removing {}", code.display()))?;
        git(repo, ["worktree", "prune"])
            .map(drop)
            .unwrap_or_else(|error| log::warn!("git worktree prune: {error:#}"));
    }
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `git -C cwd …`, its output trimmed.
fn git<I, S>(cwd: &Path, arguments: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    git_raw(cwd, arguments).map(|output| output.trim().to_string())
}

/// `git -C cwd …`, its output as it is (file contents).
fn git_raw<I, S>(cwd: &Path, arguments: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(arguments);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().context("running git")?;
    if !output.status.success() {
        bail!(
            "git failed in {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
