//! `co-review mcp`: an MCP server on stdio for agent harnesses, bridged to Co-Review's Streamable HTTP endpoint.
//!
//! Harnesses start this with every agent session, and starting the desktop app then would open it each time: the
//! handshake (initialize result, tool list) of the last session is cached in `mcp-handshake.json` and answered here,
//! and Co-Review starts on the first tool call. Co-Review can also go away under a session (the reviewer quits it, or
//! it closes itself when the review is done): the agent's calls then fail instead of waiting forever, and the next
//! one reconnects, starting Co-Review again and replaying the agent's `initialize`.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use crate::app::{self, App};
use crate::debug_log::log;
use crate::event_stream::Events;
use crate::installation::encode_uri_component;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
const TERMINATE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct BridgeOptions {
    pub app: Option<App>,
    pub port: Option<u16>,
    /// The repository the agent works in.
    pub root: PathBuf,
    pub home: PathBuf,
}

/// Runs the bridge until the agent's session ends (stdin closes), then ends Co-Review's session and exits.
pub fn run(options: BridgeOptions) -> ! {
    let bridge = Arc::new(Bridge::new(options));
    // With Co-Review running (or nothing cached yet) connect now and forward the handshake; otherwise answer it here.
    let cached = bridge.state().has_cached_handshake();
    if !cached || app::find_server(bridge.app.as_ref(), bridge.port).is_some() {
        let mut state = bridge.state();
        bridge.start_connection(&mut state, false);
    }
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(messages)) => messages
                .into_iter()
                .for_each(|message| bridge.receive(message)),
            Ok(message) => bridge.receive(message),
            Err(error) => log(&format!("not a JSON-RPC message from the agent: {error}")),
        }
    }
    bridge.end()
}

struct Bridge {
    app: Option<App>,
    port: Option<u16>,
    root: PathBuf,
    cache_file: PathBuf,
    http: ureq::Agent,
    state: Mutex<State>,
    /// Held while a check runs: whoever asks meanwhile waits for its outcome.
    checking: Mutex<()>,
    output: Mutex<io::Stdout>,
    ending: AtomicBool,
    next_internal_id: AtomicU64,
}

struct State {
    /// The handshake of the last session, kept fresh so the next one can be answered without starting Co-Review.
    cache: Map<String, Value>,
    /// The connection being made or made: every message to Co-Review waits for it. None when Co-Review is lost.
    connection: Option<Arc<Connection>>,
    next_connection: u64,
    next_attempt: u64,
    /// The agent's `initialize`, replayed on a new Co-Review session.
    initialize: Option<Value>,
    /// The review the agent last opened: a new Co-Review session rejoins it, so waits and replies stay on it.
    review_id: Option<String>,
    /// Agent requests forwarded to Co-Review and not answered yet, by id.
    inflight: HashMap<String, Inflight>,
}

impl State {
    fn has_cached_handshake(&self) -> bool {
        self.cache.contains_key("initialize") && self.cache.contains_key("tools")
    }

    fn is_current(&self, connection: u64) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|current| current.id == connection)
    }

    /// Records an agent request as waiting for its answer; the attempt tells this sending of it from a later one.
    fn register(&mut self, message: Value, watch: Option<Watch>, retried: bool) -> u64 {
        self.next_attempt += 1;
        let attempt = self.next_attempt;
        self.inflight.insert(
            id_key(&message),
            Inflight {
                message,
                attempt,
                connection: None,
                retried,
                watch,
            },
        );
        attempt
    }

    /// Takes the request off the waiting list if it is still this attempt's to answer.
    fn take(&mut self, key: &str, attempt: u64) -> Option<Inflight> {
        if self.inflight.get(key)?.attempt != attempt {
            return None;
        }
        self.inflight.remove(key)
    }
}

struct Inflight {
    message: Value,
    attempt: u64,
    /// The connection it was sent on.
    connection: Option<u64>,
    /// Sent again once already: a second failure is an error to the agent, not a loop.
    retried: bool,
    watch: Option<Watch>,
}

