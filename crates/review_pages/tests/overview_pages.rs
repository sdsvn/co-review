//! Pages on disk: written at once with what is cheap, rewritten when the slow section is built,
//! settled with the fallback when the build fails, following the review as it changes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use parking_lot::Mutex;
use review_graph::{BlastRadius, GraphNode};
use review_model::{Participant, ParticipantKind, ReviewBundle, ReviewScope};
use review_pages::{OverviewPages, SectionBuilder};
use review_store::{CreateReviewParams, ReviewStore, path_to_file_uri};

const INDEXING: &str = "_Co-Review is indexing the repository.";
const GRAPHING: &str = "_Co-Review is building the code graph of the repository (Graphify).";
const REPOSITORY: &str = "# Repository overview: repo\n\nWhere to start.\n\n## Areas\n\n- src";

/// A builder that waits for the test to let each build finish, then succeeds or fails.
struct GatedSections {
    release: Mutex<mpsc::Receiver<()>>,
    succeed: bool,
    calls: AtomicUsize,
}

impl GatedSections {
    fn new(succeed: bool) -> (Arc<Self>, mpsc::Sender<()>) {
        let (sender, receiver) = mpsc::channel();
        let sections = Arc::new(Self {
            release: Mutex::new(receiver),
            succeed,
            calls: AtomicUsize::new(0),
        });
        (sections, sender)
    }

    fn wait(&self) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.release.lock().recv()?;
        if self.succeed {
            Ok(())
        } else {
            Err(anyhow!("the index failed"))
        }
    }
}

impl SectionBuilder for GatedSections {
    fn repository_overview(&self, _root: &Path, _viewed: &HashMap<String, u64>) -> Result<String> {
        self.wait()?;
        Ok(REPOSITORY.to_string())
    }

