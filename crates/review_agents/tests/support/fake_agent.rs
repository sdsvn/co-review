//! A scripted ACP agent over stdio, for the tests and the TypeScript recorder (`tests/gen/record.mjs`).
//!
//! Every message it receives is appended to the file named by its first argument, or `$FAKE_ACP_LOG` (JSON lines), so
//! a test can check what the client sent.
//! What a prompt does depends on words in its last text block:
//! - `permission`: asks for permission, then says which option was chosen;
//! - `slow`: streams a little, then waits for `session/cancel`;
//! - `read:<path>` / `write:<path>`: reads or writes the file through the client;
//! - `settings`: says the session's model and effort;
//! - `refuse`: stops with `refusal` and no text;
//! - `crash`: streams a little, then exits with code 3;
//! - anything else: a first message ("Let me look."), a tool call, then the answer in a second message.

use std::collections::HashMap;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

struct Agent {
    lines: std::io::Lines<std::io::StdinLock<'static>>,
    log: Option<std::fs::File>,
    sessions: HashMap<String, (String, String)>,
    next_session: u32,
    next_request: u64,
    cancelled: bool,
}

fn main() {
    let log_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("FAKE_ACP_LOG").ok());
    let log = log_path.and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let mut agent = Agent {
        lines: std::io::stdin().lock().lines(),
        log,
        sessions: HashMap::new(),
        next_session: 1,
        next_request: 1,
        cancelled: false,
    };
    while let Some(message) = agent.read() {
        agent.handle(message);
    }
}

fn send(message: Value) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(format!("{message}\n").as_bytes());
    let _ = stdout.flush();
}

fn respond(id: &Value, result: Value) {
    send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn update(session_id: &str, update: Value) {
    send(
        json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session_id, "update": update } }),
    );
}

fn chunk(session_id: &str, message_id: &str, text: &str) {
    update(
        session_id,
        json!({ "sessionUpdate": "agent_message_chunk", "messageId": message_id, "content": { "type": "text", "text": text } }),
    );
}

fn config_options(model: &str, effort: &str) -> Value {
    json!([
        { "id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model, "options": [
            { "value": "sonnet", "name": "Sonnet", "description": "Fast" },
            { "value": "opus", "name": "Opus" }
        ] },
        { "id": "effort", "name": "Reasoning effort", "category": "thought_level", "type": "select", "currentValue": effort, "options": [
            { "group": "levels", "name": "Levels", "options": [
                { "value": "low", "name": "Low" },
                { "value": "high", "name": "High" }
            ] }
        ] },
        { "id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "ask", "options": [
            { "value": "ask", "name": "Ask" },
            { "value": "yolo", "name": "Bypass permissions" }
        ] },
        { "id": "fast", "name": "Fast", "type": "boolean", "currentValue": false }
    ])
}

impl Agent {
    fn read(&mut self) -> Option<Value> {
        loop {
            let line = self.lines.next()?.ok()?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(log) = &mut self.log {
                // One write per line, so a reader never sees half of one.
                let _ = log.write_all(format!("{message}\n").as_bytes());
            }
            return Some(message);
        }
    }

    /// Sends a request to the client and handles everything else until its response arrives.
    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = format!("agent-{}", self.next_request);
        self.next_request += 1;
        send(
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
        while let Some(message) = self.read() {
            if message.get("method").is_none() && message["id"] == json!(id) {
                return message;
            }
            self.handle(message);
        }
        std::process::exit(0);
    }

