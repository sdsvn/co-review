//! The agent service against a scripted ACP agent (`tests/support/fake_agent.rs`) over stdio.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use review_agents::{AgentEvent, AgentService, ConnectionState, answers};
use review_mcp::HumanDecisions;
use review_model::{
    AgentActivity, AgentConfig, AgentState, AgentTransport, CodeAnchor,
    CodeLocation, LocationKind, MessageStatus, Participant, ParticipantKind,
    Position, Range, ReviewMessage, ReviewScope, ReviewThread, ThreadIntent,
    ThreadOptions, ToolStatus,
};
use review_store::{CreateReviewParams, ReviewStore, path_to_file_uri};
use serde_json::Value;

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Calculator;\n\nimpl Calculator {\n    pub fn total(&self) -> i32 {\n        add(1, 2)\n    }\n}\n";

struct Fixture {
    _home: tempfile::TempDir,
    _repository: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
    store: Arc<ReviewStore>,
    decisions: Arc<HumanDecisions>,
    service: Arc<AgentService>,
    events: Receiver<AgentEvent>,
    review_id: String,
}

fn reviewer() -> Participant {
    Participant {
        id: "human:reviewer@example.com".into(),
        kind: ParticipantKind::Human,
        name: "Reviewer".into(),
    }
}

fn fake_agent(log: &Path) -> AgentConfig {
    AgentConfig {
        id: "fake".into(),
        name: "Fake Agent".into(),
        transport: Some(AgentTransport::Acp),
        command: Some(env!("CARGO_BIN_EXE_fake-acp-agent").into()),
        args: Some(vec![log.to_string_lossy().into_owned()]),
        settings: None,
        settings_label: None,
        extra: serde_json::Map::new(),
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}");
}

fn fixture() -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let repository = tempfile::tempdir().unwrap();
    // The repository is a folder of its own temporary directory, so files "outside" it are private to the test.
    let root = std::fs::canonicalize(repository.path())
        .unwrap()
        .join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("README.md"), "# Calculator\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    let log = home.path().join("agent.jsonl");
    let store = Arc::new(ReviewStore::with_home(home.path().join("co-review")));
    let decisions = Arc::new(HumanDecisions::default());
    let service = AgentService::new(Arc::clone(&store), Arc::clone(&decisions));
    let events = service.subscribe();
    let review = store
        .create(CreateReviewParams {
            workspace_root: path_to_file_uri(&root.to_string_lossy()),
            title: "Calculator".into(),
            scope: ReviewScope::Repository,
            author: reviewer(),
        })
        .unwrap();
    Fixture {
        _home: home,
        _repository: repository,
        root,
        log,
        store,
        decisions,
        service,
        events,
        review_id: review.id,
    }
}

impl Fixture {
    fn connect(&self) {
        self.service
            .connect(&self.review_id, fake_agent(&self.log))
            .unwrap();
    }

    /// A question on lines 1-3 of src/lib.rs.
    fn question(&self, body: &str) -> String {
        self.ask_about(body, false)
    }

