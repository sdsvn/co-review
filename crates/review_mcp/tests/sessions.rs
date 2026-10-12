//! Delivery across sessions, questions answered after their call returned, and the endpoint's local-only rule:
//! behaviour the recorded workflows don't reach.

#![allow(clippy::unwrap_used)]

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use review_mcp::{CoReviewer, ServerOptions, Services, Session, ToolOutput};
use review_model::{AgentState, Participant, ParticipantKind, ThreadIntent};
use review_store::ReviewStore;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

fn repository(dir: &Path) -> String {
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(
        repo.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    let git = |arguments: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(arguments)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "Reviewer"]);
    git(&["config", "user.email", "reviewer@example.com"]);
    review_store::real_path(&repo)
        .to_string_lossy()
        .into_owned()
}

fn reviewer() -> Participant {
    Participant {
        id: "human:reviewer@example.com".to_string(),
        kind: ParticipantKind::Human,
        name: "Reviewer".to_string(),
    }
}

async fn call(session: &Arc<Session>, tool: &str, arguments: Value) -> Value {
    let arguments: Map<String, Value> = arguments.as_object().cloned().unwrap();
    let ToolOutput { text, is_error } =
        review_mcp::call(session, tool, arguments, CancellationToken::new()).await;
    assert!(!is_error, "{tool}: {text}");
    serde_json::from_str(&text).unwrap()
}

fn session(reviewer: &Arc<CoReviewer>, repo: &str) -> Arc<Session> {
    let session = Arc::new(Session::new(Arc::clone(reviewer)));
    session.initialized("codex", Some(repo));
    session
}

