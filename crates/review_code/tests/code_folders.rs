//! Code folders on tiny temporary repositories. The `onDiff`, `baseFile` and `sidesOf` cases
//! replay `fixtures/code_folders.json`, recorded from the TypeScript CodeFolders by `gen/gen.mjs`
//! over the same scenario (which the fixture carries, so the repository built here matches).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use review_code::{CodeFolders, Spawner, sides_of};
use review_model::{
    CheckoutKind, CodeAnchor, CodeLocation, LocationKind, Participant, ParticipantKind, Position,
    Range, Review, ReviewBundle, ReviewScope,
};
use review_store::{CreateReviewParams, ReviewStore, path_to_file_uri};
use serde_json::{Value, json};
use tempfile::TempDir;

type Jobs = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/code_folders.json")).unwrap()
}

fn git(cwd: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn write_tree(root: &Path, files: &Value) {
    for (file, text) in files.as_object().unwrap() {
        let path = root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text.as_str().unwrap()).unwrap();
    }
}

/// Everything a test touches lives in one temporary folder: the repository, the store's home,
/// the worktrees and the review directory. Dropping it removes them all.
struct Scenario {
    temp: TempDir,
    repo: PathBuf,
    store: Arc<ReviewStore>,
    code_folders: Arc<CodeFolders>,
    /// Background jobs (the rest of each checkout), run when the test says so.
    jobs: Jobs,
}

impl Scenario {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let scenario = &fixture()["scenario"];
        git(&repo, &["init", "--quiet", "-b", "main"]);
        write_tree(&repo, &scenario["base"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "base"]);
        git(&repo, &["checkout", "--quiet", "-b", "feature"]);
        for file in scenario["base"].as_object().unwrap().keys() {
            fs::remove_file(repo.join(file)).unwrap();
        }
        write_tree(&repo, &scenario["head"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "head"]);
        git(&repo, &["checkout", "--quiet", "main"]);

