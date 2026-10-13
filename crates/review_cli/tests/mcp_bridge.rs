//! The `co-review mcp` bridge as an agent's harness sees it (port of extensions/review/test/mcp-bridge.test.mjs, and
//! the bridge's other promises): the built binary on stdio, a fake Co-Review behind it speaking Streamable HTTP.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default, Clone, Debug, PartialEq)]
struct Stats {
    initializes: u32,
    calls: u32,
    deletes: u32,
    streams: u32,
    open_reviews: Vec<Value>,
}

/// A Co-Review-like MCP server: a few tools, sessions it can be told to forget, and a stop that severs every
/// connection (as when the app quits).
struct FakeCoReview {
    port: u16,
    shared: Arc<Shared>,
}

struct Shared {
    /// Taken by `stop`: no connection is accepted after it returns.
    listener: Mutex<Option<TcpListener>>,
    sessions: Mutex<HashSet<String>>,
    stats: Mutex<Stats>,
    connections: Mutex<Vec<TcpStream>>,
    standalone_streams: Mutex<Vec<TcpStream>>,
    next_session: AtomicU64,
    tools: Value,
}

fn tool_list() -> Value {
    json!({ "tools": [
        { "name": "echo", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } } },
        { "name": "wait", "inputSchema": { "type": "object" } },
        { "name": "open_review", "inputSchema": { "type": "object" } }
    ] })
}