    /// A question on lines 1-3 of src/lib.rs, or on the whole repository.
    fn ask_about(&self, body: &str, repository: bool) -> String {
        let mut location = CodeLocation {
            kind: LocationKind::Range,
            uri: Some(path_to_file_uri(
                &self.root.join("src/lib.rs").to_string_lossy(),
            )),
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 2,
                    character: 1,
                },
            }),
            symbol: Some("add".into()),
            anchor: Some(CodeAnchor {
                text: "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}".into(),
                before: None,
                after: None,
                syntax: None,
            }),
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: serde_json::Map::new(),
        };
        if repository {
            location = CodeLocation {
                kind: LocationKind::Repository,
                uri: None,
                range: None,
                symbol: None,
                anchor: None,
                ..location
            };
        }
        let options = ThreadOptions {
            intent: Some(ThreadIntent::Question),
            ..ThreadOptions::default()
        };
        let (_, thread) = self
            .store
            .create_thread(
                &self.review_id,
                location,
                body.into(),
                reviewer(),
                options,
            )
            .unwrap();
        thread.id
    }

    fn reply(&self, thread_id: &str, body: &str) {
        self.store
            .add_message(&self.review_id, thread_id, body.into(), reviewer(), None)
            .unwrap();
    }

    fn thread(&self, thread_id: &str) -> ReviewThread {
        let review = self.store.get(&self.review_id).unwrap();
        review
            .threads
            .into_iter()
            .find(|thread| thread.id == thread_id)
            .unwrap()
    }

    /// Waits for a thread event matching `until`; returns every thread state seen on the way.
    fn wait(
        &self,
        thread_id: &str,
        until: impl Fn(&ReviewThread) -> bool,
    ) -> Vec<ReviewThread> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(remaining) {
                Ok(AgentEvent::Thread { thread, .. }) if thread.id == thread_id => {
                    let done = until(&thread);
                    seen.push(thread);
                    if done {
                        return seen;
                    }
                }
                Ok(_) => {}
                Err(_) => panic!(
                    "timed out; thread is {:#?}",
                    self.thread(thread_id).messages.last()
                ),
            }
        }
    }

    /// Asks and waits until the turn has finished (its message is no longer streaming and the agent is idle).
    fn ask(&self, thread_id: &str) -> ReviewMessage {
        let answers = self.answers(thread_id);
        self.service.ask(&self.review_id, thread_id);
        self.wait(thread_id, |thread| {
            agent_messages(thread).len() > answers
                && thread.agent_state == Some(AgentState::Idle)
        });
        agent_messages(&self.thread(thread_id))
            .last()
            .cloned()
            .unwrap()
    }

    fn answers(&self, thread_id: &str) -> usize {
        agent_messages(&self.thread(thread_id)).len()
    }

    /// What the agent received, as JSON-RPC messages.
    fn received(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            // The agent may be writing the last line.
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn requests(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|message| message["method"] == method)
            .collect()
    }
}

fn agent_messages(thread: &ReviewThread) -> Vec<ReviewMessage> {
    thread
        .messages
        .iter()
        .filter(|message| message.author.kind == ParticipantKind::Agent)
        .cloned()
        .collect()
}

fn activity(
    id: &str,
    title: &str,
    kind: Option<&str>,
    status: ToolStatus,
) -> AgentActivity {
    AgentActivity {
        id: id.into(),
        title: title.into(),
        kind: kind.map(str::to_string),
        status,
    }
}

