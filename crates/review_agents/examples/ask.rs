//! Asks a real ACP agent one question about a function, the way the Review panel does, and prints the thread as the
//! answer streams in.
//!
//! `cargo run -p review_agents --example ask -- <repository> <file> <line> <agent id | ACP command> <question>`
//!
//! `agent id` is one of the detected presets (`omp`, `pi`, `claude`, …). Reviews go to a temporary Co-Review home.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use review_agents::{
    AgentEvent, AgentService, ConnectionState, custom_agent, detect_agents,
};
use review_mcp::HumanDecisions;
use review_model::{
    AgentState, CodeAnchor, CodeLocation, LocationKind, MessageStatus, Participant,
    ParticipantKind, Position, Range, ReviewScope, ThreadIntent, ThreadOptions,
};
use review_store::{CreateReviewParams, ReviewStore, path_to_file_uri};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [repository, file, line, agent, question] = arguments.as_slice() else {
        bail!(
            "usage: ask <repository> <file> <line> <agent id | ACP command> <question>"
        );
    };
    let detected = detect_agents();
    println!(
        "detected: {}",
        detected
            .iter()
            .map(|agent| agent.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let agent = detected
        .into_iter()
        .find(|preset| &preset.id == agent)
        .or_else(|| custom_agent(agent))
        .context("no agent")?;
    let root = std::fs::canonicalize(repository)?;
    let line: u32 = line.parse::<u32>()?.saturating_sub(1);
    let text = std::fs::read_to_string(root.join(file))?;
    let anchor = text
        .lines()
        .nth(line as usize)
        .unwrap_or_default()
        .to_string();

    let home =
        std::env::temp_dir().join(format!("co-review-ask-{}", std::process::id()));
    let store = Arc::new(ReviewStore::with_home(home.clone()));
    let service =
        AgentService::new(Arc::clone(&store), Arc::new(HumanDecisions::default()));
    let events = service.subscribe();
    let reviewer = Participant {
        id: "human:reviewer@example.com".into(),
        kind: ParticipantKind::Human,
        name: "Reviewer".into(),
    };
    let review = store.create(CreateReviewParams {
        workspace_root: path_to_file_uri(&root.to_string_lossy()),
        title: "Manual ask".into(),
        scope: ReviewScope::Repository,
        author: reviewer.clone(),
    })?;
    service.connect(&review.id, agent)?;
    match service.list_options(&review.id) {
        Ok(settings) => {
            for setting in settings {
                println!(
                    "option {} ({}): current {} of {}",
                    setting.id,
                    setting.category,
                    setting.current,
                    setting
                        .options
                        .iter()
                        .map(|option| option.value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        Err(error) => println!("options failed: {error:#}"),
    }
    let location = CodeLocation {
        kind: LocationKind::Line,
        uri: Some(path_to_file_uri(&root.join(file).to_string_lossy())),
        range: Some(Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: anchor.len() as u32,
            },
        }),
        symbol: None,
        anchor: Some(CodeAnchor {
            text: anchor,
            before: None,
            after: None,
            syntax: None,
        }),
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: serde_json::Map::new(),
    };
    let options = ThreadOptions {
        intent: Some(ThreadIntent::Question),
        ..ThreadOptions::default()
    };
    let (_, thread) = store.create_thread(
        &review.id,
        location,
        question.clone(),
        reviewer,
        options,
    )?;
    let started = Instant::now();
    service.ask(&review.id, &thread.id);
    let mut last_line = String::new();
    let mut updates = 0;
    let answer = loop {
        match events.recv_timeout(Duration::from_secs(300)) {
            Ok(AgentEvent::Thread { thread, .. }) => {
                let Some(message) = thread
                    .messages
                    .last()
                    .filter(|m| m.author.kind == ParticipantKind::Agent)
                else {
                    continue;
                };
                updates += 1;
                let activity = message.activity.as_ref().map_or(0, Vec::len);
                let line = format!(
                    "{:?} {:?}: {} chars, {activity} activity lines",
                    thread.agent_state,
                    message.status,
                    message.body.len()
                );
                if line != last_line {
                    println!("[{:>6} ms] {line}", started.elapsed().as_millis());
                    last_line = line;
                }
                if thread.agent_state == Some(AgentState::Idle)
                    && message.status != Some(MessageStatus::Streaming)
                {
                    break message.clone();
                }
            }
            Ok(AgentEvent::Connection { state, .. }) => println!(
                "[{:>6} ms] connection: {state:?}",
                started.elapsed().as_millis()
            ),
            Err(_) => bail!("no answer within 5 minutes"),
        }
    };
    for activity in answer.activity.iter().flatten() {
        println!(
            "activity: {:?} {} ({})",
            activity.status,
            activity.title,
            activity.kind.as_deref().unwrap_or("-")
        );
    }
    println!(
        "\n--- {:?} after {} ms, {updates} thread updates ---\n{}\n",
        answer.status,
        started.elapsed().as_millis(),
        answer.body
    );
    service.disconnect(&review.id);
    let exited = loop {
        match events.recv_timeout(Duration::from_secs(10)) {
            Ok(AgentEvent::Connection {
                state: state @ ConnectionState::Exited { .. },
                ..
            }) => break Some(state),
            Ok(_) => {}
            Err(_) => break None,
        }
    };
    println!("after disconnect: {exited:?}");
    std::fs::remove_dir_all(home).ok();
    Ok(())
}