impl FakeCoReview {
    fn start(port: u16) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let listener = loop {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => break listener,
                Err(error) if Instant::now() > deadline => panic!("bind {port}: {error}"),
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let port = listener.local_addr().expect("address").port();
        listener.set_nonblocking(true).expect("nonblocking");
        let shared = Arc::new(Shared {
            listener: Mutex::new(Some(listener)),
            sessions: Mutex::default(),
            stats: Mutex::default(),
            connections: Mutex::default(),
            standalone_streams: Mutex::default(),
            next_session: AtomicU64::new(0),
            tools: tool_list(),
        });
        let accepting = shared.clone();
        std::thread::spawn(move || {
            loop {
                // Accepted and registered under the listener's lock, so `stop` cuts every connection it let in.
                let accepted = match &*accepting.listener.lock().expect("lock") {
                    Some(listener) => listener.accept().map(|(stream, _)| {
                        if let Ok(clone) = stream.try_clone() {
                            accepting.connections.lock().expect("lock").push(clone);
                        }
                        stream
                    }),
                    None => return,
                };
                match accepted {
                    Ok(stream) => {
                        stream.set_nonblocking(false).expect("blocking");
                        let shared = accepting.clone();
                        std::thread::spawn(move || shared.handle(stream));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        FakeCoReview { port, shared }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn stats(&self) -> Stats {
        self.shared.stats.lock().expect("lock").clone()
    }

    fn forget(&self) {
        self.shared.sessions.lock().expect("lock").clear();
    }

    /// Like the app quitting: no new connections, and every open one cut.
    fn stop(&self) {
        self.shared.listener.lock().expect("lock").take();
        for connection in self.shared.connections.lock().expect("lock").drain(..) {
            connection.shutdown(Shutdown::Both).ok();
        }
        self.shared.standalone_streams.lock().expect("lock").clear();
    }

    /// A message Co-Review sends unasked, on every session's own stream.
    fn push(&self, message: &Value) {
        for stream in self
            .shared
            .standalone_streams
            .lock()
            .expect("lock")
            .iter_mut()
        {
            stream
                .write_all(format!("event: message\ndata: {message}\n\n").as_bytes())
                .ok();
        }
    }

    fn wait_for(&self, what: &str, condition: impl Fn(&Stats) -> bool) {
        let deadline = Instant::now() + TIMEOUT;
        while !condition(&self.stats()) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {:?}",
                self.stats()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for FakeCoReview {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Request {
    method: String,
    session: Option<String>,
    body: Value,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let method = line.split(' ').next()?.to_string();
    let mut length = 0;
    let mut session = None;
    loop {
        line.clear();
        reader.read_line(&mut line).ok()?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().ok()?,
            "mcp-session-id" => session = Some(value.trim().to_string()),
            _ => {}
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        session,
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    })
}

fn respond(mut stream: &TcpStream, status: &str, headers: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).ok();
    stream.shutdown(Shutdown::Both).ok();
}

/// An event stream answering one request: the messages, then the end.
fn respond_with_events(mut stream: &TcpStream, session: &str, messages: &[Value]) {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nmcp-session-id: {session}\r\nConnection: close\r\n\r\n"
    );
    for message in messages {
        response.push_str(&format!("event: message\ndata: {message}\n\n"));
    }
    stream.write_all(response.as_bytes()).ok();
    stream.shutdown(Shutdown::Both).ok();
}

fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn text(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

impl Shared {
    fn handle(&self, stream: TcpStream) {
        let Some(request) = read_request(&stream) else {
            return;
        };
        let known = request
            .session
            .as_ref()
            .filter(|session| self.sessions.lock().expect("lock").contains(*session))
            .cloned();
        let initialize = request.body.get("method").and_then(Value::as_str) == Some("initialize");
        let session = match (known, request.method.as_str(), initialize) {
            (Some(session), _, _) => session,
            (None, "POST", true) => {
                let session = format!(
                    "session-{}",
                    self.next_session.fetch_add(1, Ordering::SeqCst)
                );
                self.sessions.lock().expect("lock").insert(session.clone());
                session
            }
            _ => {
                let status = if request.session.is_some() {
                    "404 Not Found"
                } else {
                    "400 Bad Request"
                };
                let body = json!({ "jsonrpc": "2.0", "error": { "code": -32000, "message": "No valid MCP session" }, "id": null });
                return respond(
                    &stream,
                    status,
                    "Content-Type: application/json\r\n",
                    &body.to_string(),
                );
            }
        };
        match request.method.as_str() {
            "GET" => {
                self.stats.lock().expect("lock").streams += 1;
                let mut writer = &stream;
                writer
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
                    .ok();
                if let Ok(clone) = stream.try_clone() {
                    self.standalone_streams.lock().expect("lock").push(clone);
                }
            }
            "DELETE" => {
                self.stats.lock().expect("lock").deletes += 1;
                self.sessions.lock().expect("lock").remove(&session);
                respond(&stream, "200 OK", "", "");
            }
            _ => self.handle_message(&stream, &session, &request.body),
        }
    }

    fn handle_message(&self, stream: &TcpStream, session: &str, message: &Value) {
        let Some(id) = message.get("id") else {
            return respond(stream, "202 Accepted", "", "");
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let messages = match method {
            "initialize" => {
                self.stats.lock().expect("lock").initializes += 1;
                vec![result(
                    id,
                    json!({
                        "protocolVersion": message.pointer("/params/protocolVersion"),
                        "capabilities": { "experimental": { "claude/channel": {} }, "tools": { "listChanged": true } },
                        "serverInfo": { "name": "co-review", "version": "0.1.0" },
                        "instructions": "Co-Review is a repository review app."
                    }),
                )]
            }
            "ping" => vec![result(id, json!({}))],
            "tools/list" => vec![result(id, self.tools.clone())],
            "tools/call" => {
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                match message.pointer("/params/name").and_then(Value::as_str) {
                    Some("echo") => {
                        self.stats.lock().expect("lock").calls += 1;
                        let said = arguments
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        vec![
                            json!({ "jsonrpc": "2.0", "method": "notifications/message", "params": { "level": "info", "data": format!("echoing {said}") } }),
                            result(id, text(&format!("echo:{said}"))),
                        ]
                    }
                    Some("wait") => {
                        self.stats.lock().expect("lock").calls += 1;
                        let mut writer = stream;
                        writer
                            .write_all(
                                format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nmcp-session-id: {session}\r\nConnection: close\r\n\r\n")
                                    .as_bytes(),
                            )
                            .ok();
                        // Answered never: the reviewer is away until Co-Review quits.
                        return;
                    }
                    Some("open_review") => {
                        self.stats
                            .lock()
                            .expect("lock")
                            .open_reviews
                            .push(arguments);
                        vec![result(
                            id,
                            text(r#"{"reviewId":"review-1","note":"opened"}"#),
                        )]
                    }
                    _ => vec![
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "unknown tool" } }),
                    ],
                }
            }
            _ => vec![
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "unknown method" } }),
            ],
        };
        respond_with_events(stream, session, &messages);
    }
}

/// The bridge as an agent's harness sees it: JSON-RPC lines over stdio.
struct Agent {
    child: Child,
    stdin: Option<ChildStdin>,
    received: Arc<(Mutex<Vec<Value>>, Condvar)>,
    next_id: u64,
}

impl Agent {
    fn start(home: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_co-review"));
        command
            .arg("mcp")
            .args(arguments)
            .env("CO_REVIEW_HOME", home)
            .env("CO_REVIEW_APP", "none")
            .env_remove("CO_REVIEW_PORT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("co-review mcp");
        let stdout = child.stdout.take().expect("stdout");
        let received: Arc<(Mutex<Vec<Value>>, Condvar)> = Arc::default();
        let receiving = received.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                let message: Value = serde_json::from_str(&line).expect("a JSON-RPC line");
                let (messages, arrived) = &*receiving;
                messages.lock().expect("lock").push(message);
                arrived.notify_all();
            }
        });
        Agent {
            stdin: child.stdin.take(),
            child,
            received,
            next_id: 0,
        }
    }

    fn write(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").expect("write to the bridge");
        stdin.flush().expect("flush");
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        self.next_id += 1;
        let message =
            json!({ "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params });
        self.write(&message);
        self.next_id
    }

    fn notify(&mut self, method: &str) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method }));
    }

    /// The first message received that matches.
    fn wait_for(&self, what: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let (messages, arrived) = &*self.received;
        let deadline = Instant::now() + TIMEOUT;
        let mut messages = messages.lock().expect("lock");
        loop {
            if let Some(found) = messages.iter().find(|message| matches(message)) {
                return found.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "{what} not received in 10 s; received {messages:?}"
            );
            messages = arrived.wait_timeout(messages, left).expect("lock").0;
        }
    }

    fn reply(&self, id: u64) -> Value {
        self.wait_for(&format!("the reply to {id}"), |message| {
            message.get("id") == Some(&json!(id)) && message.get("method").is_none()
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.reply(id)
    }

    fn initialize(&mut self) -> Value {
        let reply = self.request(
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
        );
        self.notify("notifications/initialized");
        reply
    }

    fn start_call(&mut self, name: &str, arguments: Value) -> u64 {
        self.send(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }

    fn outcome(&self, id: u64) -> String {
        let reply = self.reply(id);
        match reply
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
        {
            Some(text) => text.to_string(),
            None => format!(
                "error: {}",
                reply
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
        }
    }

    fn call(&mut self, said: &str) -> String {
        let id = self.start_call("echo", json!({ "text": said }));
        self.outcome(id)
    }

    fn received(&self) -> Vec<Value> {
        self.received.0.lock().expect("lock").clone()
    }

    /// Closes stdin, as a harness ending the session does, and waits for the bridge to exit.
    fn end(&mut self) -> ExitStatus {
        self.stdin.take();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("status") {
                return status;
            }
            assert!(Instant::now() < deadline, "the bridge did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn here() -> String {
    std::env::current_dir()
        .expect("current directory")
        .display()
        .to_string()
}

#[test]
fn a_call_after_co_review_forgot_the_session_is_answered_on_a_new_session_once_without_the_agent_noticing()
 {
    let home = home();
    let server = FakeCoReview::start(0);
    let port = server.port.to_string();
    let mut agent = Agent::start(home.path(), &["--port", &port, &here()], &[]);
    agent.initialize();
    assert_eq!(agent.call("one"), "echo:one");
    assert_eq!((server.stats().initializes, server.stats().calls), (1, 1));

    server.forget();
    assert_eq!(agent.call("two"), "echo:two");
    assert_eq!(agent.call("three"), "echo:three");
    assert_eq!(
        (server.stats().initializes, server.stats().calls),
        (2, 3),
        "one new session, every call ran once"
    );

    // Two calls in flight when the session goes: both come back, on one new session.
    server.forget();
    let four = agent.start_call("echo", json!({ "text": "four" }));
    let five = agent.start_call("echo", json!({ "text": "five" }));
    assert_eq!(
        [agent.outcome(four), agent.outcome(five)],
        ["echo:four", "echo:five"]
    );
    assert_eq!((server.stats().initializes, server.stats().calls), (3, 5));
}

#[test]
fn the_handshake_is_answered_without_co_review_which_starts_in_the_background_on_the_first_tool_call()
 {
    let home = home();
    let cached_tools =
        json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] });
    let cached_initialize = json!({
        "protocolVersion": "2025-11-25",
        "capabilities": { "experimental": { "claude/channel": {} }, "tools": { "listChanged": true } },
        "serverInfo": { "name": "co-review", "version": "0.1.0" },
        "instructions": "cached instructions"
    });
    fs::write(
        home.path().join("mcp-handshake.json"),
        json!({ "initialize": cached_initialize, "tools": cached_tools }).to_string(),
    )
    .expect("handshake");
    // The "app": records how it was started.
    let app = home.path().join("app.sh");
    fs::write(
        &app,
        "#!/bin/sh\necho \"$@\" > \"$CO_REVIEW_HOME/started\"\n",
    )
    .expect("app");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let app = app.display().to_string();
    let mut agent = Agent::start(home.path(), &[&here()], &[("CO_REVIEW_APP", &app)]);

    let started = Instant::now();
    let initialized = agent.initialize();
    let mut expected = cached_initialize.clone();
    expected["protocolVersion"] = json!("2025-06-18");
    assert_eq!(initialized["result"], expected);
    assert_eq!(
        agent.request("tools/list", json!({}))["result"],
        cached_tools
    );
    assert_eq!(agent.request("ping", json!({}))["result"], json!({}));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "answered locally: {:?}",
        started.elapsed()
    );
    assert!(
        !home.path().join("started").exists(),
        "Co-Review is not started by the handshake"
    );

    let call = agent.start_call("echo", json!({ "text": "one" }));
    let deadline = Instant::now() + TIMEOUT;
    while !home.path().join("started").exists() {
        assert!(Instant::now() < deadline, "the app was not started");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(home.path().join("started"))
            .expect("started")
            .trim(),
        "--background"
    );
    // The app comes up and registers its backend.
    let server = FakeCoReview::start(0);
    fs::write(
        home.path().join("server.json"),
        json!({ "url": server.url(), "pid": std::process::id() }).to_string(),
    )
    .expect("server.json");

    assert_eq!(agent.outcome(call), "echo:one");
    assert_eq!(
        (server.stats().initializes, server.stats().calls),
        (1, 1),
        "the agent's initialize replayed once"
    );
    // Its tool list differs from the cached one: the agent is told, and the cache is refreshed.
    agent.wait_for("tools/list_changed", |message| {
        message.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed")
    });
    let cache: Value = serde_json::from_str(
        &fs::read_to_string(home.path().join("mcp-handshake.json")).expect("cache"),
    )
    .expect("json");
    assert_eq!(cache["tools"], tool_list());
}

#[test]
fn a_stateless_discovery_probe_is_refused_as_unknown_without_starting_co_review() {
    let home = home();
    // A handshake from an earlier session: the bridge starts nothing until it must.
    fs::write(
        home.path().join("mcp-handshake.json"),
        json!({ "initialize": { "protocolVersion": "2025-11-25", "capabilities": {} }, "tools": tool_list() })
            .to_string(),
    )
    .expect("handshake");
    let app = home.path().join("app.sh");
    fs::write(
        &app,
        "#!/bin/sh\necho \"$@\" > \"$CO_REVIEW_HOME/started\"\n",
    )
    .expect("app");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    let app = app.display().to_string();
    let mut agent = Agent::start(home.path(), &[&here()], &[("CO_REVIEW_APP", &app)]);
    // As Claude Code 2.1 sends it, before `initialize`.
    agent.write(
        &json!({ "jsonrpc": "2.0", "id": "server-discover-probe-1", "method": "server/discover",
        "params": { "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" } } }),
    );
    let reply = agent.wait_for("the probe's answer", |message| {
        message.get("id") == Some(&json!("server-discover-probe-1"))
    });
    assert_eq!(reply["error"]["code"], json!(-32601), "{reply}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !home.path().join("started").exists(),
        "Co-Review is not started by the probe"
    );
}

