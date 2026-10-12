//! Replays MCP sessions recorded from the Theia app (`tests/conformance/*.jsonl`) against
//! review_mcp over Streamable HTTP, with the same scripted reviewer acting through the store, and compares every
//! result. Ids, times and scratch paths are normalized.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use review_mcp::{ServerHandle, ServerOptions, Services};
use review_model::{
    CodeLocation, Participant, ParticipantKind, PermissionRequest, Review, ReviewDecision,
    ThreadIntent, ThreadOptions,
};
use review_store::ReviewStore;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, CustomNotification, Implementation,
    ProtocolVersion,
};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt as _};
use serde_json::{Map, Value, json};

/// The scratch directory every workflow works in (`{tmp}`); Co-Review's home is `{tmp}/home`, as when recording.
fn scratch() -> &'static Path {
    static SCRATCH: OnceLock<tempfile::TempDir> = OnceLock::new();
    SCRATCH
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            // SAFETY: set once, before any test reads it.
            unsafe {
                std::env::set_var(
                    "CO_REVIEW_HOME",
                    dir.path().canonicalize().unwrap().join("home"),
                )
            };
            dir
        })
        .path()
}

fn tmp() -> String {
    scratch()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

struct Placeholders(Vec<(String, String)>);

impl Placeholders {
    fn substitute(&self, text: &str, review_id: &str) -> String {
        let mut text = text.to_string();
        for (name, value) in &self.0 {
            text = text.replace(&format!("{{{name}}}"), value);
        }
        text.replace("{reviewId}", review_id)
    }

    fn substitute_value(&self, value: &Value, review_id: &str) -> Value {
        match value {
            Value::String(text) => Value::String(self.substitute(text, review_id)),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.substitute_value(item, review_id))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), self.substitute_value(value, review_id)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

/// Collects Claude Code channel notifications.
#[derive(Clone, Default)]
struct Recorder {
    name: String,
    notifications: Arc<Mutex<Vec<Value>>>,
    arrived: Arc<tokio::sync::Notify>,
}

impl Recorder {
    /// Takes the notifications once there are `count` (or what came within 10 s).
    async fn take(&self, count: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.notifications.lock().unwrap().len() < count
            && tokio::time::timeout_at(deadline, self.arrived.notified())
                .await
                .is_ok()
        {}
        self.notifications.lock().unwrap().drain(..).collect()
    }
}

impl ClientHandler for Recorder {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(self.name.clone(), "1.0.0"),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        self.notifications
            .lock()
            .unwrap()
            .push(json!({ "method": notification.method, "params": notification.params }));
        self.arrived.notify_one();
    }
}

fn reviewer() -> Participant {
    Participant {
        id: "human:reviewer@example.com".to_string(),
        kind: ParticipantKind::Human,
        name: "Reviewer".to_string(),
    }
}

struct Replay {
    server: ServerHandle,
    store: Arc<ReviewStore>,
    services: Services,
    placeholders: Placeholders,
    review_id: String,
    mismatches: Vec<String>,
}

impl Replay {
    fn thread_id(&self, reference: &str) -> String {
        let number: u32 = reference.trim_start_matches('t').parse().unwrap();
        let review = self.store.get(&self.review_id).unwrap();
        review
            .threads
            .iter()
            .find(|thread| thread.number == number)
            .unwrap()
            .id
            .clone()
    }

    fn pending_permission(&self) -> Option<PermissionRequest> {
        let review = self.store.get(&self.review_id)?;
        review
            .threads
            .iter()
            .flat_map(|thread| &thread.messages)
            .filter_map(|message| message.permission.clone())
            .find(|permission| permission.outcome.is_none())
    }

    async fn wait_for(&self, action: &Value) {
        for _ in 0..400 {
            let ready = if action["action"] == "choose" {
                self.pending_permission().is_some()
            } else {
                self.services
                    .presence
                    .get(&self.review_id)
                    .is_some_and(|presence| presence.listening)
            };
            if ready {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out waiting before {action}");
    }

    async fn act(&mut self, step: &Value) {
        let store = &self.store;
        let review_id = self.review_id.clone();
        let text = |key: &str| step[key].as_str().unwrap_or_default().to_string();
        match step["action"].as_str().unwrap() {
            "ask" => {
                let thread = self.thread_id(&text("thread"));
                store
                    .add_message(&review_id, &thread, text("body"), reviewer(), None)
                    .unwrap();
                store
                    .set_thread_intent(&review_id, &thread, ThreadIntent::Question)
                    .unwrap();
            }
            "comment" => {
                let thread = self.thread_id(&text("thread"));
                store
                    .add_message(&review_id, &thread, text("body"), reviewer(), None)
                    .unwrap();
            }
            "newThread" => {
                let location: CodeLocation = serde_json::from_value(
                    self.placeholders
                        .substitute_value(&step["location"], &review_id),
                )
                .unwrap();
                let options: ThreadOptions =
                    serde_json::from_value(step["options"].clone()).unwrap_or_default();
                store
                    .create_thread(&review_id, location, text("body"), reviewer(), options)
                    .unwrap();
            }
            "choose" => {
                let permission = self.pending_permission().unwrap();
                self.services
                    .decisions
                    .resolve(&permission.id, Some(text("option")));
                // The choice is written to the thread after the decision resolves.
                for _ in 0..400 {
                    if self.pending_permission().map(|pending| pending.id)
                        != Some(permission.id.clone())
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                panic!("the choice was not recorded");
            }
            "submit" => {
                let decision: ReviewDecision =
                    serde_json::from_value(step["decision"].clone()).unwrap();
                store
                    .submit(&review_id, decision, text("summary"), reviewer())
                    .unwrap();
            }
            "accept" => {
                let thread = self.thread_id(&text("thread"));
                store
                    .decide_proposal(&review_id, &thread, true, text("note"), None)
                    .unwrap();
            }
            "viewed" => {
                let paths: Vec<String> = serde_json::from_value(step["paths"].clone()).unwrap();
                store.set_viewed(&review_id, &paths, true).unwrap();
            }
            "close" => {
                let review = store.get(&review_id).unwrap();
                self.server.reviewer().window_closed(&window_root(&review));
            }
            other => panic!("unknown reviewer action {other}"),
        }
    }

    fn compare(&mut self, what: &str, expected: &Value, actual: &Value) {
        let expected = normalize(&reworded(expected), None);
        let actual = normalize(actual, Some(&tmp()));
        if expected != actual {
            self.mismatches
                .push(format!("{what}\n{}", difference(&expected, &actual)));
        }
    }
}

/// The lines that differ (`-` Theia, `+` review_mcp), with a little context.
fn difference(expected: &Value, actual: &Value) -> String {
    let expected = serde_json::to_string_pretty(expected).unwrap();
    let actual = serde_json::to_string_pretty(actual).unwrap();
    similar::TextDiff::from_lines(&expected, &actual)
        .unified_diff()
        .context_radius(2)
        .header("Theia", "review_mcp")
        .to_string()
}

fn window_root(review: &Review) -> String {
    match &review.bundle {
        Some(bundle) => {
            review_store::path_to_file_uri(bundle.code.as_deref().unwrap_or(&bundle.dir))
        }
        None => review.workspace_root.clone(),
    }
}

/// The one deliberate contract change (design decision 11: no browser app, `open_review` returns a `note` and never a
/// `url`): where the Theia app told agents to give the reviewer the URL, review_mcp tells them the review is open in
/// Co-Review. Applied to the recorded side only, so the old wording can't come back unnoticed.
const DECISION_11: [(&str, &str); 4] = [
    (
        "and give me the URL (if there is one).",
        "and tell me it's open in Co-Review.",
    ),
    (
        "tell the reviewer the URL.",
        "tell the reviewer the review is open in Co-Review.",
    ),
    (
        "returns the URL to give the reviewer.",
        "tell the reviewer it is open in Co-Review.",
    ),
    (
        "Share the URL with the reviewer,",
        "Tell the reviewer the review is open in Co-Review,",
    ),
];

fn reworded(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(
            DECISION_11
                .iter()
                .fold(text.clone(), |text, (theia, rust)| {
                    text.replace(theia, rust)
                }),
        ),
        Value::Array(items) => Value::Array(items.iter().map(reworded).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), reworded(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A tool result as comparable JSON: the parsed text when it is JSON, else the text.
fn result_value(text: &str, is_error: bool) -> Value {
    let content =
        serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string()));
    json!({ "content": content, "isError": is_error })
}

static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
});
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z").unwrap());
/// The enclosing symbol in a channel message (`#1 on src/a.ts:7-9 (createOrder)`): Tree-sitter, not ported yet.
static CHANNEL_SYMBOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#\d+ on [^\n(]*?) \([^)\n]*\)").unwrap());

/// Ids and times by order of appearance, scratch paths as `{tmp}`, and what review_syntax will add (symbols) left
/// out.
fn normalize(value: &Value, tmp: Option<&str>) -> Value {
    let mut ids = HashMap::new();
    normalize_with(value, tmp, &mut ids)
}

fn normalize_with(value: &Value, tmp: Option<&str>, ids: &mut HashMap<String, String>) -> Value {
    match value {
        Value::String(text) => {
            let mut text = text.clone();
            if let Some(tmp) = tmp {
                text = text.replace(tmp, "{tmp}");
            }
            text = UUID
                .replace_all(&text, |captures: &regex::Captures| {
                    let next = ids.len() + 1;
                    ids.entry(captures[0].to_string())
                        .or_insert_with(|| format!("<id-{next}>"))
                        .clone()
                })
                .into_owned();
            text = TIME.replace_all(&text, "<time>").into_owned();
            text = CHANNEL_SYMBOL.replace(&text, "$1").into_owned();
            Value::String(text)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| normalize_with(item, tmp, ids))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                // `symbol`: Tree-sitter (review_syntax) is not ported yet.
                .filter(|(key, _)| key.as_str() != "symbol")
                .map(|(key, value)| (key.clone(), normalize_with(value, tmp, ids)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.clone(),
    }
}

fn prepare(operations: &[Value], placeholders: &Placeholders) {
    for operation in operations {
        match operation["op"].as_str().unwrap() {
            "write" => {
                let path =
                    PathBuf::from(placeholders.substitute(operation["path"].as_str().unwrap(), ""));
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, operation["text"].as_str().unwrap()).unwrap();
            }
            "git" => {
                let arguments: Vec<String> = operation["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|argument| placeholders.substitute(argument.as_str().unwrap(), ""))
                    .collect();
                let status = std::process::Command::new("git")
                    .args(&arguments)
                    .current_dir(tmp())
                    .status()
                    .unwrap();
                assert!(status.success(), "git {arguments:?}");
            }
            other => panic!("unknown fixture operation {other}"),
        }
    }
}

async fn replay(name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/conformance")
        .join(format!("{name}.jsonl"));
    let lines: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let home = PathBuf::from(tmp()).join("home");
    let store = Arc::new(ReviewStore::with_home(home.clone()));
    let services = Services::headless(Arc::clone(&store));
    let server = review_mcp::start(
        ServerOptions {
            port: 0,
            default_root: None,
            registry: Some(home.join(format!("server-{name}.json"))),
        },
        services.clone(),
    )
    .unwrap();

    let fixture = &lines[0];
    let mut placeholders = Placeholders(vec![("tmp".to_string(), tmp())]);
    for (key, value) in fixture["placeholders"].as_object().unwrap() {
        let resolved = placeholders.substitute(value.as_str().unwrap(), "");
        placeholders.0.push((key.clone(), resolved));
    }
    prepare(fixture["operations"].as_array().unwrap(), &placeholders);

    let session = &lines[1];
    let recorder = Recorder {
        name: session["client"].as_str().unwrap().to_string(),
        notifications: Arc::default(),
        arrived: Arc::default(),
    };
    let root = placeholders.substitute("{repo}", "");
    let url = format!("{}/mcp?root={}", server.url(), root.replace('/', "%2F"));
    let client: RunningService<RoleClient, Recorder> = recorder
        .clone()
        .serve(StreamableHttpClientTransport::from_uri(url))
        .await
        .unwrap();

    let mut replay = Replay {
        server,
        store,
        services,
        placeholders,
        review_id: String::new(),
        mismatches: Vec::new(),
    };

    let info = client.peer_info().unwrap();
    let initialize = &lines[2];
    replay.compare(
        "instructions",
        &initialize["instructions"],
        &json!(info.instructions),
    );
    replay.compare(
        "capabilities",
        &initialize["capabilities"],
        &serde_json::to_value(&info.capabilities).unwrap(),
    );
    let tools: Vec<Value> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| json!({ "name": tool.name, "description": tool.description, "inputSchema": tool.input_schema }))
        .collect();
    let expected_tools: Vec<Value> = lines[3]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| json!({ "name": tool["name"], "description": tool["description"], "inputSchema": tool["inputSchema"] }))
        .collect();
    replay.compare("tools/list", &json!(expected_tools), &json!(tools));

    for (index, step) in lines.iter().enumerate().skip(4) {
        match step["kind"].as_str().unwrap() {
            "call" => {
                let tool = step["tool"].as_str().unwrap().to_string();
                let arguments = replay
                    .placeholders
                    .substitute_value(&step["arguments"], &replay.review_id);
                let peer = client.peer().clone();
                let call = tokio::spawn(async move {
                    peer.call_tool(
                        CallToolRequestParams::new(tool)
                            .with_arguments(arguments.as_object().cloned().unwrap_or_default()),
                    )
                    .await
                });
                for action in step["during"].as_array().into_iter().flatten() {
                    replay.wait_for(action).await;
                    replay.act(action).await;
                }
                let result = call.await.unwrap().unwrap();
                let text = result
                    .content
                    .first()
                    .and_then(|content| content.as_text())
                    .map(|text| text.text.clone())
                    .unwrap_or_default();
                let actual = result_value(&text, result.is_error.unwrap_or(false));
                if step["tool"] == "open_review"
                    && let Some(id) = actual["content"]["reviewId"].as_str()
                {
                    replay.review_id = id.to_string();
                }
                let recorded = &step["result"];
                let expected = result_value(
                    recorded["content"][0]["text"].as_str().unwrap_or_default(),
                    recorded["isError"].as_bool().unwrap_or(false),
                );
                replay.compare(
                    &format!(
                        "{name} step {index}: {} {}",
                        step["tool"], step["arguments"]
                    ),
                    &expected,
                    &actual,
                );
            }
            "reviewer" => replay.act(step).await,
            "notifications" => {
                let expected: Vec<Value> = step["notifications"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut actual = recorder.take(expected.len()).await;
                actual.sort_by_key(|notification| notification.to_string());
                let mut actual: Vec<Value> = actual
                    .iter()
                    .map(|value| normalize(value, Some(&tmp())))
                    .collect();
                let mut expected: Vec<Value> = expected
                    .iter()
                    .map(|value| normalize(&reworded(value), None))
                    .collect();
                // Ids are numbered per notification; order by the normalized text.
                actual.sort_by_key(|value| value.to_string());
                expected.sort_by_key(|value| value.to_string());
                if expected != actual {
                    replay.mismatches.push(format!(
                        "{name} step {index}: notifications\n{}",
                        difference(&json!(expected), &json!(actual))
                    ));
                }
            }
            other => panic!("unknown step {other}"),
        }
    }
    client.cancel().await.ok();
    replay.server.stop();
    assert!(
        replay.mismatches.is_empty(),
        "{} of the recorded results differ:\n\n{}",
        replay.mismatches.len(),
        replay.mismatches.join("\n\n")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn audit() {
    replay("audit").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn change() {
    replay("change").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_request() {
    replay("pr").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn design() {
    replay("design").await;
}
