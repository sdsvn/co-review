//! `co-review status [dir]`.

use std::path::{Path, PathBuf};

use review_model::{ParticipantKind, Review, ThreadStatus};
use review_store::{ReviewStore, file_uri_to_path, path_to_file_uri, real_path};
use serde::Serialize;
use serde_json::Value;

use crate::installation::{self, normalize, read_json};

/// An agent integration as its harness recorded it.
#[derive(Debug, PartialEq, Eq)]
pub struct Integration {
    pub harness: String,
    pub version: Option<String>,
    pub location: Option<PathBuf>,
    /// The command that brings it to the app's version.
    pub fix: String,
}

/// Where the harnesses keep their records.
pub struct HarnessDirectories {
    pub home: PathBuf,
    pub claude: PathBuf,
    pub pi: PathBuf,
}

impl HarnessDirectories {
    pub fn from_environment() -> Self {
        let home = installation::user_home();
        let from = |variable: &str, default: PathBuf| {
            std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default)
        };
        HarnessDirectories {
            claude: from("CLAUDE_CONFIG_DIR", home.join(".claude")),
            pi: from("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            home,
        }
    }
}

/// The Co-Review integrations each agent harness has installed, read from the harness's own records.
/// `bundled_pi` is the Pi package this Co-Review ships.
pub fn integrations(
    directories: &HarnessDirectories,
    bundled_pi: Option<&Path>,
) -> Vec<Integration> {
    let mut found = Vec::new();
    // Claude Code: the plugin from the co-review marketplace, per scope it is installed in.
    let claude_plugins = read_json(&directories.claude.join("plugins/installed_plugins.json"));
    if let Some(installs) = claude_plugins
        .as_ref()
        .and_then(|plugins| plugins.pointer("/plugins/co-review@co-review"))
        .and_then(Value::as_array)
    {
        for install in installs {
            let scope = install
                .get("scope")
                .map(display_value)
                .unwrap_or_else(|| "undefined".to_string());
            found.push(Integration {
                harness: format!("Claude Code plugin ({scope})"),
                version: install.get("version").and_then(Value::as_str).map(str::to_string),
                location: None,
                fix: "claude plugin marketplace update co-review && claude plugin update co-review@co-review"
                    .to_string(),
            });
        }
    }
    // Oh My Pi: the version omp recorded when the package was installed (what `omp plugin list` shows).
    if let Some(omp) = read_json(&directories.home.join(".omp/plugins/omp-plugins.lock.json"))
        .and_then(|lock| lock.pointer("/plugins/co-review-omp").cloned())
    {
        found.push(Integration {
            harness: "Oh My Pi package".to_string(),
            version: omp
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            location: None,
            fix: "co-review setup omp".to_string(),
        });
    }
    // Pi: packages are paths (relative to the agent dir) read live, so the version is that path's package.json.
    let packages = read_json(&directories.pi.join("settings.json"))
        .and_then(|settings| settings.get("packages").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    for entry in packages {
        let source = match &entry {
            Value::String(source) => source.clone(),
            Value::Object(object) => match object.get("source").and_then(Value::as_str) {
                Some(source) => source.to_string(),
                None => continue,
            },
            _ => continue,
        };
        if ["npm:", "git:", "http:", "https:"]
            .iter()
            .any(|scheme| source.starts_with(scheme))
        {
            continue;
        }
        let expanded = match source.strip_prefix("~/") {
            Some(rest) => directories.home.join(rest),
            None => PathBuf::from(&source),
        };
        let directory = normalize(&directories.pi.join(expanded));
        let Some(package) = read_json(&directory.join("package.json")) else {
            continue;
        };
        if package.get("name").and_then(Value::as_str) != Some("co-review-pi") {
            continue;
        }
        let fix = if bundled_pi.is_some_and(|bundled| normalize(bundled) == directory) {
            "update Co-Review (the package it ships is out of date)".to_string()
        } else {
            format!("pi remove {source} && co-review setup pi")
        };
        found.push(Integration {
            harness: "Pi package".to_string(),
            version: package
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            location: Some(directory),
            fix,
        });
    }
    found
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The version lines: this Co-Review's, then each integration's, flagging any that differ.
pub fn version_report(app_version: &str, found: &[Integration]) -> Vec<String> {
    let mut lines = vec![format!("co-review {app_version}")];
    for integration in found {
        let location = integration
            .location
            .as_ref()
            .map(|location| format!(" ({})", location.display()))
            .unwrap_or_default();
        lines.push(match &integration.version {
            Some(version) if version == app_version => {
                format!("  {}: {version}{location}", integration.harness)
            }
            version => format!(
                "  {}: {}{location}  ← differs from the app; update it with: {}",
                integration.harness,
                version.as_deref().unwrap_or("unknown"),
                integration.fix
            ),
        });
    }
    if found.is_empty() {
        lines.push(
            "  no agent integrations installed (see https://sdsvn.github.io/co-review/#setup)"
                .to_string(),
        );
    }
    lines
}

/// A review of the repository (or of a review directory inside it) with threads still to deal with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenReview {
    pub id: String,
    pub title: String,
    /// The review directory, or the repository the review is of.
    pub root: String,
    pub open: usize,
    pub proposed: usize,
    /// Open threads whose last message is the reviewer's.
    pub needs_reply: usize,
}

/// The reviews among `reviews` that are of `root` (a real path) or of a review directory inside it, not archived,
/// with an open or proposed thread.
pub fn open_reviews(reviews: &[Review], root: &str) -> Vec<OpenReview> {
    let inside = format!("{root}{}", std::path::MAIN_SEPARATOR);
    reviews
        .iter()
        .filter(|review| review.archived_at.is_none())
        .filter_map(|review| {
            let directory = match &review.bundle {
                Some(bundle) => bundle.dir.clone(),
                None => file_uri_to_path(&review.workspace_root)?,
            };
            if directory != root && !directory.starts_with(&inside) {
                return None;
            }
            let count = |status: ThreadStatus| {
                review
                    .threads
                    .iter()
                    .filter(|thread| thread.status == status)
                    .count()
            };
            let (open, proposed) = (count(ThreadStatus::Open), count(ThreadStatus::Proposed));
            if open + proposed == 0 {
                return None;
            }
            let needs_reply = review
                .threads
                .iter()
                .filter(|thread| {
                    thread.status == ThreadStatus::Open
                        && thread
                            .messages
                            .last()
                            .is_some_and(|message| message.author.kind == ParticipantKind::Human)
                })
                .count();
            Some(OpenReview {
                id: review.id.clone(),
                title: review.title.clone(),
                root: directory,
                open,
                proposed,
                needs_reply,
            })
        })
        .collect()
}

/// Claude Code SessionStart hook output telling the session about `reviews` of `root`; None when there are none.
pub fn claude_hook_output(reviews: &[OpenReview], root: &str) -> Option<String> {
    if reviews.is_empty() {
        return None;
    }
    let mut lines = vec!["Co-Review has open reviews for this repository:".to_string()];
    for review in reviews {
        let mut line = format!(
            "- \"{}\": {} question(s) waiting for an answer, {} open thread(s)",
            review.title, review.needs_reply, review.open
        );
        if review.proposed > 0 {
            line.push_str(&format!(", {} proposed finding(s)", review.proposed));
        }
        if review.root != root {
            let relative = Path::new(&review.root)
                .strip_prefix(root)
                .unwrap_or(Path::new(&review.root));
            line.push_str(&format!(" (review directory {})", relative.display()));
        }
        lines.push(line);
    }
    lines.push(
        "If the user wants to continue reviewing, rejoin with the co-review skill (open_review with this repository, \
         or its `dir`), answer the waiting questions, and wait for their verdict."
            .to_string(),
    );
    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": lines.join("\n"),
        }
    });
    Some(output.to_string())
}

