#![allow(clippy::unwrap_used)]
//! Replays the review-directory scenarios of `fixtures/service/script.json` against the Rust
//! `BundleService` and compares with what the TypeScript app recorded.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use review_bundle::findings::Finding;
use review_bundle::payloads::accepted_suggestions;
use review_bundle::service::document_of;
use review_bundle::{BundleService, BundleServiceOptions, OpenBundleParams};
use review_model::{Participant, ParticipantKind};
use review_store::ReviewStore;
use serde::Deserialize;
use serde_json::{Map, Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service")
}

fn author() -> Participant {
    Participant {
        id: "agent:test".into(),
        kind: ParticipantKind::Agent,
        name: "test-agent".into(),
    }
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    input: Option<String>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Step {
    #[serde(rename_all = "camelCase")]
    Open {
        title: Option<String>,
        store_path: Option<String>,
        openspec: Option<String>,
        markdown: Option<String>,
        patch: Option<String>,
        patch_name: Option<String>,
    },
    AddFinding {
        finding: Finding,
    },
    #[serde(rename_all = "camelCase")]
    Decide {
        source_id: String,
        accept: bool,
    },
    Sync,
    Append {
        file: String,
        text: String,
    },
    Export,
    Snapshot {
        label: String,
    },
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn files_of(dir: &Path, skip: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, skip: &Path, out: &mut BTreeMap<String, String>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, skip, out);
            } else if path != skip {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.insert(relative, fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, skip, &mut out);
    out
}

/// Mirrors the recorder's `normalize`: sorted keys, uuids numbered in order of
/// appearance, times zeroed, clocks and the temporary root replaced.
fn normalize(value: &Value, root: &str) -> Value {
    fn is_uuid(text: &str) -> bool {
        let groups: Vec<&str> = text.split('-').collect();
        groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
            && groups
                .iter()
                .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
    }
    fn is_clock(text: &str) -> bool {
        let bytes = text.as_bytes();
        bytes.len() == 5
            && bytes[2] == b':'
            && [0, 1, 3, 4]
                .iter()
                .all(|&index| bytes[index].is_ascii_digit())
    }
    fn visit(value: &Value, key: &str, root: &str, ids: &mut Vec<String>) -> Value {
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| visit(item, "", root, ids))
                    .collect(),
            ),
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for key in keys {
                    out.insert(key.clone(), visit(&map[key], key, root, ids));
                }
                Value::Object(out)
            }
            Value::Number(_)
                if matches!(
                    key,
                    "createdAt" | "updatedAt" | "at" | "archivedAt" | "postedAt" | "submittedAt"
                ) =>
            {
                json!(0)
            }
            Value::String(text) if is_uuid(text) => {
                let index = ids.iter().position(|id| id == text).unwrap_or_else(|| {
                    ids.push(text.clone());
                    ids.len() - 1
                });
                Value::String(format!("<ID{index}>"))
            }
            Value::String(text) if matches!(key, "time" | "submittedAt") && is_clock(text) => {
                Value::String("<CLOCK>".into())
            }
            Value::String(text) => Value::String(text.replace(root, "<ROOT>")),
            other => other.clone(),
        }
    }
    visit(value, "", root, &mut Vec::new())
}

fn run(scenario: &Scenario, root: &Path) -> Value {
    let home = root.join("home");
    let store = Arc::new(ReviewStore::with_home(home.clone()));
    let service = BundleService::with_options(
        store.clone(),
        BundleServiceOptions {
            home,
            export_on_change: false,
        },
    );
    let case_dir = root.join("cases").join(&scenario.name);
    if let Some(input) = &scenario.input {
        copy_dir(&fixtures().join(input), &case_dir);
    }
    let mut review_id = String::new();
    let mut dir = PathBuf::new();
    let mut opened = Vec::new();
    let mut synced = Vec::new();
    let mut snapshots = Vec::new();
    let thread_of = |review_id: &str, source_id: &str| {
        store
            .get(review_id)
            .unwrap()
            .threads
            .into_iter()
            .find(|thread| thread.source_id.as_deref() == Some(source_id))
            .unwrap()
            .id
    };

    for step in &scenario.steps {
        match step {
            Step::Open {
                title,
                store_path,
                openspec,
                markdown,
                patch,
                patch_name,
            } => {
                let result = service
                    .open(OpenBundleParams {
                        dir: scenario.input.as_ref().map(|_| case_dir.clone()),
                        markdown: markdown.clone(),
                        patch: patch.clone(),
                        patch_name: patch_name.clone(),
                        title: title.clone(),
                        store_path: store_path.as_ref().map(|path| case_dir.join(path)),
                        openspec: openspec.as_ref().map(|path| case_dir.join(path)),
                        repo: None,
                        author: author(),
                    })
                    .unwrap();
                // The scenario drives syncing itself; a live watcher would race with it.
                service.unwatch_all();
                review_id = result.review.id.clone();
                dir = result.dir.clone();
                opened.push(json!({
                    "dir": result.dir,
                    "docFile": result.doc_file,
                    "patches": result.patches,
                    "threads": result.review.threads.len(),
                }));
            }
            Step::AddFinding { finding } => {
                service
                    .add_finding(
                        &review_id,
                        &dir,
                        document_of(&dir).as_deref(),
                        finding,
                        None,
                    )
                    .unwrap();
            }
            Step::Decide { source_id, accept } => {
                service
                    .decide_proposal(&review_id, &thread_of(&review_id, source_id), *accept)
                    .unwrap();
            }
            Step::Sync => synced.push(service.sync_findings(&review_id).unwrap()),
            Step::Append { file, text } => {
                let file = dir.join(file);
                let existing = fs::read_to_string(&file).unwrap_or_default();
                fs::write(&file, existing + text).unwrap();
            }
            Step::Export => service.export(&review_id).unwrap(),
            Step::Snapshot { label } => {
                let review = store.get(&review_id).unwrap();
                let state_file = review
                    .bundle
                    .as_ref()
                    .and_then(|bundle| bundle.store_path.clone())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| dir.join("review.json"));
                let state: Value = fs::read_to_string(&state_file)
                    .map(|text| serde_json::from_str(&text).unwrap())
                    .unwrap_or(Value::Null);
                snapshots.push(json!({
                    "label": label,
                    "review": review,
                    "stateFile": state,
                    "files": files_of(&dir, &state_file),
                    "acceptedSuggestions": accepted_suggestions(&review),
                }));
            }
        }
    }
    let record = json!({ "opened": opened, "synced": synced, "snapshots": snapshots });
    normalize(&record, &root.to_string_lossy())
}