#[test]
fn answers_stream_into_the_thread_with_steps_and_tool_activity() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("What does add do?");
    fixture.service.ask(&fixture.review_id, &thread_id);
    let seen = fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::Idle)
    });
    assert!(
        seen.iter()
            .any(|thread| thread.agent_state == Some(AgentState::Working)),
        "the thread shows the agent working"
    );
    assert!(
        seen.iter().any(|thread| {
            agent_messages(thread).last().is_some_and(|message| {
                message.status == Some(MessageStatus::Streaming)
            })
        }),
        "the answer is shown while it streams"
    );
    let answer = agent_messages(&fixture.thread(&thread_id)).pop().unwrap();
    assert_eq!(answer.author.id, "agent:fake");
    assert_eq!(answer.author.name, "Fake Agent");
    assert_eq!(answer.status, Some(MessageStatus::Done));
    assert_eq!(answer.body, "It adds two numbers.");
    assert_eq!(
        answer.activity.unwrap(),
        vec![
            activity(
                "read-1",
                "Read src/lib.rs",
                Some("read"),
                ToolStatus::Completed
            ),
            activity("search-1", "Search for add", None, ToolStatus::InProgress),
            // The first message becomes a step when the second starts.
            activity(
                "message:m1",
                "Let me look at the code.",
                Some("think"),
                ToolStatus::Completed
            ),
        ]
    );

    // What the client told the agent.
    let initialize = &fixture.requests("initialize")[0]["params"];
    assert_eq!(initialize["protocolVersion"], 1);
    assert_eq!(
        initialize["clientCapabilities"]["fs"],
        serde_json::json!({ "readTextFile": true, "writeTextFile": true })
    );
    assert_ne!(initialize["clientCapabilities"]["terminal"], true);
    assert_eq!(initialize["clientInfo"]["name"], "co-review");
    let sessions = fixture.requests("session/new");
    assert!(!sessions.is_empty());
    for session in &sessions {
        assert_eq!(
            session["params"]["cwd"],
            fixture.root.to_string_lossy().as_ref()
        );
        assert_eq!(session["params"]["mcpServers"], serde_json::json!([]));
    }

    // The first prompt: context, map, code, link, conversation.
    let prompt = &fixture.requests("session/prompt")[0]["params"];
    assert_eq!(
        prompt["sessionId"], "session-1",
        "the session opened ahead of time"
    );
    let blocks = prompt["prompt"].as_array().unwrap();
    let texts: Vec<&str> = blocks
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect();
    assert!(texts[0].starts_with(&format!(
        "You are a participant in a code review of the repository at {} (\"Calculator\").\nA reviewer asked you about src/lib.rs:1-3 (add) in review thread #1.",
        fixture.root.display()
    )));
    assert!(texts[0].ends_with(review_mcp::ANSWER_STYLE));
    assert!(
        texts[1].contains("src/lib.rs") && texts[1].contains("Calculator"),
        "the repository map: {}",
        texts[1]
    );
    assert_eq!(
        texts[2],
        "The code in question (lines 1-3):\n```\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n```"
    );
    let link = blocks
        .iter()
        .find(|block| block["type"] == "resource_link")
        .unwrap();
    assert_eq!(link["name"], "src/lib.rs");
    assert_eq!(
        link["uri"],
        path_to_file_uri(&fixture.root.join("src/lib.rs").to_string_lossy())
    );
    assert_eq!(
        *texts.last().unwrap(),
        "Conversation:\n\nReviewer (Reviewer): What does add do?"
    );

    // A follow-up continues the same session with only the new messages.
    fixture.reply(&thread_id, "Thanks.");
    fixture.reply(&thread_id, "And what are the settings?");
    assert!(answers(
        &fixture.store.get(&fixture.review_id).unwrap(),
        &fixture.thread(&thread_id)
    ));
    let follow_up = fixture.ask(&thread_id);
    assert_eq!(follow_up.body, "model=sonnet effort=low");
    let prompts = fixture.requests("session/prompt");
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1]["params"]["sessionId"], "session-1");
    assert_eq!(
        prompts[1]["params"]["prompt"],
        serde_json::json!([{ "type": "text", "text": "Thanks.\n\nAnd what are the settings?" }])
    );

    // A second thread takes the next spare session.
    let other = fixture.question("Another question");
    fixture.ask(&other);
    assert_eq!(
        fixture.requests("session/prompt")[2]["params"]["sessionId"],
        "session-2"
    );
    fixture.service.shutdown();
}

#[test]
fn model_and_effort_apply_to_every_session() {
    let fixture = fixture();
    fixture.connect();
    let settings = fixture.service.list_options(&fixture.review_id).unwrap();
    let summary: Vec<(String, String, String, Vec<String>)> = settings
        .iter()
        .map(|setting| {
            (
                setting.id.clone(),
                setting.category.clone(),
                setting.current.clone(),
                setting
                    .options
                    .iter()
                    .map(|option| option.name.clone())
                    .collect(),
            )
        })
        .collect();
    // Modes and boolean options are not offered; grouped values are flattened.
    assert_eq!(
        summary,
        vec![
            (
                "model".into(),
                "model".into(),
                "sonnet".into(),
                vec!["Sonnet".into(), "Opus".into()]
            ),
            (
                "effort".into(),
                "thought_level".into(),
                "low".into(),
                vec!["Low".into(), "High".into()]
            ),
        ]
    );
    assert_eq!(settings[0].options[0].description.as_deref(), Some("Fast"));

    // A session open before the choice gets it too.
    let early = fixture.question("settings?");
    assert_eq!(fixture.ask(&early).body, "model=sonnet effort=low");

    fixture
        .service
        .set_option(&fixture.review_id, "model", "opus")
        .unwrap();
    let review = fixture
        .service
        .set_option(&fixture.review_id, "effort", "high")
        .unwrap();
    let agent = review.agent.unwrap();
    assert_eq!(agent.settings_label.as_deref(), Some("Opus · High"));
    assert_eq!(
        agent.settings.unwrap().get("model").map(String::as_str),
        Some("opus")
    );
    assert_eq!(
        fixture.service.list_options(&fixture.review_id).unwrap()[0].current,
        "opus",
        "the review's choice is current"
    );

    let thread_id = fixture.question("What are the settings?");
    assert_eq!(fixture.ask(&thread_id).body, "model=opus effort=high");
    fixture.reply(&early, "settings again?");
    assert_eq!(fixture.ask(&early).body, "model=opus effort=high");

    // Reconnecting the same agent keeps the choice.
    fixture.service.disconnect(&fixture.review_id);
    fixture.connect();
    let later = fixture.question("settings now?");
    assert_eq!(fixture.ask(&later).body, "model=opus effort=high");
    fixture.service.shutdown();
}