/// Answers the bridge keeps something from.
#[derive(Clone, Copy)]
enum Watch {
    Initialize,
    Tools,
    OpenReview,
}

struct Connection {
    id: u64,
    outcome: Mutex<Option<Result<Arc<Session>, String>>>,
    settled: Condvar,
}

impl Connection {
    fn wait(&self) -> Result<Arc<Session>, String> {
        let mut outcome = self.outcome.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(outcome) = outcome.as_ref() {
                return outcome.clone();
            }
            outcome = self
                .settled
                .wait(outcome)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn settle(&self, result: Result<Arc<Session>, String>) {
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        self.settled.notify_all();
    }

    fn session(&self) -> Option<Arc<Session>> {
        match self
            .outcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            Some(Ok(session)) => Some(session.clone()),
            _ => None,
        }
    }
}

/// One MCP session with Co-Review.
struct Session {
    connection: u64,
    endpoint: String,
    session_id: Mutex<Option<String>>,
    /// Signalled when the answer to `initialize` names the session (or never will).
    identified: Condvar,
    initializing: AtomicBool,
    protocol_version: Mutex<Option<String>>,
}

impl Session {
    fn new(connection: u64, endpoint: String) -> Self {
        Session {
            connection,
            endpoint,
            session_id: Mutex::new(None),
            identified: Condvar::new(),
            initializing: AtomicBool::new(false),
            protocol_version: Mutex::new(None),
        }
    }