        let store = Arc::new(ReviewStore::with_home(temp.path().join("home")));
        let jobs: Jobs = Arc::default();
        let queue = Arc::clone(&jobs);
        let spawn: Spawner = Arc::new(move |job| queue.lock().push(job));
        let code_folders = Arc::new(CodeFolders::with_options(
            Arc::clone(&store),
            temp.path().join("worktrees"),
            spawn,
        ));
        Self {
            temp,
            repo,
            store,
            code_folders,
            jobs,
        }
    }

    fn run_background_jobs(&self) {
        let jobs: Vec<_> = std::mem::take(&mut *self.jobs.lock());
        for job in jobs {
            job();
        }
    }

    /// A review whose directory holds the scenario's patch, as `open_review` leaves it.
    fn review(&self) -> Review {
        let workspace = self.temp.path().join("review-dir");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            workspace.join("change.patch"),
            fixture()["patch"].as_str().unwrap(),
        )
        .unwrap();
        let review = self
            .store
            .create(CreateReviewParams {
                workspace_root: path_to_file_uri(&workspace.to_string_lossy()),
                title: "A pull request".to_string(),
                scope: ReviewScope::Repository,
                author: Participant {
                    id: "human:test".to_string(),
                    kind: ParticipantKind::Human,
                    name: "Test".to_string(),
                },
            })
            .unwrap();
        let bundle = ReviewBundle {
            dir: workspace.to_string_lossy().into_owned(),
            store_path: None,
            openspec: None,
            doc_version: None,
            github: None,
            code: None,
            repo: None,
            base: None,
            head: None,
            worktree: None,
            checkout: None,
            related: None,
            extra: serde_json::Map::new(),
        };
        self.store.set_bundle(&review.id, Some(bundle)).unwrap()
    }

    fn bundle(&self, review: &Review) -> ReviewBundle {
        self.store.get(&review.id).unwrap().bundle.unwrap()
    }

    fn commit(&self, revision: &str) -> String {
        git(&self.repo, &["rev-parse", revision])
    }

    fn registered_worktrees(&self) -> usize {
        git(&self.repo, &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count()
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sides_match_typescript() {
    for case in fixture()["sides"].as_array().unwrap() {
        let range = case["range"].as_str().unwrap();
        let sides = sides_of(range);
        let actual = json!({
            "base": sides.base,
            "head": sides.head,
            "mergeBase": sides.merge_base,
        });
        let mut expected = case["result"].clone();
        if expected.get("head").is_none() {
            expected["head"] = Value::Null;
        }
        assert_eq!(actual, expected, "range {range:?}");
    }
}

#[test]
fn pull_request_opens_on_its_neighbourhood_before_the_full_checkout() {
    let scenario = Scenario::new();
    let review = scenario.review();
    let code = scenario
        .code_folders
        .prepare(
            &review,
            &scenario.repo,
            "main...feature",
            &strings(&["src/utils/helper.rs"]),
            &[],
        )
        .unwrap();

    assert!(code.starts_with(fs::canonicalize(scenario.temp.path()).unwrap()));
    assert_eq!(
        fs::read_to_string(code.join("src/utils/helper.rs")).unwrap(),
        "pub fn help() {}\n"
    );
    // Cone mode: the parent folder's own files come along, other folders don't yet.
    assert!(
        fs::read_to_string(code.join("src/app.rs"))
            .unwrap()
            .contains("head line 2")
    );
    assert!(!code.join("docs").exists());
    let bundle = scenario.bundle(&review);
    assert_eq!(bundle.checkout, Some(CheckoutKind::Neighbourhood));
    assert_eq!(bundle.worktree, Some(true));
    assert_eq!(bundle.code.as_deref(), Some(&*code.to_string_lossy()));
    assert_eq!(
        bundle.repo.as_deref(),
        Some(&*scenario.repo.to_string_lossy())
    );
    assert_eq!(bundle.head, Some(scenario.commit("feature")));
    assert_eq!(bundle.base, Some(scenario.commit("main")));
    assert_eq!(bundle.related, Some(Vec::new()));
    // The repository is left alone.
    assert_eq!(git(&scenario.repo, &["branch", "--show-current"]), "main");
    assert!(scenario.repo.join("README.md").exists());

    // The window on the code folder lists the review, although its directory is elsewhere.
    let window = scenario
        .store
        .list_for_window(&path_to_file_uri(&code.to_string_lossy()));
    assert_eq!(
        window.iter().map(|review| &review.id).collect::<Vec<_>>(),
        vec![&review.id]
    );

    scenario.run_background_jobs();
    assert_eq!(
        fs::read_to_string(code.join("docs/guide.md")).unwrap(),
        "Guide\n"
    );
    assert!(!code.join("README.md").exists());
    assert_eq!(scenario.bundle(&review).checkout, Some(CheckoutKind::Full));
}

#[test]
fn related_files_are_part_of_the_neighbourhood() {
    let scenario = Scenario::new();
    let review = scenario.review();
    let code = scenario
        .code_folders
        .prepare(
            &review,
            &scenario.repo,
            "main..feature",
            &strings(&["src/utils/helper.rs"]),
            &strings(&["docs/guide.md"]),
        )
        .unwrap();
    assert!(code.join("docs/guide.md").exists());
    let bundle = scenario.bundle(&review);
    assert_eq!(bundle.related, Some(strings(&["docs/guide.md"])));
    assert_eq!(bundle.checkout, Some(CheckoutKind::Neighbourhood));

    // Opened again: the same worktree, moved to the head, still filled in the background.
    let again = scenario
        .code_folders
        .prepare(
            &review,
            &scenario.repo,
            "main..feature",
            &strings(&["src/app.rs"]),
            &[],
        )
        .unwrap();
    assert_eq!(again, code);
    assert_eq!(scenario.registered_worktrees(), 2);
    scenario.run_background_jobs();
    assert_eq!(scenario.bundle(&review).checkout, Some(CheckoutKind::Full));
    assert!(code.join("docs/guide.md").exists());
}

#[test]
fn own_change_is_the_repository_itself() {
    let scenario = Scenario::new();
    for range in ["HEAD", "feature...main", "main~0..main"] {
        let review = scenario.review();
        let code = scenario
            .code_folders
            .prepare(
                &review,
                &scenario.repo,
                range,
                &strings(&["src/app.rs"]),
                &[],
            )
            .unwrap();
        assert_eq!(code, fs::canonicalize(&scenario.repo).unwrap(), "{range}");
        let bundle = scenario.bundle(&review);
        assert_eq!(bundle.worktree, Some(false), "{range}");
        assert_eq!(bundle.head, None, "{range}");
        assert_eq!(bundle.base, Some(scenario.commit("main")), "{range}");
        assert_eq!(bundle.checkout, Some(CheckoutKind::Full), "{range}");
    }
    assert!(!scenario.temp.path().join("worktrees").exists());
    assert!(scenario.jobs.lock().is_empty());
}

#[test]
fn worktree_follows_archive_unarchive_and_delete() {
    let scenario = Scenario::new();
    let watcher = scenario.code_folders.start_watching();
    let review = scenario.review();
    let code = scenario
        .code_folders
        .prepare(
            &review,
            &scenario.repo,
            "main...feature",
            &strings(&["src/app.rs"]),
            &[],
        )
        .unwrap();
    scenario.run_background_jobs();
    assert!(code.join("docs/guide.md").exists());

    scenario.store.archive(&review.id, true).unwrap();
    wait_until("the archived review's worktree is removed", || {
        !code.exists()
    });
    assert_eq!(scenario.registered_worktrees(), 1);

    scenario.store.archive(&review.id, false).unwrap();
    wait_until("the unarchived review's worktree is back", || {
        scenario.bundle(&review).head == Some(scenario.commit("feature"))
            && code.join("docs/guide.md").exists()
            && scenario.registered_worktrees() == 2
    });
    assert!(
        fs::read_to_string(code.join("src/app.rs"))
            .unwrap()
            .contains("inserted line")
    );
    let bundle = scenario.bundle(&review);
    assert_eq!(bundle.checkout, Some(CheckoutKind::Full));
    assert_eq!(bundle.base, Some(scenario.commit("main")));

    scenario.store.delete(&review.id).unwrap();
    wait_until("the deleted review's worktree is removed", || {
        !code.exists() && scenario.registered_worktrees() == 1
    });

    let Scenario {
        temp,
        store,
        code_folders,
        ..
    } = scenario;
    drop(code_folders);
    drop(store);
    watcher.join().unwrap();
    drop(temp);
}

/// The review prepared on its worktree, with the location of each fixture case in it.
fn prepared(scenario: &Scenario) -> (Review, PathBuf) {
    let review = scenario.review();
    let code = scenario
        .code_folders
        .prepare(
            &review,
            &scenario.repo,
            "main...feature",
            &strings(&["src/app.rs"]),
            &[],
        )
        .unwrap();
    (scenario.store.get(&review.id).unwrap(), code)
}

fn location_of(case: &Value, code: &Path) -> CodeLocation {
    let uri = match case.get("uri") {
        Some(uri) => uri.as_str().unwrap().to_string(),
        None => path_to_file_uri(&code.join(case["file"].as_str().unwrap()).to_string_lossy()),
    };
    let range = case.get("lines").map(|lines| {
        let line = |index: usize| Position {
            line: lines[index].as_u64().unwrap() as u32 - 1,
            character: 0,
        };
        Range {
            start: line(0),
            end: line(1),
        }
    });
    CodeLocation {
        kind: LocationKind::Line,
        uri: Some(uri),
        range,
        symbol: None,
        anchor: case.get("text").map(|text| CodeAnchor {
            text: text.as_str().unwrap().to_string(),
            before: None,
            after: None,
            syntax: None,
        }),
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: serde_json::Map::new(),
    }
}

#[test]
fn on_diff_matches_typescript() {
    let scenario = Scenario::new();
    let (review, code) = prepared(&scenario);
    let patch_uri = path_to_file_uri(
        &Path::new(&review.bundle.as_ref().unwrap().dir)
            .join("change.patch")
            .to_string_lossy(),
    );
    let fixture = fixture();
    let mut outcomes = BTreeMap::new();
    for case in fixture["onDiff"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let result = scenario
            .code_folders
            .on_diff(&review, &location_of(case, &code))
            .map(|location| {
                assert_eq!(location.uri.as_deref(), Some(&*patch_uri), "{name}");
                let mut value = serde_json::to_value(location).unwrap();
                value["uri"] = json!("change.patch");
                value
            })
            .unwrap_or(Value::Null);
        assert_eq!(result, case["result"], "{name}");
        outcomes.insert(name, !result.is_null());
    }
    assert!(outcomes.values().any(|on_diff| *on_diff));
    assert!(outcomes.values().any(|on_diff| !*on_diff));
}

#[test]
fn base_reads_match_typescript() {
    let scenario = Scenario::new();
    let (review, _code) = prepared(&scenario);
    for case in fixture()["baseFiles"].as_array().unwrap() {
        let file = case["file"].as_str().unwrap();
        let expected = case["result"].as_str().map(str::to_string);
        assert_eq!(
            scenario.code_folders.base_file(&review, file),
            expected,
            "{file}"
        );
    }
}