#[test]
fn permission_requests_wait_for_the_reviewer() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("Fix it, permission needed");
    fixture.service.ask(&fixture.review_id, &thread_id);
    let seen = fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::WaitingForHuman)
    });
    let request = agent_messages(seen.last().unwrap())
        .pop()
        .unwrap()
        .permission
        .unwrap();
    assert_eq!(request.title, "Edit src/lib.rs");
    assert_eq!(request.outcome, None);
    let options: Vec<(String, String, String)> = request
        .options
        .iter()
        .map(|option| (option.id.clone(), option.name.clone(), option.kind.clone()))
        .collect();
    assert_eq!(
        options,
        vec![
            ("allow".into(), "Allow".into(), "allow_once".into()),
            ("reject".into(), "Reject".into(), "reject_once".into()),
        ]
    );
    fixture.decisions.resolve(&request.id, Some("allow".into()));
    fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::Idle)
    });
    let answer = agent_messages(&fixture.thread(&thread_id)).pop().unwrap();
    assert_eq!(answer.body, "You chose allow.");
    assert_eq!(answer.status, Some(MessageStatus::Done));
    assert_eq!(answer.permission.unwrap().outcome.as_deref(), Some("allow"));

    // Stop while the agent waits for a decision cancels the request and the turn.
    fixture.reply(&thread_id, "Again, permission needed");
    fixture.service.ask(&fixture.review_id, &thread_id);
    fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::WaitingForHuman)
    });
    fixture
        .service
        .stop(&fixture.review_id, &thread_id)
        .unwrap();
    fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::Idle)
    });
    let answer = agent_messages(&fixture.thread(&thread_id)).pop().unwrap();
    assert_eq!(answer.status, Some(MessageStatus::Cancelled));
    assert_eq!(
        answer.permission.unwrap().outcome.as_deref(),
        Some("cancelled")
    );
    assert_eq!(answer.body, "The permission request was cancelled.");
    fixture.service.shutdown();
}

#[test]
fn stop_cancels_the_turn() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("Take it slow");
    fixture.service.ask(&fixture.review_id, &thread_id);
    fixture.wait(&thread_id, |thread| {
        agent_messages(thread)
            .last()
            .is_some_and(|message| message.body == "Working on it")
    });
    fixture
        .service
        .stop(&fixture.review_id, &thread_id)
        .unwrap();
    fixture.wait(&thread_id, |thread| {
        thread.agent_state == Some(AgentState::Idle)
    });
    let answer = agent_messages(&fixture.thread(&thread_id)).pop().unwrap();
    assert_eq!(answer.status, Some(MessageStatus::Cancelled));
    assert_eq!(answer.body, "Working on it");
    let cancel = &fixture.requests("session/cancel")[0]["params"];
    assert_eq!(cancel["sessionId"], "session-1");

    // The session goes on afterwards.
    fixture.reply(&thread_id, "What are the settings?");
    assert_eq!(fixture.ask(&thread_id).body, "model=sonnet effort=low");
    fixture.service.shutdown();
}