    fn session_id(&self) -> Option<String> {
        self.session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn identify(&self, session_id: Option<&str>) {
        let mut current = self
            .session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(session_id) = session_id {
            *current = Some(session_id.to_string());
        }
        self.initializing.store(false, Ordering::SeqCst);
        self.identified.notify_all();
    }

    /// Holds a message the agent sent before the answer to its `initialize` arrived: without the session id that
    /// answer carries, Co-Review would refuse it.
    fn wait_for_identity(&self) {
        let mut current = self
            .session_id
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        while current.is_none() && self.initializing.load(Ordering::SeqCst) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            current = self
                .identified
                .wait_timeout(current, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The session's id and protocol version, once known.
    fn headers(&self, mut request: ureq::Request) -> ureq::Request {
        if let Some(session_id) = self.session_id() {
            request = request.set("mcp-session-id", &session_id);
        }
        if let Some(version) = self
            .protocol_version
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            request = request.set("mcp-protocol-version", &version);
        }
        request
    }

    fn set_protocol_version(&self, result: &Value) {
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
            *self
                .protocol_version
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(version.to_string());
        }
    }
}

fn id_key(message: &Value) -> String {
    message.get("id").map(Value::to_string).unwrap_or_default()
}

fn closed_message(reason: &str) -> String {
    format!(
        "Co-Review closed ({reason}). Call the tool again: it reconnects. If it keeps failing or Co-Review seems \
         stuck, run `co-review logs` and show the user what it says."
    )
}

fn unreachable_message(reason: &str) -> String {
    format!(
        "Co-Review is not reachable: {reason}. Run `co-review logs` and show the user what it says."
    )
}

impl Bridge {
    fn new(options: BridgeOptions) -> Self {
        let cache_file = options.home.join("mcp-handshake.json");
        let cache = std::fs::read_to_string(&cache_file)
            .ok()
            .and_then(|contents| serde_json::from_str::<Map<String, Value>>(&contents).ok())
            .unwrap_or_default();
        Bridge {
            app: options.app,
            port: options.port,
            root: options.root,
            cache_file,
            http: app::http_agent(),
            state: Mutex::new(State {
                cache,
                connection: None,
                next_connection: 0,
                next_attempt: 0,
                initialize: None,
                review_id: None,
                inflight: HashMap::new(),
            }),
            checking: Mutex::new(()),
            output: Mutex::new(io::stdout()),
            ending: AtomicBool::new(false),
            next_internal_id: AtomicU64::new(0),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_current(&self, connection: u64) -> bool {
        self.state().is_current(connection)
    }

    /// One message from the agent.
    fn receive(self: &Arc<Self>, message: Value) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = message.get("id").cloned();
        let handshake = method.as_deref() == Some("initialize");
        let mut state = self.state();
        if handshake {
            state.initialize = Some(message.clone());
        }
        if state.connection.is_none() && state.has_cached_handshake() {
            let local_answer = match (method.as_deref(), &id) {
                (Some("initialize"), Some(_)) => {
                    state.cache.get("initialize").cloned().map(|mut result| {
                        if let Some(result) = result.as_object_mut() {
                            match message.pointer("/params/protocolVersion") {
                                Some(version) => {
                                    result.insert("protocolVersion".to_string(), version.clone())
                                }
                                None => result.remove("protocolVersion"),
                            };
                        }
                        result
                    })
                }
                (Some("tools/list"), Some(_)) => state.cache.get("tools").cloned(),
                (Some("ping"), Some(_)) => Some(json!({})),
                // Notifications (`notifications/initialized`) need no Co-Review.
                (_, None) => return,
                _ => None,
            };
            if let (Some(result), Some(id)) = (local_answer, id.as_ref()) {
                drop(state);
                self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                return;
            }
        }
        if state.connection.is_none() {
            let replay = state.initialize.is_some() && !handshake;
            self.start_connection(&mut state, replay);
        }
        let is_request = id.is_some() && method.is_some();
        let watch = match method.as_deref() {
            Some("initialize") => Some(Watch::Initialize),
            Some("tools/list") => Some(Watch::Tools),
            Some("tools/call")
                if message.pointer("/params/name").and_then(Value::as_str)
                    == Some("open_review") =>
            {
                Some(Watch::OpenReview)
            }
            _ => None,
        };
        let attempt = is_request.then(|| state.register(message.clone(), watch, false));
        let Some(connection) = state.connection.clone() else {
            return;
        };
        drop(state);
        self.spawn_delivery(message, attempt, connection);
    }

    fn start_connection(self: &Arc<Self>, state: &mut State, replay: bool) {
        state.next_connection += 1;
        let connection = Arc::new(Connection {
            id: state.next_connection,
            outcome: Mutex::new(None),
            settled: Condvar::new(),
        });
        state.connection = Some(connection.clone());
        let bridge = self.clone();
        std::thread::spawn(move || {
            let outcome = bridge
                .connect(connection.id, replay)
                .map_err(|error| format!("{error:#}"));
            if outcome.is_err() {
                // Tried again on the next call if Co-Review did not start.
                let mut state = bridge.state();
                if state.is_current(connection.id) {
                    state.connection = None;
                }
            }
            connection.settle(outcome);
        });
    }

    fn connect(self: &Arc<Self>, connection: u64, replay: bool) -> Result<Arc<Session>> {
        let base = app::ensure_server(self.app.as_ref(), self.port, &self.root, false)?;
        let endpoint = format!(
            "{base}/mcp?root={}",
            encode_uri_component(&self.root.to_string_lossy())
        );
        let session = Arc::new(Session::new(connection, endpoint));
        if !replay {
            return Ok(session);
        }
        let params = self
            .state()
            .initialize
            .as_ref()
            .and_then(|initialize| initialize.get("params").cloned())
            .unwrap_or_else(|| json!({}));
        let initialized = self.request(&session, "initialize", params, REQUEST_TIMEOUT)?;
        session.set_protocol_version(&initialized);
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        let response = self
            .post(&session, &notification, Some(REQUEST_TIMEOUT))
            .map_err(|error| anyhow!(error))?;
        self.read_messages(response, &mut |message| self.on_message(&session, message))
            .map_err(|error| anyhow!(error))?;
        self.open_standalone_stream(session.clone());
        let tools = self.request(&session, "tools/list", json!({}), REQUEST_TIMEOUT)?;
        let changed = {
            let mut state = self.state();
            let changed = state.cache.get("tools") != Some(&tools);
            if changed {
                self.remember(&mut state, "tools", tools);
            }
            changed
        };
        if changed {
            self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" }));
        }
        let review_id = self.state().review_id.clone();
        if let Some(review_id) = review_id {
            // Without it the session still works: the agent's next open_review picks the review.
            let arguments = json!({ "name": "open_review", "arguments": { "reviewId": review_id, "open": false } });
            if let Err(error) = self.request(&session, "tools/call", arguments, REQUEST_TIMEOUT) {
                log(&format!("could not rejoin review {review_id}: {error:#}"));
                self.state().review_id = None;
            }
        }
        Ok(session)
    }

    /// Sends the agent's message to Co-Review once connected, and its answer back; a failure is told to the agent,
    /// or the call sent again on a new session.
    fn spawn_delivery(
        self: &Arc<Self>,
        message: Value,
        attempt: Option<u64>,
        connection: Arc<Connection>,
    ) {
        let bridge = self.clone();
        std::thread::spawn(move || bridge.deliver(message, attempt, &connection));
    }

    fn deliver(self: &Arc<Self>, message: Value, attempt: Option<u64>, connection: &Connection) {
        let key = id_key(&message);
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let session = match connection.wait() {
            Ok(session) => session,
            Err(error) => {
                log(&error);
                if let Some(attempt) = attempt
                    && self.state().take(&key, attempt).is_some()
                {
                    self.fail(&id, &unreachable_message(&error));
                }
                return;
            }
        };
        if let Some(attempt) = attempt {
            let mut state = self.state();
            match state.inflight.get_mut(&key) {
                Some(entry) if entry.attempt == attempt => {
                    entry.connection = Some(session.connection)
                }
                // Answered meanwhile (Co-Review was lost, and the call failed or was sent again).
                _ => return,
            }
        }
        if message.get("method").and_then(Value::as_str) == Some("initialize") {
            session.initializing.store(true, Ordering::SeqCst);
        } else {
            session.wait_for_identity();
        }
        match self.post(&session, &message, None) {
            Ok(response) => {
                if message.get("method").and_then(Value::as_str)
                    == Some("notifications/initialized")
                {
                    self.open_standalone_stream(session.clone());
                }
                if let Err(error) =
                    self.read_messages(response, &mut |reply| self.on_message(&session, reply))
                {
                    log(&format!("http: {error}"));
                }
                let Some(attempt) = attempt else {
                    return;
                };
                if !self
                    .state()
                    .inflight
                    .get(&key)
                    .is_some_and(|entry| entry.attempt == attempt)
                {
                    return;
                }
                // The answer's stream ended without it: Co-Review may be gone.
                self.check(&session);
                if self.state().take(&key, attempt).is_some() {
                    self.fail(
                        &id,
                        "Co-Review ended the call without an answer. Call the tool again. If it keeps failing, \
                         run `co-review logs` and show the user what it says.",
                    );
                }
            }
            Err(error) => {
                if self.is_current(session.connection) {
                    self.lost(session.connection, &error);
                }
                if let Some(attempt) = attempt
                    && self.state().take(&key, attempt).is_some()
                {
                    self.fail(&id, &unreachable_message(&error));
                }
            }
        }
    }

    /// One message from Co-Review on `session`, passed on to the agent.
    fn on_message(&self, session: &Session, message: Value) {
        let mut state = self.state();
        // A session given up: the agent was already told, and a new session answers instead.
        if !state.is_current(session.connection) {
            return;
        }
        if message.get("id").is_some() && message.get("method").is_none() {
            let key = id_key(&message);
            if !state
                .inflight
                .get(&key)
                .is_some_and(|entry| entry.connection == Some(session.connection))
            {
                return;
            }
            let watch = state.inflight.remove(&key).and_then(|entry| entry.watch);
            match (watch, message.get("result")) {
                (Some(Watch::OpenReview), Some(result)) => {
                    // The review the agent opened: a reconnect rejoins it.
                    if let Some(review_id) = result
                        .pointer("/content/0/text")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .and_then(|opened| opened.get("reviewId")?.as_str().map(str::to_string))
                    {
                        state.review_id = Some(review_id);
                    }
                }
                (Some(Watch::Initialize), Some(result)) => {
                    session.set_protocol_version(result);
                    self.remember(&mut state, "initialize", result.clone());
                }
                (Some(Watch::Tools), Some(result)) => {
                    self.remember(&mut state, "tools", result.clone())
                }
                _ => {}
            }
        }
        drop(state);
        self.send(&message);
    }

    /// Co-Review is gone, or forgot this session: the agent's waiting calls fail, or, when Co-Review answers but not
    /// for this session (it was restarted, or let the session go), are sent again once on a new session without the
    /// agent noticing. None runs twice: the old session never got them.
    fn lost(self: &Arc<Self>, connection: u64, reason: &str) {
        let mut state = self.state();
        if !state.is_current(connection) {
            return;
        }
        log(&format!("lost Co-Review: {reason}"));
        state.connection = None;
        let session_forgotten = reason.contains("No valid MCP session");
        let (again, failed): (Vec<Inflight>, Vec<Inflight>) = state
            .inflight
            .drain()
            .map(|(_, entry)| entry)
            .partition(|entry| session_forgotten && !entry.retried);
        let mut deliveries = Vec::new();
        if !again.is_empty() {
            log(&format!(
                "sending {} call(s) again on a new session",
                again.len()
            ));
            self.start_connection(&mut state, true);
            for entry in again {
                let attempt = state.register(entry.message.clone(), entry.watch, true);
                deliveries.push((entry.message, attempt));
            }
        }
        let connection = state.connection.clone();
        drop(state);
        for entry in failed {
            let id = entry.message.get("id").cloned().unwrap_or(Value::Null);
            self.fail(&id, &closed_message(reason));
        }
        if let Some(connection) = connection {
            for (message, attempt) in deliveries {
                self.spawn_delivery(message, Some(attempt), connection.clone());
            }
        }
    }

    /// Checks that the session still answers (a stream of it ended or failed); Co-Review is lost if it does not.
    fn check(self: &Arc<Self>, session: &Session) {
        let _checking = self.checking.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.is_current(session.connection) {
            return;
        }
        if let Err(error) = self.request(session, "ping", json!({}), PING_TIMEOUT) {
            self.lost(session.connection, &format!("{error:#}"));
        }
    }

    /// The session's own stream, for what Co-Review sends unasked (the reviewer's questions pushed into Claude Code's
    /// session); opened again while the session lasts.
    fn open_standalone_stream(self: &Arc<Self>, session: Arc<Session>) {
        let bridge = self.clone();
        std::thread::spawn(move || {
            loop {
                if !bridge.is_current(session.connection) {
                    return;
                }
                let request = bridge
                    .http
                    .get(&session.endpoint)
                    .set("Accept", "text/event-stream");
                match session.headers(request).call() {
                    Ok(response) => {
                        if let Err(error) = bridge.read_messages(response, &mut |message| {
                            bridge.on_message(&session, message)
                        }) {
                            log(&format!("http: {error}"));
                        }
                    }
                    // Co-Review offers no such stream.
                    Err(ureq::Error::Status(405, _)) => return,
                    Err(error) => log(&format!("http: {error}")),
                }
                bridge.check(&session);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    /// A request of the bridge's own on `session`; its answer comes back here, not to the agent.
    fn request(
        self: &Arc<Self>,
        session: &Session,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = format!(
            "co-review:{}",
            self.next_internal_id.fetch_add(1, Ordering::SeqCst)
        );
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self
            .post(session, &message, Some(timeout))
            .map_err(|error| anyhow!(error))?;
        let mut reply = None;
        self.read_messages(response, &mut |message| {
            if message.get("id").and_then(Value::as_str) == Some(id.as_str()) {
                reply = Some(message);
            } else {
                self.on_message(session, message);
            }
        })
        .map_err(|error| anyhow!("{method} failed: {error}"))?;
        let reply = reply.with_context(|| format!("{method} was not answered"))?;
        if let Some(error) = reply.get("error") {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("{text}");
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }

    /// POSTs one message to Co-Review's endpoint; errors read like the MCP SDK's, which tell a forgotten session.
    fn post(
        &self,
        session: &Session,
        message: &Value,
        timeout: Option<Duration>,
    ) -> Result<ureq::Response, String> {
        let mut request = session.headers(
            self.http
                .post(&session.endpoint)
                .set("Content-Type", "application/json")
                .set("Accept", "application/json, text/event-stream"),
        );
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let body = serde_json::to_string(message).map_err(|error| error.to_string())?;
        let initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        let result = request.send_string(&body);
        if let Ok(response) = &result
            && let Some(session_id) = response.header("mcp-session-id")
        {
            session.identify(Some(session_id));
        }
        if initialize {
            session.identify(None);
        }
        match result {
            Ok(response) => Ok(response),
            Err(ureq::Error::Status(status, response)) => {
                let text = response.into_string().unwrap_or_default();
                Err(format!("Error POSTing to endpoint (HTTP {status}): {text}"))
            }
            Err(ureq::Error::Transport(transport)) => Err(transport.to_string()),
        }
    }

    /// The messages of a response (JSON or an event stream, read as it comes) until it ends.
    fn read_messages(
        &self,
        response: ureq::Response,
        handle: &mut dyn FnMut(Value),
    ) -> Result<(), String> {
        if response.status() == 202 {
            return Ok(());
        }
        let content_type = response.content_type().to_string();
        let mut deliver = |text: &str| match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(messages)) => messages.into_iter().for_each(&mut *handle),
            Ok(message) => handle(message),
            Err(error) => log(&format!("not a JSON-RPC message from Co-Review: {error}")),
        };
        let reader = response.into_reader();
        if content_type == "text/event-stream" {
            for event in Events::new(BufReader::new(reader)) {
                deliver(&event.map_err(|error| error.to_string())?);
            }
        } else if content_type == "application/json" {
            let mut text = String::new();
            BufReader::new(reader)
                .read_to_string(&mut text)
                .map_err(|error| error.to_string())?;
            if !text.trim().is_empty() {
                deliver(&text);
            }
        }
        Ok(())
    }

    fn remember(&self, state: &mut State, key: &str, value: Value) {
        state.cache.insert(key.to_string(), value);
        if let Some(directory) = self.cache_file.parent() {
            std::fs::create_dir_all(directory).ok();
        }
        if let Ok(text) = serde_json::to_string(&state.cache)
            && let Err(error) = std::fs::write(&self.cache_file, text)
        {
            log(&format!("could not save the MCP handshake: {error}"));
        }
    }

    fn fail(&self, id: &Value, message: &str) {
        self.send(
            &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": message } }),
        );
    }

    /// One line to the agent; when nobody reads them any more, the session is over.
    fn send(&self, message: &Value) {
        let written = {
            let mut output = self.output.lock().unwrap_or_else(PoisonError::into_inner);
            serde_json::to_writer(&mut *output, message)
                .map_err(io::Error::from)
                .and_then(|()| output.write_all(b"\n"))
                .and_then(|()| output.flush())
        };
        if written.is_err() {
            self.end();
        }
    }

    /// The agent's session ended: end ours too, so Co-Review knows this agent is done, and exit instead of lingering
    /// on the open HTTP stream.
    fn end(&self) -> ! {
        if self.ending.swap(true, Ordering::SeqCst) {
            // Another thread is ending the process.
            loop {
                std::thread::park();
            }
        }
        log("the agent's session ended");
        let session = self
            .state()
            .connection
            .as_ref()
            .and_then(|connection| connection.session());
        if let Some(session) = session
            && let Some(session_id) = session.session_id()
        {
            self.http
                .request("DELETE", &session.endpoint)
                .set("mcp-session-id", &session_id)
                .timeout(TERMINATE_TIMEOUT)
                .call()
                .ok();
        }
        std::process::exit(0)
    }
}