#[tokio::test(flavor = "multi_thread")]
async fn a_question_is_delivered_once_per_session_and_given_back_when_a_session_ends() {
    let scratch = tempfile::tempdir().unwrap();
    let repo = repository(scratch.path());
    let store = Arc::new(ReviewStore::with_home(scratch.path().join("home")));
    let coreviewer = Arc::new(CoReviewer::new(
        Services::headless(Arc::clone(&store)),
        None,
    ));

    let first = session(&coreviewer, &repo);
    let opened = call(&first, "open_review", json!({ "open": false })).await;
    let review_id = opened["reviewId"].as_str().unwrap().to_string();
    call(
        &first,
        "add_findings",
        json!({ "findings": [{ "path": "src/lib.rs", "line": 2, "body": "Overflow?" }] }),
    )
    .await;
    let thread = store.get(&review_id).unwrap().threads[0].id.clone();
    store
        .add_message(
            &review_id,
            &thread,
            "Can it overflow?".to_string(),
            reviewer(),
            None,
        )
        .unwrap();
    store
        .set_thread_intent(&review_id, &thread, ThreadIntent::Question)
        .unwrap();

    let handed = call(&first, "await_reviewer", json!({ "timeoutSec": 1 })).await;
    assert_eq!(handed["status"], "comment");
    assert_eq!(handed["threads"][0]["id"], "t1");
    assert_eq!(
        store.get(&review_id).unwrap().threads[0].agent_state,
        Some(AgentState::Working)
    );
    // Once per session: the same question doesn't come back to it.
    assert_eq!(
        call(&first, "await_reviewer", json!({ "timeoutSec": 1 })).await["status"],
        "pending"
    );

    // Another session on the review gets it too: the first may be gone.
    let second = session(&coreviewer, &repo);
    call(
        &second,
        "open_review",
        json!({ "reviewId": review_id, "open": false }),
    )
    .await;
    assert_eq!(
        call(&second, "await_reviewer", json!({ "timeoutSec": 1 })).await["status"],
        "comment"
    );

    // A session that ends without answering gives its threads back to the next agent.
    drop(first);
    drop(second);
    assert_eq!(
        store.get(&review_id).unwrap().threads[0].agent_state,
        Some(AgentState::Queued)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_choice_made_after_ask_reviewer_returned_comes_from_await_reviewer() {
    let scratch = tempfile::tempdir().unwrap();
    let repo = repository(scratch.path());
    let store = Arc::new(ReviewStore::with_home(scratch.path().join("home")));
    let services = Services::headless(Arc::clone(&store));
    let decisions = Arc::clone(&services.decisions);
    let coreviewer = Arc::new(CoReviewer::new(services, None));
    let agent = session(&coreviewer, &repo);
    let opened = call(&agent, "open_review", json!({ "open": false })).await;
    let review_id = opened["reviewId"].as_str().unwrap().to_string();

    let asked = call(
        &agent,
        "ask_reviewer",
        json!({ "question": "Ship it?", "options": ["Yes", "No"], "timeoutSec": 1 }),
    )
    .await;
    assert_eq!(asked["status"], "pending");
    assert_eq!(
        asked["question"],
        json!({ "threadId": "t1", "status": "pending" })
    );

    let permission = store.get(&review_id).unwrap().threads[0].messages[1]
        .permission
        .clone()
        .unwrap();
    decisions.resolve(&permission.id, Some("1".to_string()));
    let answered = call(&agent, "await_reviewer", json!({ "timeoutSec": 5 })).await;
    assert_eq!(answered["status"], "comment");
    let messages = &answered["threads"][0]["messages"];
    assert_eq!(
        messages[messages.as_array().unwrap().len() - 1]["body"],
        "Chose: **No**"
    );
    let thread = &store.get(&review_id).unwrap().threads[0];
    assert_eq!(thread.agent_state, Some(AgentState::Idle));
    assert_eq!(
        thread.messages[1]
            .permission
            .as_ref()
            .unwrap()
            .outcome
            .as_deref(),
        Some("1")
    );
}

fn request(url: &str, headers: &str) -> String {
    let address = url.trim_start_matches("http://");
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\n{headers}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok();
    String::from_utf8_lossy(&response).into_owned()
}

#[test]
fn only_local_clients_are_served_and_the_instance_is_recorded() {
    let scratch = tempfile::tempdir().unwrap();
    let registry = scratch.path().join("server.json");
    let store = Arc::new(ReviewStore::with_home(scratch.path().join("home")));
    let server = review_mcp::start(
        ServerOptions {
            port: 0,
            default_root: None,
            registry: Some(registry.clone()),
        },
        Services::headless(store),
    )
    .unwrap();
    let url = server.url().to_string();
    let record: Value = serde_json::from_str(&std::fs::read_to_string(&registry).unwrap()).unwrap();
    assert_eq!(record, json!({ "url": url, "pid": std::process::id() }));

    let port = url.rsplit(':').next().unwrap();
    let refused = request(
        &url,
        &format!("Host: 127.0.0.1:{port}\r\nOrigin: https://example.com\r\n"),
    );
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    assert!(
        refused.ends_with("Co-Review MCP only accepts local connections"),
        "{refused}"
    );
    let rebound = request(&url, "Host: attacker.example\r\n");
    assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
    let local = request(
        &url,
        &format!("Host: localhost:{port}\r\nOrigin: http://localhost:3000\r\n"),
    );
    assert!(local.starts_with("HTTP/1.1 200"), "{local}");
    assert!(local.contains("mcp-session-id"), "{local}");
    // serverInfo carries the app's (workspace) version, not a crate's own.
    let workspace_manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .unwrap();
    let app_version = workspace_manifest
        .lines()
        .skip_while(|line| line.trim() != "[workspace.package]")
        .find_map(|line| line.trim().strip_prefix("version"))
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap();
    assert!(
        local.contains(&format!(r#""serverInfo":{{"name":"co-review","version":"{app_version}"}}"#)),
        "{local}"
    );

    server.stop();
    assert!(!registry.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_mcp_session_gives_its_threads_back() {
    use rmcp::ServiceExt as _;

    let scratch = tempfile::tempdir().unwrap();
    let repo = repository(scratch.path());
    let store = Arc::new(ReviewStore::with_home(scratch.path().join("home")));
    let server = review_mcp::start(
        ServerOptions {
            port: 0,
            default_root: None,
            registry: None,
        },
        Services::headless(Arc::clone(&store)),
    )
    .unwrap();
    let url = format!("{}/mcp?root={}", server.url(), repo.replace('/', "%2F"));
    let client = ()
        .serve(rmcp::transport::StreamableHttpClientTransport::from_uri(
            url,
        ))
        .await
        .unwrap();
    let call = |name: &'static str, arguments: Value| {
        let peer = client.peer().clone();
        async move {
            let result = peer
                .call_tool(
                    rmcp::model::CallToolRequestParams::new(name)
                        .with_arguments(arguments.as_object().cloned().unwrap()),
                )
                .await
                .unwrap();
            let text = result.content[0].as_text().unwrap().text.clone();
            serde_json::from_str::<Value>(&text).unwrap()
        }
    };
    let review_id = call("open_review", json!({ "open": false })).await["reviewId"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        "add_findings",
        json!({ "findings": [{ "path": "src/lib.rs", "line": 2, "body": "Overflow?" }] }),
    )
    .await;
    let thread = store.get(&review_id).unwrap().threads[0].id.clone();
    store
        .add_message(
            &review_id,
            &thread,
            "Can it overflow?".to_string(),
            reviewer(),
            None,
        )
        .unwrap();
    store
        .set_thread_intent(&review_id, &thread, ThreadIntent::Question)
        .unwrap();
    assert_eq!(
        call("await_reviewer", json!({ "timeoutSec": 1 })).await["status"],
        "comment"
    );
    assert_eq!(
        store.get(&review_id).unwrap().threads[0].agent_state,
        Some(AgentState::Working)
    );

    client.cancel().await.unwrap();
    for _ in 0..100 {
        if store.get(&review_id).unwrap().threads[0].agent_state == Some(AgentState::Queued) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the thread stayed with the closed session");
}
