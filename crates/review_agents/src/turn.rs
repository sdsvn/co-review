//! One prompt turn: the agent message being written into a thread, built from the agent's session updates.

use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, ToolCallStatus, ToolKind,
};
use review_model::{AgentActivity, PermissionRequest, ToolStatus};

#[derive(Debug, Clone)]
pub(crate) struct Turn {
    pub review_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub body: String,
    pub activity: Vec<AgentActivity>,
    pub permission: Option<PermissionRequest>,
    /// The agent message being streamed; a turn can hold several ("Let me look…", then the answer).
    pub agent_message_id: Option<String>,
    /// Changed since the thread was last written.
    pub dirty: bool,
}

impl Turn {
    pub fn new(review_id: &str, thread_id: &str, message_id: &str) -> Self {
        Self {
            review_id: review_id.to_string(),
            thread_id: thread_id.to_string(),
            message_id: message_id.to_string(),
            body: String::new(),
            activity: Vec::new(),
            permission: None,
            agent_message_id: None,
            dirty: false,
        }
    }

    /// Applies a `session/update`. Returns whether the thread message changed.
    pub fn apply(&mut self, update: &SessionUpdate) -> bool {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                let ContentBlock::Text(text) = &chunk.content else {
                    return false;
                };
                // A new message in the same turn: the thread shows the last one as the answer, and earlier ones
                // ("Let me check…", a first draft) become steps, so answers stay short.
                let id = chunk.message_id.as_ref().map(|id| id.to_string());
                if let (Some(id), Some(previous)) = (&id, &self.agent_message_id)
                    && id != previous
                    && !self.body.trim().is_empty()
                {
                    let text =
                        self.body.split_whitespace().collect::<Vec<_>>().join(" ");
                    let title = if text.chars().count() > 120 {
                        format!("{}…", text.chars().take(117).collect::<String>())
                    } else {
                        text
                    };
                    self.activity.push(AgentActivity {
                        id: format!("message:{previous}"),
                        title,
                        kind: Some("think".to_string()),
                        status: ToolStatus::Completed,
                    });
                    self.body.clear();
                }
                if id.is_some() {
                    self.agent_message_id = id;
                }
                self.body.push_str(&text.text);
            }
            SessionUpdate::ToolCall(call) => self.activity.push(AgentActivity {
                id: call.tool_call_id.to_string(),
                title: call.title.clone(),
                kind: Some(kind_name(&call.kind)),
                status: tool_status(call.status),
            }),
            SessionUpdate::ToolCallUpdate(update) => {
                let fields = &update.fields;
                let id = update.tool_call_id.to_string();
                if let Some(activity) =
                    self.activity.iter_mut().find(|activity| activity.id == id)
                {
                    if let Some(title) = &fields.title {
                        activity.title = title.clone();
                    }
                    if let Some(status) = fields.status {
                        activity.status = tool_status(status);
                    }
                    if let Some(kind) = &fields.kind {
                        activity.kind = Some(kind_name(kind));
                    }
                } else {
                    self.activity.push(AgentActivity {
                        id,
                        title: fields
                            .title
                            .clone()
                            .unwrap_or_else(|| "Tool call".to_string()),
                        kind: fields.kind.as_ref().map(kind_name),
                        status: fields
                            .status
                            .map_or(ToolStatus::InProgress, tool_status),
                    });
                }
            }
            _ => return false,
        }
        self.dirty = true;
        true
    }
}

fn kind_name(kind: &ToolKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "other".to_string())
}

fn tool_status(status: ToolCallStatus) -> ToolStatus {
    match status {
        ToolCallStatus::Pending => ToolStatus::Pending,
        ToolCallStatus::InProgress => ToolStatus::InProgress,
        ToolCallStatus::Completed => ToolStatus::Completed,
        ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::InProgress,
    }
}
