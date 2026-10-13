//! The MCP endpoint: Streamable HTTP at `http://127.0.0.1:<port>/mcp?root=<repo>`, local connections only, on a
//! tokio runtime of its own (so the app needn't run tokio), recorded in `~/.co-review/server.json`.

use std::borrow::Cow;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use axum::extract::Request;
use axum::http;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CustomNotification,
    Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, ServerNotification,
    Tool,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::prompts::SERVER_INSTRUCTIONS;
use crate::reviewer::{AskArguments, AwaitArguments, ChannelSink, CoReviewer, Session, ToolOutput};
use crate::services::Services;

/// Where and how the server runs.
pub struct ServerOptions {
    /// The port on 127.0.0.1; 0 picks a free one.
    pub port: u16,
    /// The repository the app was started for: the default `root` of every session.
    pub default_root: Option<PathBuf>,
    /// Where the running instance is recorded (`~/.co-review/server.json`); `None` records nothing.
    pub registry: Option<PathBuf>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            port: 0,
            default_root: None,
            registry: Some(review_store::co_review_home().join("server.json")),
        }
    }
}

/// The running server. Dropping it stops the server.
pub struct ServerHandle {
    url: String,
    reviewer: Arc<CoReviewer>,
    cancel: CancellationToken,
    thread: Option<JoinHandle<()>>,
    registry: Option<PathBuf>,
}

impl ServerHandle {
    /// `http://127.0.0.1:<port>`; agents connect to `<url>/mcp?root=<repo>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The tools' shared state, for the app: answering decisions, presence, closed and opened windows.
    pub fn reviewer(&self) -> &Arc<CoReviewer> {
        &self.reviewer
    }

    /// Stops the server and removes its record (only its own: another instance may have replaced it).
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("[co-review] the MCP server thread panicked");
        }
        if let Some(registry) = self.registry.take() {
            let ours = std::fs::read_to_string(&registry)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|record| {
                    record["pid"].as_u64() == Some(u64::from(std::process::id()))
                });
            if ours {
                std::fs::remove_file(&registry).ok();
            }
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Starts the MCP server on its own thread and tokio runtime; returns once it listens.
pub fn start(options: ServerOptions, services: Services) -> Result<ServerHandle> {
    let reviewer = Arc::new(CoReviewer::new(services, options.default_root.as_deref()));
    let cancel = CancellationToken::new();
    let (bound_sender, bound) = std::sync::mpsc::channel::<Result<SocketAddr>>();
    let thread = {
        let (reviewer, cancel) = (Arc::clone(&reviewer), cancel.clone());
        std::thread::Builder::new()
            .name("co-review-mcp".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        bound_sender.send(Err(error.into())).ok();
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener =
                        match tokio::net::TcpListener::bind(("127.0.0.1", options.port)).await {
                            Ok(listener) => listener,
                            Err(error) => {
                                bound_sender
                                    .send(Err(anyhow!(error).context("binding the MCP port")))
                                    .ok();
                                return;
                            }
                        };
                    bound_sender
                        .send(listener.local_addr().map_err(anyhow::Error::from))
                        .ok();
                    let router = router(reviewer, cancel.clone());
                    let served = axum::serve(listener, router)
                        .with_graceful_shutdown(async move { cancel.cancelled_owned().await })
                        .await;
                    if let Err(error) = served {
                        log::error!("[co-review] the MCP server stopped: {error}");
                    }
                });
                runtime.shutdown_timeout(std::time::Duration::from_secs(1));
            })?
    };
    let address = bound.recv().context("the MCP server thread stopped")??;
    let url = format!("http://127.0.0.1:{}", address.port());
    if let Some(registry) = &options.registry {
        // Records where this instance listens, so `co-review mcp` can find the running app.
        let record =
            serde_json::to_string_pretty(&json!({ "url": url, "pid": std::process::id() }))?;
        if let Some(parent) = registry.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(registry, record)?;
    }
    Ok(ServerHandle {
        url,
        reviewer,
        cancel,
        thread: Some(thread),
        registry: options.registry,
    })
}

/// The longest `timeoutSec` a waiting tool (await_reviewer, ask_reviewer) takes.
const MAX_WAIT_SECONDS: u64 = 3600;

/// rmcp ends a session after 5 minutes without a message either way, but a session waiting for the reviewer sends
/// none: it must outlive the longest wait (and then some), or an agent's wait fails after 5 minutes.
fn session_manager() -> LocalSessionManager {
    let mut sessions = LocalSessionManager::default();
    sessions.session_config.keep_alive = Some(Duration::from_secs(MAX_WAIT_SECONDS + 300));
    sessions
}