/// `root` with symbolic links resolved, written as the store writes the paths of its file URIs (on Windows, a
/// lowercase drive letter), so it compares equal to them.
pub fn review_root(root: &Path) -> String {
    let real = real_path(root).to_string_lossy().into_owned();
    file_uri_to_path(&path_to_file_uri(&real)).unwrap_or(real)
}

/// What `status` prints about the open reviews of `root` (`display_root` as the user gave it).
pub fn reviews_report(reviews: &[OpenReview], display_root: &Path) -> String {
    if reviews.is_empty() {
        return format!("No open reviews of {}.", display_root.display());
    }
    let json = serde_json::to_string_pretty(reviews).unwrap_or_default();
    format!("Open reviews of {}:\n{json}", display_root.display())
}

/// This Co-Review's version and each installed integration's (flagging any that differ), then the open reviews of
/// `root`, read from the review data (Co-Review need not run). With `claude_hook`, only the reviews (for the hook).
pub fn run(root: &Path, claude_hook: bool) {
    if !claude_hook {
        let bundled_pi = installation::distribution_root().map(|root| root.join("integrations/pi"));
        let found = integrations(
            &HarnessDirectories::from_environment(),
            bundled_pi.as_deref(),
        );
        for line in version_report(&installation::version(), &found) {
            println!("{line}");
        }
    }
    let real_root = review_root(root);
    let store = ReviewStore::with_home(installation::co_review_home());
    let reviews = open_reviews(&store.list_all(), &real_root);
    if claude_hook {
        if let Some(output) = claude_hook_output(&reviews, &real_root) {
            println!("{output}");
        }
    } else {
        println!("{}", reviews_report(&reviews, root));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use review_model::{CodeLocation, LocationKind, Participant, ReviewScope, ThreadOptions};
    use review_store::CreateReviewParams;
    use serde_json::json;
    use std::fs;

    /// A path as the store gives it back from a file URI, so the tests hold on every platform.
    fn store_path(path: &Path) -> String {
        file_uri_to_path(&path_to_file_uri(&path.to_string_lossy())).unwrap_or_default()
    }

    fn root() -> String {
        store_path(&std::env::temp_dir().join("repository"))
    }

    fn inside(root: &str, name: &str) -> String {
        format!("{root}{}{name}", std::path::MAIN_SEPARATOR)
    }

    /// A review of `directory` (a review directory when `bundle`) whose threads have these statuses, each last
    /// answered by a person (`true`) or an agent.
    fn review(
        title: &str,
        directory: &str,
        bundle: bool,
        archived: bool,
        threads: &[(&str, bool)],
    ) -> Review {
        let threads: Vec<Value> = threads
            .iter()
            .enumerate()
            .map(|(index, (status, by_human))| {
                let kind = if *by_human { "human" } else { "agent" };
                json!({
                    "id": format!("thread-{index}"), "number": index + 1, "location": { "kind": "repository" },
                    "messages": [{ "id": "message", "author": { "id": kind, "kind": kind, "name": kind },
                                   "body": "text", "createdAt": 1 }],
                    "status": status, "intent": "comment", "createdAt": 1, "updatedAt": 1,
                })
            })
            .collect();
        let mut review = json!({
            "id": format!("id-{title}"), "title": title,
            "workspaceRoot": path_to_file_uri(if bundle { "/elsewhere" } else { directory }),
            "scope": { "kind": "repository" }, "participants": [], "threads": threads, "activity": [],
            "nextThreadNumber": 1, "createdAt": 1, "updatedAt": 1,
        });
        if bundle {
            review["bundle"] = json!({ "dir": directory });
        }
        if archived {
            review["archivedAt"] = json!(2);
        }
        serde_json::from_value(review).expect("a review")
    }

    #[test]
    fn only_unarchived_reviews_of_the_root_or_inside_it_with_work_left_are_open() {
        let root = root();
        let reviews = vec![
            review("of the root", &root, false, false, &[("open", true)]),
            review("archived", &root, false, true, &[("open", true)]),
            review(
                "outside",
                &store_path(&std::env::temp_dir().join("other")),
                false,
                false,
                &[("open", true)],
            ),
            review(
                "prefix only",
                &format!("{root}-two"),
                false,
                false,
                &[("open", true)],
            ),
            review("all resolved", &root, false, false, &[("resolved", true)]),
            review("no threads", &root, false, false, &[]),
            review(
                "review directory",
                &inside(&root, "changes/one"),
                true,
                false,
                &[("proposed", false)],
            ),
        ];
        let titles: Vec<String> = open_reviews(&reviews, &root)
            .into_iter()
            .map(|review| review.title)
            .collect();
        assert_eq!(titles, vec!["of the root", "review directory"]);
    }

    #[test]
    fn counts_open_proposed_and_open_threads_waiting_for_an_answer() {
        let root = root();
        let reviews = vec![review(
            "counts",
            &root,
            false,
            false,
            &[
                ("open", true),
                ("open", true),
                ("open", false),
                ("proposed", true),
                ("proposed", false),
                ("resolved", true),
            ],
        )];
        assert_eq!(
            open_reviews(&reviews, &root),
            vec![OpenReview {
                id: "id-counts".to_string(),
                title: "counts".to_string(),
                root: root.clone(),
                open: 3,
                proposed: 2,
                needs_reply: 2,
            }]
        );
    }

    #[test]
    fn the_claude_hook_names_each_review_and_its_review_directory() {
        let root = root();
        let summary = |title: &str, directory: String, proposed| OpenReview {
            id: title.to_string(),
            title: title.to_string(),
            root: directory,
            open: 2,
            proposed,
            needs_reply: 1,
        };
        assert_eq!(claude_hook_output(&[], &root), None);
        let output = claude_hook_output(
            &[
                summary("Repository", root.clone(), 0),
                summary("Change", inside(&root, "review"), 3),
            ],
            &root,
        )
        .expect("output");
        let parsed: Value = serde_json::from_str(&output).expect("JSON");
        assert_eq!(
            parsed,
            json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": ([
                "Co-Review has open reviews for this repository:",
                "- \"Repository\": 1 question(s) waiting for an answer, 2 open thread(s)",
                "- \"Change\": 1 question(s) waiting for an answer, 2 open thread(s), 3 proposed finding(s) (review directory review)",
                "If the user wants to continue reviewing, rejoin with the co-review skill (open_review with this repository, or its `dir`), answer the waiting questions, and wait for their verdict.",
            ].join("\n")) } })
        );
        assert!(!output.contains('\n'), "one line, as express wrote it");
        assert!(output.starts_with(
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"#
        ));
    }

    #[test]
    fn the_report_prints_the_reviews_as_indented_json_or_says_there_are_none() {
        let root = PathBuf::from("/work/repository");
        assert_eq!(
            reviews_report(&[], &root),
            "No open reviews of /work/repository."
        );
        let report = reviews_report(
            &[OpenReview {
                id: "a".to_string(),
                title: "T".to_string(),
                root: "/work/repository".to_string(),
                open: 1,
                proposed: 0,
                needs_reply: 1,
            }],
            &root,
        );
        assert_eq!(
            report,
            "Open reviews of /work/repository:\n[\n  {\n    \"id\": \"a\",\n    \"title\": \"T\",\n    \"root\": \"/work/repository\",\n    \"open\": 1,\n    \"proposed\": 0,\n    \"needsReply\": 1\n  }\n]"
        );
    }

    #[test]
    fn reviews_written_by_the_store_are_read_back() {
        let home = tempfile::tempdir().expect("temporary directory");
        let repository = home.path().join("repository");
        fs::create_dir_all(&repository).expect("directory");
        let root = review_root(&repository);
        let store = ReviewStore::with_home(home.path().join("co-review"));
        let person = Participant {
            id: "human:reviewer".to_string(),
            kind: ParticipantKind::Human,
            name: "Reviewer".to_string(),
        };
        let agent = Participant {
            id: "agent:claude".to_string(),
            kind: ParticipantKind::Agent,
            name: "Claude".to_string(),
        };
        let location = CodeLocation {
            kind: LocationKind::Repository,
            uri: None,
            range: None,
            symbol: None,
            anchor: None,
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: serde_json::Map::new(),
        };
        let workspace_root = path_to_file_uri(&repository.to_string_lossy());
        let created = store
            .create(CreateReviewParams {
                workspace_root: workspace_root.clone(),
                title: "Audit".to_string(),
                scope: ReviewScope::Repository,
                author: person.clone(),
            })
            .expect("review");
        store
            .create_thread(
                &created.id,
                location.clone(),
                "Why?".to_string(),
                person.clone(),
                ThreadOptions::default(),
            )
            .expect("thread");
        let (_, answered) = store
            .create_thread(
                &created.id,
                location.clone(),
                "And this?".to_string(),
                person.clone(),
                ThreadOptions::default(),
            )
            .expect("thread");
        store
            .add_message(
                &created.id,
                &answered.id,
                "Because.".to_string(),
                agent.clone(),
                None,
            )
            .expect("message");
        store
            .create_thread(
                &created.id,
                location.clone(),
                "A finding".to_string(),
                agent,
                ThreadOptions {
                    status: Some(ThreadStatus::Proposed),
                    ..ThreadOptions::default()
                },
            )
            .expect("thread");
        let archived = store
            .create(CreateReviewParams {
                workspace_root: workspace_root.clone(),
                title: "Old".to_string(),
                scope: ReviewScope::Repository,
                author: person.clone(),
            })
            .expect("review");
        store
            .create_thread(
                &archived.id,
                location,
                "Old".to_string(),
                person,
                ThreadOptions::default(),
            )
            .expect("thread");
        store.archive(&archived.id, true).expect("archive");

        let reviews = open_reviews(&store.list_all(), &root);
        assert_eq!(
            reviews,
            vec![OpenReview {
                id: created.id,
                title: "Audit".to_string(),
                root,
                open: 2,
                proposed: 1,
                needs_reply: 1,
            }]
        );
    }

    #[test]
    fn integrations_of_another_version_are_flagged_with_their_fix() {
        let home = tempfile::tempdir().expect("temporary directory");
        let directories = HarnessDirectories {
            home: home.path().to_path_buf(),
            claude: home.path().join(".claude"),
            pi: home.path().join(".pi/agent"),
        };
        fs::create_dir_all(directories.claude.join("plugins")).expect("directory");
        fs::write(
            directories.claude.join("plugins/installed_plugins.json"),
            r#"{"plugins":{"co-review@co-review":[{"scope":"user","version":"0.6.2"},{"scope":"project","version":"0.6.3"}]}}"#,
        )
        .expect("write");
        let bundled = home.path().join("app/integrations/pi");
        fs::create_dir_all(&bundled).expect("directory");
        fs::write(
            bundled.join("package.json"),
            r#"{"name":"co-review-pi","version":"0.6.1"}"#,
        )
        .expect("write");
        fs::create_dir_all(&directories.pi).expect("directory");
        fs::write(
            directories.pi.join("settings.json"),
            r#"{"packages":["npm:other","../../app/integrations/pi"]}"#,
        )
        .expect("write");

        let found = integrations(&directories, Some(&bundled));
        let report = version_report("0.6.3", &found);
        assert_eq!(report[0], "co-review 0.6.3");
        assert_eq!(
            report[1],
            "  Claude Code plugin (user): 0.6.2  ← differs from the app; update it with: \
             claude plugin marketplace update co-review && claude plugin update co-review@co-review"
        );
        assert_eq!(report[2], "  Claude Code plugin (project): 0.6.3");
        assert_eq!(
            report[3],
            format!(
                "  Pi package: 0.6.1 ({})  ← differs from the app; update it with: update Co-Review (the package it ships is out of date)",
                bundled.display()
            )
        );
        assert_eq!(report.len(), 4);
    }

    #[test]
    fn no_integrations_points_to_setup() {
        assert_eq!(
            version_report("0.6.3", &[]),
            vec![
                "co-review 0.6.3",
                "  no agent integrations installed (see https://sdsvn.github.io/co-review/#setup)"
            ]
        );
    }
}