#[test]
fn file_access_is_limited_to_the_repository() {
    let fixture = fixture();
    fixture.connect();
    let outside = fixture.root.parent().unwrap().join(format!(
        "{}-outside.txt",
        fixture.root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(&outside, "one\nsecret\n").unwrap();
    let thread_id = fixture.question("read:src/lib.rs");
    assert_eq!(fixture.ask(&thread_id).body, "Line 2 is:     a + b");

    let reads = [
        format!("read:../{}", outside.file_name().unwrap().to_string_lossy()),
        format!("read:{}", outside.display()),
    ];
    for read in reads {
        fixture.reply(&thread_id, &read);
        let body = fixture.ask(&thread_id).body;
        assert!(
            body.starts_with(
                "Read failed: Access outside the repository is not allowed:"
            ),
            "{body}"
        );
    }

    fixture.reply(&thread_id, "write:notes/../written.txt");
    assert_eq!(fixture.ask(&thread_id).body, "Written.");
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("written.txt")).unwrap(),
        "written by agent\n"
    );
    let target = fixture.root.parent().unwrap().join(format!(
        "{}-escaped.txt",
        fixture.root.file_name().unwrap().to_string_lossy()
    ));
    fixture.reply(&thread_id, &format!("write:{}", target.display()));
    assert!(
        fixture
            .ask(&thread_id)
            .body
            .starts_with("Write failed: Access outside the repository")
    );
    assert!(!target.exists());
    std::fs::remove_file(outside).ok();
    fixture.service.shutdown();
}

#[test]
fn a_crashed_agent_is_reported_and_restarted() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("Please crash");
    let answer = fixture.ask(&thread_id);
    assert_eq!(answer.status, Some(MessageStatus::Error));
    assert!(answer.body.starts_with("About to fail\n\n**The agent process exited (code 3).**\n\n**Agent error:** "), "{}", answer.body);
    assert!(
        answer
            .body
            .ends_with("```\nfake agent: crashing on purpose\n```"),
        "{}",
        answer.body
    );

    // The next question starts the agent again, in a new session.
    fixture.reply(&thread_id, "What are the settings?");
    let answer = fixture.ask(&thread_id);
    assert_eq!(answer.status, Some(MessageStatus::Done));
    assert_eq!(answer.body, "model=sonnet effort=low");
    assert_eq!(fixture.requests("initialize").len(), 2);
    fixture.service.shutdown();
}

#[test]
fn refusals_and_empty_answers_say_so() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("Please refuse");
    let answer = fixture.ask(&thread_id);
    assert_eq!(answer.body, "_The agent declined to answer._");
    assert_eq!(answer.status, Some(MessageStatus::Done));
    fixture.service.shutdown();
}

#[test]
fn an_agent_that_cannot_start_is_an_error_in_the_thread() {
    let fixture = fixture();
    let mut agent = fake_agent(&fixture.log);
    agent.command = Some("/nonexistent/co-review-agent".into());
    agent.name = "Missing".into();
    fixture.service.connect(&fixture.review_id, agent).unwrap();
    let thread_id = fixture.question("Anyone there?");
    let answer = fixture.ask(&thread_id);
    assert_eq!(answer.status, Some(MessageStatus::Error));
    assert!(
        answer.body.starts_with("**Agent error:** Could not start Missing (`/nonexistent/co-review-agent "),
        "{}",
        answer.body
    );
}

#[test]
fn turns_in_a_thread_run_one_after_another() {
    let fixture = fixture();
    fixture.connect();
    let thread_id = fixture.question("First question");
    fixture.service.ask(&fixture.review_id, &thread_id);
    fixture.service.ask(&fixture.review_id, &thread_id);
    fixture.wait(&thread_id, |thread| {
        agent_messages(thread).len() == 2
            && thread.agent_state == Some(AgentState::Idle)
    });
    let thread = fixture.thread(&thread_id);
    let messages = agent_messages(&thread);
    assert!(
        messages
            .iter()
            .all(|message| message.status == Some(MessageStatus::Done))
    );
    // The second turn starts after the first answered: it sees its answer, so it repeats the latest question.
    let prompts = fixture.requests("session/prompt");
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[1]["params"]["prompt"],
        serde_json::json!([{ "type": "text", "text": "First question" }])
    );
    fixture.service.shutdown();
}

