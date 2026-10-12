//! One running ACP agent: its process, the ACP connection, its sessions and the client side of the protocol
//! (session updates, permission requests, file reads and writes limited to the repository; no terminal).

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ClientCapabilities, ContentBlock, FileSystemCapabilities,
    Implementation, InitializeRequest, NewSessionRequest, PromptRequest,
    PromptResponse, ReadTextFileRequest, ReadTextFileResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionConfigOption, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, WriteTextFileRequest,
    WriteTextFileResponse,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use anyhow::{Context as _, anyhow};
use parking_lot::{Condvar, Mutex};
use review_model::{AgentConfig, AgentState, PermissionOption, PermissionRequest};

use crate::events::{ConnectionState, Core};
use crate::presets::command_line;
use crate::settings::current_value;
use crate::turn::Turn;

/// How long a permission request waits for the reviewer: until they answer, or the turn is stopped.
const PERMISSION_WAIT: Duration = Duration::from_secs(60 * 60 * 24 * 7);
/// How long an agent has to exit by itself once its stdin is closed.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// Stderr chunks kept for error messages.
const STDERR_CHUNKS: usize = 50;

enum Spare {
    Empty,
    Opening,
    Ready(String),
    Failed(String),
}

pub(crate) struct Connection {
    pub review_id: String,
    pub config: Mutex<AgentConfig>,
    pub root: PathBuf,
    core: Arc<Core>,
    child: Mutex<Option<Child>>,
    acp: OnceLock<Result<ConnectionTo<Agent>, String>>,
    ready: smol::channel::Receiver<()>,
    shutdown: smol::channel::Sender<()>,
    /// threadId -> ACP session id; one session per thread keeps follow-ups in context.
    pub sessions: Mutex<HashMap<String, String>>,
    /// sessionId -> the running turn.
    turns: Mutex<HashMap<String, Arc<Mutex<Turn>>>>,
    /// sessionId -> the session's config options, as the agent last reported them.
    options: Mutex<HashMap<String, Vec<SessionConfigOption>>>,
    /// A session opened ahead of the next question (and to learn the agent's settings), so a new thread does not
    /// wait for `session/new`.
    spare: Mutex<Spare>,
    spare_changed: Condvar,
    stderr: Mutex<VecDeque<String>>,
    /// Set once the process has exited: its exit code.
    exit: Mutex<Option<Option<i32>>>,
    exit_changed: Condvar,
}