    fn blast_radius(
        &self,
        _repository: &Path,
        changed_lines: &BTreeMap<String, Vec<u32>>,
    ) -> Result<Option<BlastRadius>> {
        self.wait()?;
        assert_eq!(changed_lines.get("src/lib.rs"), Some(&vec![2]));
        Ok(Some(BlastRadius {
            changed: vec![GraphNode {
                id: "f".to_string(),
                label: "frobnicate".to_string(),
                file: "src/lib.rs".to_string(),
                line: Some(2),
                area: None,
            }],
            affected: Vec::new(),
            edges: Vec::new(),
            files: vec!["src/main.rs".to_string()],
        }))
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    store: Arc<ReviewStore>,
    repository: PathBuf,
    review_dir: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("temp dir");
    let repository = temp.path().join("repo");
    let review_dir = temp.path().join("review");
    std::fs::create_dir_all(&repository).expect("repository");
    std::fs::create_dir_all(&review_dir).expect("review dir");
    Fixture {
        store: Arc::new(ReviewStore::with_home(temp.path().join("home"))),
        repository,
        review_dir,
        _temp: temp,
    }
}

fn create_review(fixture: &Fixture, title: &str) -> String {
    fixture
        .store
        .create(CreateReviewParams {
            workspace_root: path_to_file_uri(&fixture.repository.to_string_lossy()),
            title: title.to_string(),
            scope: ReviewScope::Repository,
            author: Participant {
                id: "human:ann".to_string(),
                kind: ParticipantKind::Human,
                name: "Ann".to_string(),
            },
        })
        .expect("review")
        .id
}

fn create_change_review(fixture: &Fixture) -> String {
    std::fs::write(
        fixture.review_dir.join("change.patch"),
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn frobnicate() {}\n",
    )
    .expect("patch");
    std::fs::write(
        fixture.review_dir.join("PR.md"),
        "title: Frobnicate\nurl: https://github.com/o/r/pull/9\ncommits: 2\n\nIt frobnicates.\n",
    )
    .expect("PR.md");
    let id = create_review(fixture, "Change");
    fixture
        .store
        .set_bundle(
            &id,
            Some(ReviewBundle {
                dir: fixture.review_dir.to_string_lossy().to_string(),
                store_path: None,
                openspec: None,
                doc_version: None,
                github: None,
                code: None,
                repo: Some(fixture.repository.to_string_lossy().to_string()),
                base: None,
                head: None,
                worktree: None,
                checkout: None,
                related: None,
                extra: serde_json::Map::new(),
            }),
        )
        .expect("bundle");
    id
}

/// Waits until the page satisfies `done`, and returns it.
fn eventually(file: &Path, done: impl Fn(&str) -> bool) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let page = std::fs::read_to_string(file).unwrap_or_default();
        if done(&page) {
            return page;
        }
        assert!(Instant::now() < deadline, "the page never settled:\n{page}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn repository_page_is_written_first_then_with_the_repository() {
    let fixture = fixture();
    let id = create_review(&fixture, "Audit");
    let (sections, release) = GatedSections::new(true);
    let pages = OverviewPages::new(fixture.store.clone(), sections.clone());

    let file = pages.write(&id, true).expect("write").expect("a page");
    let first = std::fs::read_to_string(&file).expect("page");
    assert!(first.ends_with(&format!("## The repository\n\n{INDEXING} Where to start, the areas of the code and how they connect appear here when it is ready._\n")));

    // Opened again while the build runs: no second build.
    pages.write(&id, true).expect("write again");
    release.send(()).expect("release");
    let page = eventually(&file, |page| !page.contains(INDEXING));
    assert!(page.ends_with("## The repository\n\nWhere to start.\n\n### Areas\n\n- src\n"));
    assert_eq!(sections.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_repository_build_settles_without_the_section() {
    let fixture = fixture();
    let id = create_review(&fixture, "Audit");
    let (sections, release) = GatedSections::new(false);
    let pages = OverviewPages::new(fixture.store.clone(), sections);

    let file = pages.write(&id, true).expect("write").expect("a page");
    assert!(
        std::fs::read_to_string(&file)
            .expect("page")
            .contains(INDEXING)
    );
    release.send(()).expect("release");
    let page = eventually(&file, |page| !page.contains(INDEXING));
    assert!(!page.contains("## The repository"));
    assert!(page.ends_with("## Findings and comments (0)\n\nNone yet.\n"));
}

#[test]
fn change_page_gets_its_blast_radius_and_related_files() {
    let fixture = fixture();
    let id = create_change_review(&fixture);
    let (sections, release) = GatedSections::new(true);
    let pages = OverviewPages::new(fixture.store.clone(), sections);

    let file = pages.write(&id, true).expect("write").expect("a page");
    let first = std::fs::read_to_string(&file).expect("page");
    assert!(first.contains("## The pull request\n\n[Frobnicate](https://github.com/o/r/pull/9) · 2 commits\n\nIt frobnicates.\n"));
    assert!(first.contains("## The change\n\n1 file, +1 −1.\n\n- `src/lib.rs` +1 −1\n"));
    assert!(first.contains(GRAPHING));

    release.send(()).expect("release");
    let page = eventually(&file, |page| !page.contains(GRAPHING));
    assert!(page.contains(
        "## Blast radius\n\n**Changed** (1): [`frobnicate`](src/lib.rs:2)\n\nNothing else"
    ));
    let related = fixture
        .store
        .get(&id)
        .and_then(|review| review.bundle)
        .and_then(|bundle| bundle.related);
    assert_eq!(related, Some(vec!["src/main.rs".to_string()]));
}

#[test]
fn failed_blast_radius_settles_with_no_graph() {
    let fixture = fixture();
    let id = create_change_review(&fixture);
    let (sections, release) = GatedSections::new(false);
    let pages = OverviewPages::new(fixture.store.clone(), sections);

    let file = pages.write(&id, true).expect("write").expect("a page");
    release.send(()).expect("release");
    let page = eventually(&file, |page| !page.contains(GRAPHING));
    assert!(page.contains("## Blast radius\n\n_With [Graphify](https://graphify.net) installed"));
}

#[test]
fn written_pages_follow_their_review_and_go_with_it() {
    let fixture = fixture();
    let id = create_review(&fixture, "Audit");
    let unwritten = create_review(&fixture, "Never opened");
    let (sections, release) = GatedSections::new(true);
    let pages = OverviewPages::new(fixture.store.clone(), sections.clone());

    let file = pages.write(&id, true).expect("write").expect("a page");
    release.send(()).expect("release");
    eventually(&file, |page| !page.contains(INDEXING));

    // A burst of changes: one rewrite, reusing the built section.
    for round in 0..5 {
        fixture
            .store
            .set_overview(&id, format!("Round {round} of the overview."))
            .expect("overview");
    }
    fixture
        .store
        .set_overview(&unwritten, "Not written".to_string())
        .expect("overview");
    let page = eventually(&file, |page| page.contains("Round 4 of the overview."));
    assert!(page.contains("Where to start."));
    assert_eq!(sections.calls.load(Ordering::SeqCst), 1);
    std::thread::sleep(review_pages::COALESCE_DELAY * 2);
    let unwritten_review = fixture.store.get(&unwritten).expect("review");
    assert!(
        !pages
            .file_of(&unwritten_review.workspace_root, &unwritten)
            .exists()
    );

    fixture.store.delete(&id).expect("delete");
    let directory = file.parent().expect("page directory").to_path_buf();
    let deadline = Instant::now() + Duration::from_secs(5);
    while directory.exists() {
        assert!(Instant::now() < deadline, "the page outlived its review");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn review_directory_without_patches_has_no_overview() {
    let fixture = fixture();
    let id = create_change_review(&fixture);
    std::fs::remove_file(fixture.review_dir.join("change.patch")).expect("remove patch");
    let (sections, _release) = GatedSections::new(true);
    let pages = OverviewPages::new(fixture.store.clone(), sections.clone());
    assert_eq!(pages.write(&id, true).expect("write"), None);
    assert_eq!(pages.write("no-such-review", true).expect("write"), None);
    assert_eq!(sections.calls.load(Ordering::SeqCst), 0);
}
