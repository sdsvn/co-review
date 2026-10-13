//! What the agent is asked: the thread as context in a new session, only the new messages after that.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_client_protocol::schema::v1::{ContentBlock, ResourceLink, TextContent};
use review_model::{
    LocationKind, MessageStatus, ParticipantKind, Review, ReviewThread,
};
use review_store::{file_uri_to_path, path_to_file_uri};

/// How long a first question waits for the repository map before it is sent without one.
pub const MAP_WAIT: Duration = Duration::from_secs(2);
/// Budget for the map in the first prompt; the agent reads files for anything beyond it.
pub const MAP_CHARS: usize = 8000;

/// The repository root of a review, as a path.
pub fn review_root(review: &Review) -> PathBuf {
    PathBuf::from(
        file_uri_to_path(&review.workspace_root)
            .unwrap_or_else(|| review.workspace_root.clone()),
    )
}

/// The file a thread is about, absolute and relative to `root`.
pub(crate) fn thread_file(
    root: &Path,
    thread: &ReviewThread,
) -> Option<(PathBuf, String)> {
    let file = PathBuf::from(file_uri_to_path(thread.location.uri.as_deref()?)?);
    let relative = file
        .strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file.to_string_lossy().into_owned());
    Some((file, relative))
}

fn text(text: String) -> ContentBlock {
    ContentBlock::Text(TextContent::new(text))
}

/// The prompt for a turn. A `fresh` session gets the review and thread context, the map (when given), the code, a
/// link to the file and the conversation so far; a continuing one the reviewer's messages since the last answer.
pub fn build_prompt(
    review: &Review,
    thread: &ReviewThread,
    fresh: bool,
    map: Option<String>,
) -> Vec<ContentBlock> {
    let root = review_root(review);
    let location = &thread.location;
    let file = thread_file(&root, thread);
    let lines = location
        .range
        .as_ref()
        .map(|range| format!("{}-{}", range.start.line + 1, range.end.line + 1));
    let mut blocks = Vec::new();
    if !fresh {
        // The session already has the context; send what the reviewer said since the last answer.
        let last_agent = thread
            .messages
            .iter()
            .rposition(|message| message.author.kind == ParticipantKind::Agent);
        let since: Vec<&str> = thread
            .messages
            .get(last_agent.map_or(0, |index| index + 1)..)
            .unwrap_or_default()
            .iter()
            .filter(|message| message.author.kind == ParticipantKind::Human)
            .map(|message| message.body.as_str())
            .collect();
        let latest: Vec<&str> = thread
            .messages
            .iter()
            .rev()
            .find(|message| message.author.kind == ParticipantKind::Human)
            .map(|message| message.body.as_str())
            .into_iter()
            .collect();
        let messages = if since.is_empty() { latest } else { since };
        blocks.push(text(messages.join("\n\n")));
        return blocks;
    }
    let place = match &file {
        Some((_, relative)) if location.kind != LocationKind::Repository => format!(
            "{relative}{}{}",
            lines
                .as_ref()
                .map(|lines| format!(":{lines}"))
                .unwrap_or_default(),
            location
                .symbol
                .as_ref()
                .map(|symbol| format!(" ({symbol})"))
                .unwrap_or_default()
        ),
        _ => "the repository as a whole".to_string(),
    };
    let intro = [
        format!(
            "You are a participant in a code review of the repository at {} (\"{}\").",
            root.display(),
            review.title
        ),
        format!("A reviewer asked you about {place} in review thread #{}.", thread.number),
        "Look at the code you need, then answer: what you write is posted in the thread as your reply, so don't post it with a tool. A map of the repository follows; use it to go straight to the right files.".to_string(),
        "Do not modify files unless the reviewer explicitly asks you to change code.".to_string(),
        String::new(),
        review_mcp::ANSWER_STYLE.to_string(),
    ];
    blocks.push(text(intro.join("\n")));
    if let Some(map) = map.filter(|map| !map.is_empty()) {
        blocks.push(text(map));
    }
    if let Some(anchor) = &location.anchor
        && !anchor.text.is_empty()
        && location.kind != LocationKind::Repository
    {
        blocks.push(text(format!(
            "The code in question{}:\n```\n{}\n```",
            lines
                .as_ref()
                .map(|lines| format!(" (lines {lines})"))
                .unwrap_or_default(),
            anchor.text
        )));
    }
    if let Some((file, relative)) = &file {
        let uri = path_to_file_uri(&file.to_string_lossy());
        blocks.push(ContentBlock::ResourceLink(ResourceLink::new(
            relative.clone(),
            uri,
        )));
    }
    let history: Vec<String> = thread
        .messages
        .iter()
        .filter(|message| {
            message.status != Some(MessageStatus::Streaming)
                && !message.body.trim().is_empty()
        })
        .map(|message| match message.author.kind {
            ParticipantKind::Agent => format!("Agent: {}", message.body),
            _ => format!("Reviewer ({}): {}", message.author.name, message.body),
        })
        .collect();
    blocks.push(text(format!("Conversation:\n\n{}", history.join("\n\n"))));
    blocks
}

/// The repository map for a first prompt, files near `focus` first; `None` when it isn't ready within `wait`.
pub fn repository_map(
    root: &Path,
    focus: Option<String>,
    wait: Duration,
) -> Option<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let root = root.to_path_buf();
    std::thread::Builder::new()
        .name("co-review-agent-map".to_string())
        .spawn(move || {
            let index = review_index::RepoIndex::new();
            let options = review_index::RenderOptions {
                focus,
                max_chars: Some(MAP_CHARS),
                ..Default::default()
            };
            let rendered = index
                .build(&root)
                .and_then(|map| index.render(&root, &map, &options));
            match rendered {
                Ok(map) => {
                    sender.send(map).ok();
                }
                Err(error) => log::error!("repository map failed: {error:#}"),
            }
        })
        .map_err(|error| log::error!("could not start the repository map: {error}"))
        .ok()?;
    receiver.recv_timeout(wait).ok()
}

/// Builds (or refreshes) the repository map in the background, so the first question finds it ready.
pub fn warm_repository_map(root: &Path) {
    let root = root.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("co-review-agent-map".to_string())
        .spawn(move || {
            if let Err(error) = review_index::RepoIndex::new().build(&root) {
                log::error!("repository map failed: {error:#}");
            }
        });
    if let Err(error) = spawned {
        log::error!("could not start the repository map: {error}");
    }
}