    fn handle(&mut self, message: Value) {
        let method = message["method"].as_str().unwrap_or_default();
        let id = &message["id"];
        let params = &message["params"];
        match method {
            "initialize" => respond(
                id,
                json!({ "protocolVersion": 1, "agentCapabilities": { "loadSession": false }, "agentInfo": { "name": "fake", "version": "1.0.0" } }),
            ),
            "session/new" => {
                let session_id = format!("session-{}", self.next_session);
                self.next_session += 1;
                self.sessions.insert(
                    session_id.clone(),
                    ("sonnet".to_string(), "low".to_string()),
                );
                respond(
                    id,
                    json!({ "sessionId": session_id, "configOptions": config_options("sonnet", "low") }),
                );
            }
            "session/set_config_option" => {
                let session_id = params["sessionId"].as_str().unwrap_or_default();
                let value = params["value"].as_str().unwrap_or_default().to_string();
                let Some(settings) = self.sessions.get_mut(session_id) else {
                    send(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": "unknown session" } }),
                    );
                    return;
                };
                match params["configId"].as_str() {
                    Some("model") => settings.0 = value,
                    Some("effort") => settings.1 = value,
                    _ => {}
                }
                respond(
                    id,
                    json!({ "configOptions": config_options(&settings.0, &settings.1) }),
                );
            }
            "session/cancel" => self.cancelled = true,
            "session/prompt" => {
                let id = id.clone();
                let session_id =
                    params["sessionId"].as_str().unwrap_or_default().to_string();
                let text = params["prompt"]
                    .as_array()
                    .and_then(|blocks| {
                        blocks.iter().rev().find(|block| block["type"] == "text")
                    })
                    .and_then(|block| block["text"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let stop_reason = self.prompt(&session_id, &text);
                respond(&id, json!({ "stopReason": stop_reason }));
            }
            _ if !id.is_null() && message.get("method").is_some() => {
                send(
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "method not found" } }),
                );
            }
            _ => {}
        }
    }

    fn prompt(&mut self, session_id: &str, text: &str) -> &'static str {
        self.cancelled = false;
        let word_after = |prefix: &str| {
            text.split_whitespace()
                .find_map(|word| word.strip_prefix(prefix))
                .map(str::to_string)
        };
        if text.contains("permission") {
            let response = self.call(
                "session/request_permission",
                json!({
                    "sessionId": session_id,
                    "toolCall": { "toolCallId": "edit-1", "title": "Edit src/lib.rs", "kind": "edit", "status": "pending" },
                    "options": [
                        { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                        { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
                    ]
                }),
            );
            let outcome = &response["result"]["outcome"];
            if outcome["outcome"] == "selected" {
                let said = format!(
                    "You chose {}.",
                    outcome["optionId"].as_str().unwrap_or_default()
                );
                chunk(session_id, "m1", &said);
                return "end_turn";
            }
            chunk(session_id, "m1", "The permission request was cancelled.");
            return "cancelled";
        }
        if text.contains("slow") {
            chunk(session_id, "m1", "Working on it");
            while !self.cancelled {
                let Some(message) = self.read() else {
                    std::process::exit(0);
                };
                self.handle(message);
            }
            return "cancelled";
        }
        if let Some(path) = word_after("read:") {
            let response = self.call(
                "fs/read_text_file",
                json!({ "sessionId": session_id, "path": path, "line": 2, "limit": 1 }),
            );
            let said = match response["result"]["content"].as_str() {
                Some(content) => format!("Line 2 is: {content}"),
                None => format!(
                    "Read failed: {}",
                    response["error"]["message"].as_str().unwrap_or("?")
                ),
            };
            chunk(session_id, "m1", &said);
            return "end_turn";
        }
        if let Some(path) = word_after("write:") {
            let response = self.call(
                "fs/write_text_file",
                json!({ "sessionId": session_id, "path": path, "content": "written by agent\n" }),
            );
            let said = match response.get("error") {
                Some(error) => format!(
                    "Write failed: {}",
                    error["message"].as_str().unwrap_or("?")
                ),
                None => "Written.".to_string(),
            };
            chunk(session_id, "m1", &said);
            return "end_turn";
        }
        if text.contains("settings") {
            let (model, effort) =
                self.sessions.get(session_id).cloned().unwrap_or_default();
            chunk(session_id, "m1", &format!("model={model} effort={effort}"));
            return "end_turn";
        }
        if text.contains("refuse") {
            return "refusal";
        }
        if text.contains("crash") {
            chunk(session_id, "m1", "About to fail");
            eprintln!("fake agent: crashing on purpose");
            std::process::exit(3);
        }
        chunk(session_id, "m1", "Let me look at   the code.");
        update(
            session_id,
            json!({ "sessionUpdate": "tool_call", "toolCallId": "read-1", "title": "Read src/lib.rs", "kind": "read", "status": "pending" }),
        );
        update(
            session_id,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "read-1", "status": "completed" }),
        );
        update(
            session_id,
            json!({ "sessionUpdate": "tool_call_update", "toolCallId": "search-1", "title": "Search for add" }),
        );
        chunk(session_id, "m2", "It adds ");
        chunk(session_id, "m2", "two numbers.");
        "end_turn"
    }
}