fn router(reviewer: Arc<CoReviewer>, cancel: CancellationToken) -> axum::Router {
    let mut config = StreamableHttpServerConfig::default();
    config.cancellation_token = cancel.child_token();
    // The `local_only` layer checks Host and Origin with the Theia app's rules.
    config.allowed_hosts.clear();
    let service: StreamableHttpService<McpHandler, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(McpHandler {
                    session: Arc::new(Session::new(Arc::clone(&reviewer))),
                })
            },
            Arc::new(session_manager()),
            config,
        );
    axum::Router::new()
        .route_service("/mcp", service)
        .layer(axum::middleware::from_fn(local_only))
}

/// Only local clients: the endpoint is unauthenticated. Web pages cannot drive it either: browsers send an Origin,
/// agents do not.
async fn local_only(request: Request, next: Next) -> Response {
    if !is_local(request.headers()) {
        return (
            http::StatusCode::FORBIDDEN,
            "Co-Review MCP only accepts local connections",
        )
            .into_response();
    }
    next.run(request).await
}

fn is_local(headers: &http::HeaderMap) -> bool {
    const LOCAL: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "::1"];
    let header = |name: http::HeaderName| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap_or("invalid").to_string())
    };
    let host = strip_port(&header(http::header::HOST).unwrap_or_default());
    let origin = header(http::header::ORIGIN).map(|origin| {
        origin
            .parse::<http::Uri>()
            .ok()
            .and_then(|uri| uri.host().map(str::to_string))
            .unwrap_or_else(|| "invalid".to_string())
    });
    std::iter::once(host.as_str())
        .chain(origin.as_deref())
        .all(|name| LOCAL.contains(&name))
}

/// `host:port` → `host` (as the TypeScript's `/:\d+$/`).
fn strip_port(host: &str) -> String {
    match host.rsplit_once(':') {
        Some((name, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            name.to_string()
        }
        _ => host.to_string(),
    }
}

/// The `root` query parameter, decoded as Express decodes it.
fn root_parameter(query: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (decode_component(key) == "root").then(|| decode_component(value))
    })
}

fn decode_component(text: &str) -> String {
    review_store::percent_decode(&text.replace('+', " "))
}

/// One MCP session's handler; rmcp makes one per session.
#[derive(Clone)]
struct McpHandler {
    session: Arc<Session>,
}

/// Claude Code channel notifications, sent in order over the session's stream.
struct PeerChannel {
    sender: tokio::sync::mpsc::UnboundedSender<(String, Map<String, Value>)>,
}

impl PeerChannel {
    fn new(peer: Peer<RoleServer>) -> Self {
        let (sender, mut receiver) =
            tokio::sync::mpsc::unbounded_channel::<(String, Map<String, Value>)>();
        tokio::spawn(async move {
            while let Some((content, meta)) = receiver.recv().await {
                let notification = CustomNotification::new(
                    "notifications/claude/channel",
                    Some(json!({ "content": content, "meta": meta })),
                );
                if let Err(error) = peer
                    .send_notification(ServerNotification::CustomNotification(notification))
                    .await
                {
                    log::debug!("[co-review] channel push failed: {error}");
                }
            }
        });
        Self { sender }
    }
}

impl ChannelSink for PeerChannel {
    fn notify(&self, content: String, meta: Map<String, Value>) {
        self.sender.send((content, meta)).ok();
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        let mut capabilities = ServerCapabilities::default();
        // Claude Code channels: review events are pushed into the session.
        let mut experimental = rmcp::model::ExperimentalCapabilities::new();
        experimental.insert("claude/channel".to_string(), Map::new());
        capabilities.experimental = Some(experimental);
        let mut tools = rmcp::model::ToolsCapability::default();
        tools.list_changed = Some(true);
        capabilities.tools = Some(tools);
        let mut info = InitializeResult::new(capabilities).with_instructions(SERVER_INSTRUCTIONS);
        info.server_info = Implementation::new("co-review", env!("CARGO_PKG_VERSION"));
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        // Versions with sessions: delivery is once per MCP session.
        Cow::Borrowed(ProtocolVersion::known_up_to(
            &ProtocolVersion::LATEST_WITH_INITIALIZE,
        ))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let root = context
            .extensions
            .get::<http::request::Parts>()
            .and_then(|parts| parts.uri.query())
            .and_then(root_parameter);
        self.session
            .initialized(&request.client_info.name, root.as_deref());
        self.session
            .set_channel_sink(Arc::new(PeerChannel::new(context.peer.clone())));
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let output = call(
            &self.session,
            &request.name,
            request.arguments.unwrap_or_default(),
            context.ct,
        )
        .await;
        let content = vec![ContentBlock::text(output.text)];
        let result = if output.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        };
        Ok(result.into())
    }
}