#[test]
fn scenarios_match_the_typescript_app() {
    let script: Vec<Scenario> =
        serde_json::from_str(&fs::read_to_string(fixtures().join("script.json")).unwrap()).unwrap();
    let expected: Value =
        serde_json::from_str(&fs::read_to_string(fixtures().join("expected.json")).unwrap())
            .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temporary.path()).unwrap();
    for scenario in &script {
        let actual = run(scenario, &root);
        let expected = &expected[&scenario.name];
        assert_eq!(
            serde_json::to_string_pretty(&actual).unwrap(),
            serde_json::to_string_pretty(expected).unwrap(),
            "scenario {}",
            scenario.name
        );
    }
}

#[test]
fn findings_files_are_ingested_while_the_review_is_open() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temporary.path()).unwrap();
    let dir = root.join("change");
    copy_dir(&fixtures().join("change"), &dir);
    let store = Arc::new(ReviewStore::with_home(root.join("home")));
    let service = BundleService::with_options(
        store.clone(),
        BundleServiceOptions {
            home: root.join("home"),
            export_on_change: true,
        },
    );
    let opened = service.open(OpenBundleParams::dir(&dir, author())).unwrap();
    assert_eq!(opened.review.threads.len(), 3);

    let findings = dir.join("change.comments.jsonl");
    let text = fs::read_to_string(&findings).unwrap()
        + "{\"id\":\"f-1\",\"anchor\":{\"type\":\"patch\"},\"body\":\"duplicate\"}\n"
        + "{\"id\":\"live\",\"anchor\":{\"type\":\"patch\"},\"body\":\"Added while open.\"}\n";
    fs::write(&findings, text).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let threads = loop {
        let threads = store.get(&opened.review.id).unwrap().threads;
        if threads.len() > 3 || Instant::now() > deadline {
            break threads;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let sources: Vec<_> = threads
        .iter()
        .filter_map(|thread| thread.source_id.clone())
        .collect();
    assert_eq!(sources, ["f-1", "f-2", "f-3", "live"]);

    // And review.json follows the review without an explicit export.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state: Option<Value> = fs::read_to_string(dir.join("review.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        if state.as_ref().is_some_and(|state| state["seq"] == json!(4)) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "review.json was not exported: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // Unwatched: later findings wait for the next open or sync.
    service.unwatch(&opened.dir);
    let text = fs::read_to_string(&findings).unwrap()
        + "{\"id\":\"late\",\"anchor\":{\"type\":\"patch\"},\"body\":\"Later.\"}\n";
    fs::write(&findings, text).unwrap();
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(store.get(&opened.review.id).unwrap().threads.len(), 4);
    assert_eq!(service.sync_findings(&opened.review.id).unwrap(), 1);
}

#[test]
fn a_findings_file_anchor_the_model_does_not_know_is_kept_exactly() {
    let temporary = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temporary.path()).unwrap();
    let dir = root.join("change");
    copy_dir(&fixtures().join("change"), &dir);
    let anchors = [
        r#"{"type":"text","exact":"refill int","prefix":"\t"}"#,
        r#"{"exact":"no type at all"}"#,
        r#"{"type":"code-line","path":"src/limiter.go","line":"5","side":"both"}"#,
        r#"{"type":"patch","source":null}"#,
    ];
    let text = anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| {
            format!("{{\"id\":\"odd-{index}\",\"anchor\":{anchor},\"body\":\"x\"}}\n")
        })
        .collect::<String>();
    fs::write(dir.join("change.comments.jsonl"), text).unwrap();
    let store = Arc::new(ReviewStore::with_home(root.join("home")));
    let service = BundleService::with_options(
        store.clone(),
        BundleServiceOptions {
            home: root.join("home"),
            export_on_change: false,
        },
    );
    let opened = service.open(OpenBundleParams::dir(&dir, author())).unwrap();
    service.unwatch_all();
    service.export(&opened.review.id).unwrap();

    let state: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("review.json")).unwrap()).unwrap();
    let threads = state["threads"].as_array().unwrap();
    assert_eq!(threads.len(), anchors.len());
    for (thread, anchor) in threads.iter().zip(anchors) {
        // Like the TypeScript app: a patch's findings are on the patch, with the anchor as written.
        assert_eq!(thread["target"], json!("patch:change"));
        assert_eq!(serde_json::to_string(&thread["anchor"]).unwrap(), anchor);
    }
    // The same holds for the stored review.
    let review = store.get(&opened.review.id).unwrap();
    for (thread, anchor) in review.threads.iter().zip(anchors) {
        let stored = thread.location.patch_anchor.as_ref().unwrap();
        assert_eq!(serde_json::to_string(stored).unwrap(), anchor);
    }
}