#[test]
fn a_probe_before_initialize_is_not_found_and_does_not_cost_the_session() {
    let home = home();
    let server = FakeCoReview::start(0);
    let port = server.port.to_string();
    let mut agent = Agent::start(home.path(), &["--port", &port, &here()], &[]);
    assert_eq!(
        agent.request("server/discover", json!({}))["error"]["code"],
        json!(-32601)
    );
    // Anything else before `initialize` too; a ping is answered, a notification dropped.
    let unknown = agent.request("tools/list", json!({}));
    assert_eq!(unknown["error"]["code"], json!(-32601), "{unknown}");
    assert_eq!(
        unknown["error"]["message"],
        json!("Method not found: tools/list")
    );
    assert_eq!(agent.request("ping", json!({}))["result"], json!({}));
    agent.notify("notifications/cancelled");
    agent.initialize();
    assert_eq!(agent.call("one"), "echo:one");
    assert_eq!(
        (server.stats().initializes, server.stats().calls),
        (1, 1)
    );
}

#[test]
fn a_probe_before_initialize_does_not_start_co_review_and_the_handshake_comes_from_the_cache() {
    let home = home();
    let cached_tools = json!({ "tools": [] });
    fs::write(
        home.path().join("mcp-handshake.json"),
        json!({
            "initialize": { "protocolVersion": "2025-06-18", "capabilities": { "tools": {} },
                "serverInfo": { "name": "co-review", "version": "0.1.0" } },
            "tools": cached_tools
        })
        .to_string(),
    )
    .expect("handshake");
    // Nothing listens on this port: the bridge would start Co-Review there.
    let port = {
        let server = FakeCoReview::start(0);
        server.port.to_string()
    };
    let mut agent = Agent::start(home.path(), &["--port", &port, &here()], &[]);
    assert_eq!(
        agent.request("server/discover", json!({}))["error"]["code"],
        json!(-32601)
    );
    let initialized = agent.request(
        "initialize",
        json!({ "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    );
    assert_eq!(initialized["result"]["serverInfo"]["name"], json!("co-review"));
    agent.notify("notifications/initialized");
    assert_eq!(
        agent.request("tools/list", json!({}))["result"],
        cached_tools
    );
    let log = fs::read_to_string(home.path().join("logs").join("cli.log")).unwrap_or_default();
    assert!(
        !log.contains("starting the Co-Review") && !log.contains("lost Co-Review"),
        "{log}"
    );
}

#[test]
fn without_a_cached_handshake_or_any_co_review_the_agent_is_told_it_is_not_reachable() {
    let home = home();
    let mut agent = Agent::start(home.path(), &["--port", "1", &here()], &[]);
    let reply = agent.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    );
    let message = reply
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.starts_with("Co-Review is not reachable: Co-Review is not running"),
        "{message}"
    );
    assert!(
        message.ends_with("Run `co-review logs` and show the user what it says."),
        "{message}"
    );
}

#[test]
fn a_call_waiting_when_co_review_goes_away_fails_and_the_next_call_reconnects() {
    let home = home();
    let server = FakeCoReview::start(0);
    let port = server.port;
    let mut agent = Agent::start(home.path(), &["--port", &port.to_string(), &here()], &[]);
    agent.initialize();
    let waiting = agent.start_call("wait", json!({}));
    server.wait_for("the wait call", |stats| stats.calls == 1);

    server.stop();
    let outcome = agent.outcome(waiting);
    assert!(
        outcome.starts_with("error: Co-Review closed ("),
        "{outcome}"
    );
    assert!(
        outcome.contains("Call the tool again: it reconnects."),
        "{outcome}"
    );
    drop(server);

    let restarted = FakeCoReview::start(port);
    assert_eq!(agent.call("again"), "echo:again");
    assert_eq!(
        (restarted.stats().initializes, restarted.stats().calls),
        (1, 1)
    );
}

#[test]
fn the_session_ends_when_the_agent_closes_stdin() {
    let home = home();
    let server = FakeCoReview::start(0);
    let mut agent = Agent::start(
        home.path(),
        &["--port", &server.port.to_string(), &here()],
        &[],
    );
    agent.initialize();
    assert_eq!(agent.call("one"), "echo:one");
    let status = agent.end();
    assert!(status.success(), "{status}");
    assert_eq!(server.stats().deletes, 1);
}

#[test]
fn what_co_review_sends_unasked_or_along_with_an_answer_reaches_the_agent() {
    let home = home();
    let server = FakeCoReview::start(0);
    let mut agent = Agent::start(
        home.path(),
        &["--port", &server.port.to_string(), &here()],
        &[],
    );
    agent.initialize();
    server.wait_for("the session's own stream", |stats| stats.streams == 1);

    let channel = json!({ "jsonrpc": "2.0", "method": "notifications/claude/channel", "params": { "content": "a question", "meta": { "thread_id": "t1" } } });
    server.push(&channel);
    assert_eq!(
        agent.wait_for("the channel message", |message| message == &channel),
        channel
    );

    let id = agent.start_call("echo", json!({ "text": "one" }));
    assert_eq!(agent.outcome(id), "echo:one");
    let received = agent.received();
    let progress = received
        .iter()
        .position(|message| {
            message.get("method").and_then(Value::as_str) == Some("notifications/message")
        })
        .expect("the notification sent with the answer");
    let answer = received
        .iter()
        .position(|message| message.get("id") == Some(&json!(id)))
        .expect("the answer");
    assert!(progress < answer, "in the order Co-Review sent them");
}

#[test]
fn a_new_session_rejoins_the_review_the_agent_opened() {
    let home = home();
    let server = FakeCoReview::start(0);
    let mut agent = Agent::start(
        home.path(),
        &["--port", &server.port.to_string(), &here()],
        &[],
    );
    agent.initialize();
    let opened = agent.start_call("open_review", json!({ "dir": "/tmp/review" }));
    assert_eq!(
        agent.outcome(opened),
        r#"{"reviewId":"review-1","note":"opened"}"#
    );

    server.forget();
    assert_eq!(agent.call("one"), "echo:one");
    assert_eq!(
        server.stats().open_reviews,
        vec![
            json!({ "dir": "/tmp/review" }),
            json!({ "reviewId": "review-1", "open": false })
        ]
    );
}