fn tools() -> Vec<Tool> {
    crate::tools::definitions()
        .into_iter()
        .map(|(name, description, schema)| {
            let schema = match schema {
                Value::Object(schema) => schema,
                _ => Map::new(),
            };
            Tool::new(name, description, Arc::new(schema))
        })
        .collect()
}

/// Runs a tool for a session.
pub async fn call(
    session: &Arc<Session>,
    name: &str,
    arguments: Map<String, Value>,
    cancel: CancellationToken,
) -> ToolOutput {
    let reviewer = Arc::clone(session.reviewer());
    let run = async {
        Ok(match name {
            "open_review" => reviewer.open_review(session, parse(name, arguments)?).await,
            "await_reviewer" => {
                let arguments: AwaitArguments = parse(name, arguments)?;
                check_timeout(name, arguments.timeout_sec)?;
                reviewer.await_reviewer(session, arguments, cancel).await
            }
            "reply" => reviewer.reply(session, parse(name, arguments)?),
            "add_findings" => reviewer.add_findings(session, parse(name, arguments)?),
            "ask_reviewer" => {
                let arguments: AskArguments = parse(name, arguments)?;
                check_timeout(name, arguments.timeout_sec)?;
                if arguments.options.is_empty() || arguments.options.len() > 6 {
                    return Err(invalid(name, "options: between 1 and 6 options"));
                }
                reviewer.ask_reviewer(session, arguments, cancel).await
            }
            "post_review_to_github" => {
                reviewer
                    .post_review_to_github(session, parse(name, arguments)?)
                    .await
            }
            "repo_map" => reviewer.repo_map(session, parse(name, arguments)?).await,
            "get_review" => reviewer.get_review(session, parse(name, arguments)?).await,
            _ => ToolOutput::fail(format!("MCP error -32602: Tool {name} not found")),
        })
    };
    run.await.unwrap_or_else(|error: ToolOutput| error)
}

fn parse<Arguments: DeserializeOwned>(
    tool: &str,
    arguments: Map<String, Value>,
) -> Result<Arguments, ToolOutput> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|error| invalid(tool, &error.to_string()))
}

fn check_timeout(tool: &str, timeout: Option<u64>) -> Result<(), ToolOutput> {
    match timeout {
        Some(seconds) if !(1..=MAX_WAIT_SECONDS).contains(&seconds) => Err(invalid(
            tool,
            &format!("timeoutSec: between 1 and {MAX_WAIT_SECONDS}"),
        )),
        _ => Ok(()),
    }
}

fn invalid(tool: &str, message: &str) -> ToolOutput {
    ToolOutput::fail(format!(
        "MCP error -32602: Input validation error: Invalid arguments for tool {tool}: {message}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_parameter_is_decoded() {
        assert_eq!(
            root_parameter("root=%2Ftmp%2Fmy%20repo&x=1").as_deref(),
            Some("/tmp/my repo")
        );
        assert_eq!(root_parameter("x=1"), None);
        assert_eq!(root_parameter("root=100%").as_deref(), Some("100%"));
    }

    #[test]
    fn a_session_outlives_the_longest_wait_for_the_reviewer() {
        let keep_alive = session_manager().session_config.keep_alive;
        assert!(
            keep_alive.is_some_and(|keep_alive| keep_alive > Duration::from_secs(MAX_WAIT_SECONDS)),
            "{keep_alive:?}"
        );
        assert!(check_timeout("await_reviewer", Some(MAX_WAIT_SECONDS)).is_ok());
        assert!(check_timeout("await_reviewer", Some(MAX_WAIT_SECONDS + 1)).is_err());
    }

    #[test]
    fn ports_are_stripped() {
        assert_eq!(strip_port("127.0.0.1:3000"), "127.0.0.1");
        assert_eq!(strip_port("[::1]:3000"), "[::1]");
        assert_eq!(strip_port("localhost"), "localhost");
    }
}