impl Connection {
    /// Starts the agent in `root` and initializes ACP in the background. `on_exit` runs when the process ends.
    pub fn start(
        core: Arc<Core>,
        review_id: &str,
        config: AgentConfig,
        root: PathBuf,
        on_exit: impl FnOnce(&Arc<Connection>) + Send + 'static,
    ) -> anyhow::Result<Arc<Self>> {
        let command = config.command.clone().unwrap_or_default();
        let mut child = Command::new(&command)
            .args(config.args.clone().unwrap_or_default())
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                anyhow!(
                    "Could not start {} (`{}`): {error}",
                    config.name,
                    command_line(&config)
                )
            })?;
        let stdin = child.stdin.take().context("agent stdin")?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let stderr = child.stderr.take().context("agent stderr")?;
        let (ready_sender, ready) = smol::channel::bounded::<()>(1);
        let (shutdown, shutdown_receiver) = smol::channel::bounded::<()>(1);
        let connection = Arc::new(Self {
            review_id: review_id.to_string(),
            config: Mutex::new(config),
            root,
            core,
            child: Mutex::new(Some(child)),
            acp: OnceLock::new(),
            ready,
            shutdown,
            sessions: Mutex::default(),
            turns: Mutex::default(),
            options: Mutex::default(),
            spare: Mutex::new(Spare::Empty),
            spare_changed: Condvar::new(),
            stderr: Mutex::default(),
            exit: Mutex::new(None),
            exit_changed: Condvar::new(),
        });
        connection
            .core
            .connection_changed(review_id, ConnectionState::Starting);

        let reader = Arc::clone(&connection);
        std::thread::Builder::new()
            .name("co-review-agent-stderr".to_string())
            .spawn(move || reader.read_stderr(stderr))?;

        let runner = Arc::clone(&connection);
        std::thread::Builder::new()
            .name("co-review-agent".to_string())
            .spawn(move || {
                let transport = ByteStreams::new(
                    smol::Unblock::new(stdin),
                    smol::Unblock::new(stdout),
                );
                let result = smol::block_on(runner.run(
                    transport,
                    ready_sender,
                    shutdown_receiver,
                ));
                if let Err(error) = &result {
                    if runner.acp.set(Err(error.message.clone())).is_ok() {
                        let message = runner.describe(&error.message);
                        runner.core.connection_changed(
                            &runner.review_id,
                            ConnectionState::Failed { message },
                        );
                    } else {
                        log::warn!("agent connection ended: {}", error.message);
                    }
                }
                runner.reap();
                on_exit(&runner);
            })?;
        Ok(connection)
    }

    async fn run(
        self: &Arc<Self>,
        transport: impl agent_client_protocol::ConnectTo<Client> + 'static,
        ready: smol::channel::Sender<()>,
        shutdown: smol::channel::Receiver<()>,
    ) -> agent_client_protocol::Result<()> {
        let updates = Arc::clone(self);
        let permissions = Arc::clone(self);
        let reads = Arc::clone(self);
        let writes = Arc::clone(self);
        Client
            .builder()
            .name("co-review")
            .on_receive_notification(
                async move |notification: SessionNotification, _connection| {
                    updates.session_update(notification);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: RequestPermissionRequest,
                            responder,
                            connection: ConnectionTo<Agent>| {
                    let this = Arc::clone(&permissions);
                    // The reviewer may take a while: wait outside the dispatch loop.
                    connection.spawn(async move {
                        let response = this.request_permission(request).await;
                        if let Err(error) = responder.respond(response) {
                            log::warn!(
                                "permission answer not sent: {}",
                                error.message
                            );
                        }
                        Ok(())
                    })
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: ReadTextFileRequest, responder, _connection| {
                    responder.respond_with_result(reads.read_text_file(&request))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: WriteTextFileRequest,
                            responder,
                            _connection| {
                    responder.respond_with_result(writes.write_text_file(&request))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async |connection: ConnectionTo<Agent>| {
                let capabilities = ClientCapabilities::new()
                    .fs(FileSystemCapabilities::new()
                        .read_text_file(true)
                        .write_text_file(true))
                    .terminal(false);
                connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(capabilities)
                            .client_info(Implementation::new("co-review", "0.1.0")),
                    )
                    .block_task()
                    .await?;
                if self.acp.set(Ok(connection.clone())).is_ok() {
                    self.core
                        .connection_changed(&self.review_id, ConnectionState::Ready);
                }
                drop(ready);
                smol::future::or(
                    async {
                        shutdown.recv().await.ok();
                    },
                    connection.incoming_closed(),
                )
                .await;
                Ok(())
            })
            .await
    }

    /// The error message for a connection that failed, with the end of the agent's stderr.
    fn describe(&self, message: &str) -> String {
        let details = self.stderr_tail();
        if details.is_empty() {
            message.to_string()
        } else {
            format!("{message}\n\n{details}")
        }
    }

    /// The last few stderr chunks, trimmed, at most 1,500 characters.
    pub fn stderr_tail(&self) -> String {
        let stderr = self.stderr.lock();
        let skip = stderr.len().saturating_sub(5);
        let tail: String = stderr.iter().skip(skip).cloned().collect();
        let tail = tail.trim();
        let count = tail.chars().count();
        tail.chars().skip(count.saturating_sub(1500)).collect()
    }

    fn read_stderr(&self, mut stderr: impl Read) {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stderr.read(&mut buffer) {
            let Some(bytes) = buffer.get(..read).filter(|bytes| !bytes.is_empty())
            else {
                break;
            };
            let text = String::from_utf8_lossy(bytes).into_owned();
            log::debug!("agent stderr: {}", text.trim_end());
            let mut chunks = self.stderr.lock();
            chunks.push_back(text);
            while chunks.len() > STDERR_CHUNKS {
                chunks.pop_front();
            }
        }
    }

    /// Ends the process (if it is still running) and records how it exited.
    fn reap(&self) {
        let child = self.child.lock().take();
        let code = child.and_then(|mut child| {
            // Agents exit when their stdin closes; give it a moment before killing.
            let deadline = Instant::now() + EXIT_GRACE;
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(child.try_wait(), Ok(None)) {
                child.kill().ok();
            }
            child.wait().ok().map(|status| status.code())
        });
        let code = code.flatten();
        *self.exit.lock() = Some(code);
        self.exit_changed.notify_all();
        // A turn still running learns the process is gone.
        for turn in self.turns.lock().values() {
            let mut turn = turn.lock();
            turn.body.push_str(&format!(
                "\n\n**The agent process exited{}.**",
                code.map(|code| format!(" (code {code})"))
                    .unwrap_or_default()
            ));
            turn.dirty = true;
        }
        self.core
            .connection_changed(&self.review_id, ConnectionState::Exited { code });
    }

    pub fn has_exited(&self) -> bool {
        self.exit.lock().is_some()
    }

    /// Waits up to `timeout` for the process to end; whether it has.
    pub fn wait_for_exit(&self, timeout: Duration) -> bool {
        let mut exit = self.exit.lock();
        if exit.is_none() {
            self.exit_changed.wait_for(&mut exit, timeout);
        }
        exit.is_some()
    }

    /// Stops the agent: closes the connection and kills the process.
    pub fn stop(self: &Arc<Self>) {
        self.shutdown.close();
        // Closing the connection closes the agent's stdin, and it exits. One that is still starting, or doesn't
        // exit, is killed.
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("co-review-agent-stop".to_string())
            .spawn(move || {
                if this.acp.get().is_some() {
                    this.wait_for_exit(EXIT_GRACE * 2);
                }
                if !this.has_exited()
                    && let Some(child) = this.child.lock().as_mut()
                {
                    child.kill().ok();
                }
            });
        if spawned.is_err()
            && let Some(child) = self.child.lock().as_mut()
        {
            child.kill().ok();
        }
    }

    /// The ACP connection, once initialized.
    pub async fn acp(&self) -> anyhow::Result<ConnectionTo<Agent>> {
        self.ready.recv().await.ok();
        match self.acp.get() {
            Some(Ok(connection)) => Ok(connection.clone()),
            Some(Err(message)) => Err(anyhow!("{message}")),
            None => Err(anyhow!("The agent connection closed")),
        }
    }

    async fn new_session(&self) -> anyhow::Result<String> {
        let acp = self.acp().await?;
        let response = acp
            .send_request(NewSessionRequest::new(self.root.clone()))
            .block_task()
            .await
            .map_err(|error| anyhow!("{}", error.message))?;
        let session_id = response.session_id.to_string();
        self.options.lock().insert(
            session_id.clone(),
            response.config_options.unwrap_or_default(),
        );
        Ok(session_id)
    }

    /// Opens the spare session in the background, if there is none.
    pub fn open_spare(self: &Arc<Self>) {
        {
            let mut spare = self.spare.lock();
            if matches!(*spare, Spare::Opening | Spare::Ready(_)) {
                return;
            }
            *spare = Spare::Opening;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("co-review-agent-session".to_string())
            .spawn(move || {
                let result = smol::block_on(this.new_session());
                *this.spare.lock() = match result {
                    Ok(session_id) => Spare::Ready(session_id),
                    Err(error) => Spare::Failed(format!("{error:#}")),
                };
                this.spare_changed.notify_all();
            });
        if let Err(error) = spawned {
            *self.spare.lock() = Spare::Failed(error.to_string());
            self.spare_changed.notify_all();
        }
    }

    /// A session for a new thread: the spare one (opening the next in the background), else a new one.
    pub fn take_session(self: &Arc<Self>) -> anyhow::Result<String> {
        let mut spare = self.spare.lock();
        loop {
            match std::mem::replace(&mut *spare, Spare::Empty) {
                Spare::Ready(session_id) => {
                    drop(spare);
                    self.open_spare();
                    return Ok(session_id);
                }
                Spare::Opening => {
                    *spare = Spare::Opening;
                    self.spare_changed.wait(&mut spare);
                }
                Spare::Empty | Spare::Failed(_) => {
                    drop(spare);
                    let session_id = smol::block_on(self.new_session())?;
                    self.open_spare();
                    return Ok(session_id);
                }
            }
        }
    }

    /// The config options of the spare session (opened now if there is none).
    pub fn spare_options(
        self: &Arc<Self>,
    ) -> anyhow::Result<Vec<SessionConfigOption>> {
        self.open_spare();
        let mut spare = self.spare.lock();
        loop {
            match &*spare {
                Spare::Ready(session_id) => {
                    return Ok(self
                        .options
                        .lock()
                        .get(session_id)
                        .cloned()
                        .unwrap_or_default());
                }
                Spare::Opening => self.spare_changed.wait(&mut spare),
                Spare::Failed(message) => {
                    let message = message.clone();
                    // The next caller tries again.
                    *spare = Spare::Empty;
                    return Err(anyhow!("{message}"));
                }
                Spare::Empty => {
                    drop(spare);
                    self.open_spare();
                    spare = self.spare.lock();
                }
            }
        }
    }

    /// Applies the review's chosen settings (model, effort) to a session, where they differ.
    pub async fn apply_settings(&self, session_id: &str) -> anyhow::Result<()> {
        let settings = self.config.lock().settings.clone().unwrap_or_default();
        let mut settings: Vec<(String, String)> = settings.into_iter().collect();
        settings.sort();
        for (id, value) in settings {
            let current = self
                .options
                .lock()
                .get(session_id)
                .and_then(|options| current_value(options, &id));
            // Only the session's own options; a mode (`__mode`, stored by older versions) is not applied.
            if current.is_none_or(|current| current == value) {
                continue;
            }
            let acp = self.acp().await?;
            let response = acp
                .send_request(SetSessionConfigOptionRequest::new(
                    session_id.to_string(),
                    id.clone(),
                    value.as_str(),
                ))
                .block_task()
                .await
                .map_err(|error| anyhow!("{}", error.message))?;
            self.options
                .lock()
                .insert(session_id.to_string(), response.config_options);
        }
        Ok(())
    }

    pub fn begin_turn(&self, session_id: &str, turn: Arc<Mutex<Turn>>) {
        self.turns.lock().insert(session_id.to_string(), turn);
    }

    pub fn end_turn(&self, session_id: &str) {
        self.turns.lock().remove(session_id);
    }

    pub fn turn(&self, session_id: &str) -> Option<Arc<Mutex<Turn>>> {
        self.turns.lock().get(session_id).cloned()
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        prompt: Vec<ContentBlock>,
    ) -> anyhow::Result<PromptResponse> {
        let acp = self.acp().await?;
        acp.send_request(PromptRequest::new(session_id.to_string(), prompt))
            .block_task()
            .await
            .map_err(|error| anyhow!("{}", error.message))
    }

    pub fn cancel(&self, session_id: &str) -> anyhow::Result<()> {
        let Some(Ok(acp)) = self.acp.get() else {
            return Ok(());
        };
        acp.send_notification(CancelNotification::new(session_id.to_string()))
            .map_err(|error| anyhow!("{}", error.message))
    }

    fn session_update(&self, notification: SessionNotification) {
        let session_id = notification.session_id.to_string();
        if let SessionUpdate::ConfigOptionUpdate(update) = &notification.update {
            self.options
                .lock()
                .insert(session_id.clone(), update.config_options.clone());
        }
        if let Some(turn) = self.turn(&session_id) {
            turn.lock().apply(&notification.update);
        }
    }

    async fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> RequestPermissionResponse {
        let cancelled =
            || RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
        let Some(turn) = self.turn(&request.session_id.to_string()) else {
            return cancelled();
        };
        let permission = PermissionRequest {
            id: uuid::Uuid::new_v4().to_string(),
            title: request
                .tool_call
                .fields
                .title
                .clone()
                .unwrap_or_else(|| "The agent wants to run a tool".to_string()),
            options: request
                .options
                .iter()
                .map(|option| PermissionOption {
                    id: option.option_id.to_string(),
                    name: option.name.clone(),
                    kind: serde_json::to_value(option.kind)
                        .ok()
                        .and_then(|kind| kind.as_str().map(str::to_string))
                        .unwrap_or_default(),
                })
                .collect(),
            outcome: None,
        };
        let request_id = permission.id.clone();
        let (review_id, thread_id) = {
            let mut turn = turn.lock();
            turn.permission = Some(permission);
            (turn.review_id.clone(), turn.thread_id.clone())
        };
        let answer = self.core.decisions.wait(&request_id, PERMISSION_WAIT);
        let shown = async {
            self.core.flush(&turn);
            self.core.set_agent_state(
                &review_id,
                &thread_id,
                AgentState::WaitingForHuman,
            );
        };
        // The wait registers on its first poll, before the reviewer can see the buttons.
        let (option_id, ()) = smol::future::zip(answer, shown).await;
        if let Some(permission) = &mut turn.lock().permission {
            permission.outcome =
                Some(option_id.clone().unwrap_or_else(|| "cancelled".to_string()));
        }
        self.core.flush(&turn);
        self.core
            .set_agent_state(&review_id, &thread_id, AgentState::Working);
        match option_id {
            Some(option_id) => {
                RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(option_id),
                ))
            }
            None => cancelled(),
        }
    }

    /// The pending permission request of the session's running turn, if any.
    pub fn pending_permission(&self, session_id: &str) -> Option<String> {
        let turn = self.turn(session_id)?;
        let turn = turn.lock();
        turn.permission
            .as_ref()
            .filter(|permission| permission.outcome.is_none())
            .map(|permission| permission.id.clone())
    }

    fn read_text_file(
        &self,
        request: &ReadTextFileRequest,
    ) -> agent_client_protocol::Result<ReadTextFileResponse> {
        let file = inside_root(&self.root, &request.path)?;
        let content = std::fs::read_to_string(&file).map_err(|error| {
            agent_client_protocol::Error::internal_error()
                .data(serde_json::json!(error.to_string()))
        })?;
        if request.line.is_none() && request.limit.is_none() {
            return Ok(ReadTextFileResponse::new(content));
        }
        let start = request.line.unwrap_or(1).saturating_sub(1) as usize;
        let lines = content.split('\n').skip(start);
        let selected: Vec<&str> = match request.limit.filter(|limit| *limit > 0) {
            Some(limit) => lines.take(limit as usize).collect(),
            None => lines.collect(),
        };
        Ok(ReadTextFileResponse::new(selected.join("\n")))
    }

    fn write_text_file(
        &self,
        request: &WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        let file = inside_root(&self.root, &request.path)?;
        std::fs::write(&file, &request.content).map_err(|error| {
            agent_client_protocol::Error::internal_error()
                .data(serde_json::json!(error.to_string()))
        })?;
        Ok(WriteTextFileResponse::new())
    }
}

/// `file` (absolute, or relative to `root`) when it is inside `root`, compared without following links.
pub(crate) fn inside_root(
    root: &Path,
    file: &Path,
) -> agent_client_protocol::Result<PathBuf> {
    let joined = root.join(file);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    if resolved.starts_with(root) {
        Ok(resolved)
    } else {
        Err(agent_client_protocol::Error::new(
            -32603,
            format!(
                "Access outside the repository is not allowed: {}",
                file.display()
            ),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn file_access_stays_in_the_repository() {
        let root = Path::new("/work/repo");
        assert_eq!(
            inside_root(root, Path::new("src/lib.rs")).unwrap(),
            PathBuf::from("/work/repo/src/lib.rs")
        );
        assert_eq!(
            inside_root(root, Path::new("/work/repo/./a/../b.rs")).unwrap(),
            PathBuf::from("/work/repo/b.rs")
        );
        assert!(inside_root(root, Path::new("../other/secret")).is_err());
        assert!(inside_root(root, Path::new("/work/repo-other/x")).is_err());
        assert!(inside_root(root, Path::new("/etc/passwd")).is_err());
        let error = inside_root(root, Path::new("src/../../x")).unwrap_err();
        assert_eq!(
            error.message,
            "Access outside the repository is not allowed: src/../../x"
        );
    }
}