#[test]
fn the_connection_reports_its_state() {
    let fixture = fixture();
    fixture.connect();
    let mut states = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !states.contains(&ConnectionState::Ready) {
        match fixture
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(AgentEvent::Connection { state, review_id }) => {
                assert_eq!(review_id, fixture.review_id);
                states.push(state);
            }
            Ok(_) => {}
            Err(_) => panic!("no Ready: {states:?}"),
        }
    }
    assert_eq!(
        states,
        vec![ConnectionState::Starting, ConnectionState::Ready]
    );
    fixture.service.disconnect(&fixture.review_id);
    loop {
        match fixture.events.recv_timeout(Duration::from_secs(10)) {
            Ok(AgentEvent::Connection {
                state: ConnectionState::Exited { code },
                ..
            }) => {
                assert_eq!(
                    code,
                    Some(0),
                    "the agent exits by itself when its stdin closes"
                );
                break;
            }
            Ok(_) => {}
            Err(_) => panic!("no Exited"),
        }
    }
}

/// Replays `tests/fixtures/acp_turns.json`, recorded from the Theia app: the same script
/// against the same agent gives the same thread messages and sends the agent the same prompts.
#[test]
fn replays_the_typescript_recording() {
    let recording: Value =
        serde_json::from_str(include_str!("fixtures/acp_turns.json")).unwrap();
    let fixture = fixture();
    let outside = fixture.root.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "one\nsecret\n").unwrap();
    let scrub = |value: Value| -> Value {
        let text = value
            .to_string()
            .replace(&outside.to_string_lossy().into_owned(), "{outside}")
            .replace(&fixture.root.to_string_lossy().into_owned(), "{repo}");
        serde_json::from_str(&text).unwrap()
    };
    let mut threads = std::collections::HashMap::new();
    let mut results = Vec::new();
    for step in recording["steps"].as_array().unwrap() {
        match step["op"].as_str().unwrap() {
            "connect" => fixture.connect(),
            "settings" => {
                for (id, value) in step["settings"].as_object().unwrap() {
                    fixture
                        .service
                        .set_option(&fixture.review_id, id, value.as_str().unwrap())
                        .unwrap();
                }
            }
            "options" => {
                let settings =
                    fixture.service.list_options(&fixture.review_id).unwrap();
                results.push(
                    serde_json::json!({ "name": "options", "settings": settings }),
                );
            }
            op => {
                let before = fixture.received().len();
                let body = step["body"]
                    .as_str()
                    .unwrap()
                    .replace("{outside}", &outside.to_string_lossy());
                let name = step["name"].as_str().unwrap();
                let thread_id = if op == "question" {
                    let thread_id =
                        fixture.ask_about(&body, step["repository"] == true);
                    threads.insert(name.to_string(), thread_id.clone());
                    thread_id
                } else {
                    let thread_id =
                        threads[step["thread"].as_str().unwrap()].clone();
                    fixture.reply(&thread_id, &body);
                    thread_id
                };
                let answered = fixture.answers(&thread_id);
                fixture.service.ask(&fixture.review_id, &thread_id);
                if let Some(decision) = step.get("decide") {
                    let seen = fixture.wait(&thread_id, |thread| {
                        thread.agent_state == Some(AgentState::WaitingForHuman)
                    });
                    let request = agent_messages(seen.last().unwrap())
                        .pop()
                        .unwrap()
                        .permission
                        .unwrap();
                    match decision.as_str() {
                        Some(option) => fixture
                            .decisions
                            .resolve(&request.id, Some(option.into())),
                        None => fixture
                            .service
                            .stop(&fixture.review_id, &thread_id)
                            .unwrap(),
                    }
                }
                if let Some(text) = step["stopAfter"].as_str() {
                    fixture.wait(&thread_id, |thread| {
                        agent_messages(thread)
                            .last()
                            .is_some_and(|message| message.body == text)
                    });
                    fixture
                        .service
                        .stop(&fixture.review_id, &thread_id)
                        .unwrap();
                }
                fixture.wait(&thread_id, |thread| {
                    agent_messages(thread).len() > answered
                        && thread.agent_state == Some(AgentState::Idle)
                });
                std::thread::sleep(Duration::from_millis(100));
                let mut message = serde_json::to_value(
                    agent_messages(&fixture.thread(&thread_id)).pop().unwrap(),
                )
                .unwrap();
                if let Some(permission) = message.get_mut("permission") {
                    permission["id"] = "{id}".into();
                }
                let prompts: Vec<Value> = fixture.received()[before..]
                    .iter()
                    .filter(|received| received["method"] == "session/prompt")
                    .map(|received| received["params"].clone())
                    .collect();
                results.push(scrub(serde_json::json!({
                    "name": name,
                    "message": {
                        "body": message["body"],
                        "status": message["status"],
                        "activity": message["activity"],
                        "permission": message.get("permission"),
                    },
                    "prompts": prompts,
                })));
            }
        }
    }
    fixture.service.shutdown();

    for (expected, actual) in recording["results"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&results)
    {
        let mut expected = expected.clone();
        let name = expected["name"].as_str().unwrap().to_string();
        match name.as_str() {
            // The TypeScript SDK hides a client error behind "Internal error"; the agent is told why here.
            "read-outside" | "write-outside" => {
                let verb = if name == "read-outside" {
                    "Read"
                } else {
                    "Write"
                };
                let body = actual["message"]["body"].as_str().unwrap();
                assert!(
                    body.starts_with(&format!("{verb} failed: Access outside the repository is not allowed: ")),
                    "{body}"
                );
                expected["message"]["body"] = body.into();
            }
            // Modes are not offered (the spec; the Theia app still listed them), and `current` is the review's
            // choice where there is one (the Theia UI showed `settings[id] ?? current`, the same thing).
            "options" => {
                let settings = expected["settings"].as_array_mut().unwrap();
                settings.retain(|setting| {
                    setting["category"] == "model"
                        || setting["category"] == "thought_level"
                });
                let chosen = &recording["steps"][3]["settings"];
                for setting in settings {
                    let id = setting["id"].as_str().unwrap().to_string();
                    setting["current"] = chosen[&id].clone();
                }
            }
            _ => {}
        }
        if expected["message"]["permission"].is_null()
            && let Some(message) = expected.get_mut("message")
        {
            message["permission"] = Value::Null;
        }
        assert_eq!(&expected, actual, "step {name}");
    }
    assert_eq!(
        results.len(),
        recording["results"].as_array().unwrap().len()
    );

    let received = scrub(Value::Array(fixture.received()));
    let of = |method: &str| -> Vec<Value> {
        received
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["method"] == method)
            .map(|message| message["params"].clone())
            .collect()
    };
    let initialize = &of("initialize")[0];
    assert_eq!(
        initialize["protocolVersion"],
        recording["initialize"]["protocolVersion"]
    );
    assert_eq!(
        initialize["clientInfo"],
        recording["initialize"]["clientInfo"]
    );
    assert_eq!(
        initialize["clientCapabilities"]["fs"],
        recording["initialize"]["clientCapabilities"]["fs"]
    );
    assert_ne!(initialize["clientCapabilities"]["terminal"], true);
    // Sessions opened ahead of time: as many as the Theia app, in the repository, without MCP servers.
    assert_eq!(Value::Array(of("session/new")), recording["sessionNew"]);
    // Settings are applied per session; their order within a session doesn't matter.
    let sorted = |mut values: Vec<Value>| {
        values.sort_by_key(|value| value.to_string());
        values
    };
    assert_eq!(
        sorted(of("session/set_config_option")),
        sorted(recording["setConfigOption"].as_array().unwrap().clone())
    );
    assert_eq!(Value::Array(of("session/cancel")), recording["cancel"]);
    std::fs::remove_file(outside).ok();
}

#[test]
fn watching_the_store_starts_and_stops_agents() {
    let fixture = fixture();
    fixture.service.watch_store().unwrap();
    let next_state = || loop {
        match fixture.events.recv_timeout(Duration::from_secs(20)) {
            Ok(AgentEvent::Connection { state, .. }) => return state,
            Ok(_) => {}
            Err(_) => panic!("no connection event"),
        }
    };
    // An agent set on the review starts ahead of the first question.
    fixture
        .store
        .set_agent(&fixture.review_id, Some(fake_agent(&fixture.log)))
        .unwrap();
    assert_eq!(next_state(), ConnectionState::Starting);
    assert_eq!(next_state(), ConnectionState::Ready);
    // Deleting the review stops it.
    fixture.store.delete(&fixture.review_id).unwrap();
    assert_eq!(next_state(), ConnectionState::Exited { code: Some(0) });
}
