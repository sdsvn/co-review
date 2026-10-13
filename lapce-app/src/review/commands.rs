//! The `Review:` commands: the review switcher, New / Rename / Delete / Archive / Show archived, the ⋯ menu, Go to
//! Comment…, Mark File as Viewed, Connect agent… and Choose Agent Model…. Their pickers and prompts open in the
//! palette ([`super::picker`]); store writes and agent calls run off the UI thread.

use std::{path::PathBuf, process::Command, rc::Rc, sync::Arc};

use floem::{
    ext_event::create_ext_action,
    reactive::{Scope, SignalGet, SignalUpdate, SignalWith},
};
use lsp_types::{MessageType, ShowMessageParams};
use review_agents::{AgentSetting, command_line, custom_agent, detect_agents, is_acp};
use review_model::{
    AgentConfig, CodeLocation, LocationKind, Review, ReviewScope, ReviewThread,
    ThreadIntent, ThreadStatus,
};
use review_store::CreateReviewParams;

use super::{
    ReviewCore, ReviewPage, ReviewWorkspaceData,
    picker::{self, Choice, Picker},
};
use crate::{
    command::{InternalCommand, LapceWorkbenchCommand},
    editor::location::{EditorLocation, EditorPosition},
    main_split::MainSplitData,
    palette::PaletteData,
    panel::{data::PanelData, kind::PanelKind},
    window_tab::{CommonData, WindowTabData},
};

/// What the review commands use of a window tab; cloned into pickers' actions and background results.
#[derive(Clone)]
pub struct ReviewTab {
    pub scope: Scope,
    pub palette: PaletteData,
    pub main_split: MainSplitData,
    pub panel: PanelData,
    pub review: ReviewWorkspaceData,
    pub common: Rc<CommonData>,
    messages: floem::reactive::RwSignal<Vec<(String, ShowMessageParams)>>,
}

impl ReviewTab {
    pub fn of(window_tab: &WindowTabData) -> Self {
        Self {
            scope: window_tab.scope,
            palette: window_tab.palette.clone(),
            main_split: window_tab.main_split.clone(),
            panel: window_tab.panel.clone(),
            review: window_tab.review.clone(),
            common: window_tab.common.clone(),
            messages: window_tab.messages,
        }
    }

    pub fn show_panel(&self) {
        self.panel.show_panel(&PanelKind::Review);
    }

    /// Makes `page`'s review active and opens the page beside the Review panel.
    pub fn reveal(&self, page: ReviewPage) {
        self.review.active.set(Some(page.review_id.clone()));
        self.show_panel();
        self.main_split.open_review_page(page);
    }

    pub fn info(&self, message: impl Into<String>) {
        self.message(MessageType::INFO, message.into());
    }

    pub fn error(&self, message: impl Into<String>) {
        self.message(MessageType::ERROR, message.into());
    }

    fn message(&self, typ: MessageType, message: String) {
        self.messages.update(|messages| {
            messages.push(("Co-Review".to_string(), ShowMessageParams { typ, message }));
        });
    }

    pub fn pick(&self, picker: Picker) {
        picker::open(&self.palette, picker);
    }

    /// Runs `work` off the UI thread, then `then` with its result on the UI thread.
    pub fn later<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        let send = create_ext_action(self.scope, then);
        let spawned = std::thread::Builder::new()
            .name("ReviewCommand".to_owned())
            .spawn(move || send(work()));
        if let Err(error) = spawned {
            tracing::error!("could not run a review command: {error}");
        }
    }

    /// Runs a store write off the UI thread; a failure reaches the reviewer as a message.
    pub fn write<T: Send + 'static>(
        &self,
        what: &'static str,
        work: impl FnOnce(&review_store::ReviewStore) -> anyhow::Result<T> + Send + 'static,
        then: impl FnOnce(T) + 'static,
    ) {
        let Some(core) = ReviewCore::get() else {
            return;
        };
        let store = Arc::clone(&core.services.store);
        let tab = self.clone();
        self.later(
            move || work(&store).map_err(|error| format!("{error:#}")),
            move |result| match result {
                Ok(value) => then(value),
                Err(error) => tab.error(format!("Could not {what}: {error}")),
            },
        );
    }

    fn uri(&self) -> Option<String> {
        let root = self.review.root.as_ref()?;
        Some(review_store::path_to_file_uri(&root.to_string_lossy()))
    }
}

/// Runs a `Review:` command (the window tab's hook in `run_workbench_command`).
pub fn run(window_tab: &WindowTabData, command: &LapceWorkbenchCommand) {
    let tab = ReviewTab::of(window_tab);
    use LapceWorkbenchCommand::*;
    match command {
        ReviewNew => new_review(&tab),
        ReviewSwitch => switch_review(&tab),
        ReviewRename => rename_review(&tab),
        ReviewDelete => delete_review(&tab),
        ReviewArchive => archive_review(&tab),
        ReviewShowArchived => show_archived(&tab),
        ReviewActions => review_actions(&tab),
        ReviewGoToComment => go_to_comment(&tab),
        ReviewToggleViewed => toggle_viewed(&tab),
        ReviewConnectAgent => connect_agent(&tab),
        ReviewChooseAgentModel => choose_agent_settings(&tab, true),
        ReviewSubmit => {
            if tab.review.active_review_untracked().is_some() {
                tab.show_panel();
                tab.review.submitting.set(true);
            }
        }
        ReviewCommentRepository => comment_on_repository(window_tab),
        ReviewStartOnSelection => {
            let selected = match window_tab.file_explorer.select.get_untracked() {
                Some(lapce_rpc::file::FileNodeViewKind::Path(path)) => Some(path),
                _ => tab
                    .main_split
                    .active_editor
                    .get_untracked()
                    .and_then(|editor| editor.doc().content.get_untracked().path().cloned()),
            };
            match selected {
                Some(path) => review_paths(&tab, vec![path]),
                None => tab.info("Select a file or folder in the explorer, or open a file, to review it."),
            }
        }
        ReviewInstallSkills => super::setup::install_skills(&tab),
        ReviewInstallCommand => super::setup::install_command(&tab, |_| {}),
        ReviewSetupHarness => super::setup::add_to_harness(&tab),
        _ => {}
    }
}

// ---- Reviews: create, switch, rename, delete, archive ----

pub fn new_review(tab: &ReviewTab) {
    let Some(root) = tab.review.root.clone() else {
        tab.info("Open a repository folder before starting a review.");
        return;
    };
    let current_file = tab
        .main_split
        .active_editor
        .get_untracked()
        .and_then(|editor| editor.doc().content.get_untracked().path().cloned())
        .filter(|path| tab.review.relative_path(path).is_some());
    let git_root = root.clone();
    let tab_then = tab.clone();
    tab.later(
        move || git_info(&git_root),
        move |git| pick_scope(&tab_then, root, current_file, git),
    );
}

/// **Start Review of Selection**: a review of these files and folders (the repository's own folder: of the whole
/// repository), after its title.
pub fn review_paths(tab: &ReviewTab, paths: Vec<PathBuf>) {
    let Some(root) = tab.review.root.clone() else {
        tab.info("Open a repository folder before starting a review.");
        return;
    };
    let paths: Vec<(PathBuf, String)> = paths
        .into_iter()
        .filter_map(|path| Some((tab.review.relative_path(&path)?, path)))
        .map(|(relative, path)| (path, relative))
        .collect();
    if paths.is_empty() {
        tab.info("Only files and folders of this repository can be reviewed here.");
        return;
    }
    let scope = if paths.iter().any(|(_, relative)| relative.is_empty()) {
        ReviewScope::Repository
    } else {
        ReviewScope::Paths {
            uris: paths
                .iter()
                .map(|(path, _)| review_store::path_to_file_uri(&path.to_string_lossy()))
                .collect(),
        }
    };
    ask_title(tab, &root, scope);
}

/// The repository's branches and current branch; `None` outside a git repository.
struct GitInfo {
    current: String,
    branches: Vec<String>,
}

fn git(root: &std::path::Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_info(root: &std::path::Path) -> Option<GitInfo> {
    if git(root, &["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        return None;
    }
    let current = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let branches = git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|branch| !branch.is_empty() && !branch.ends_with("/HEAD"))
    .map(str::to_string)
    .collect();
    Some(GitInfo { current, branches })
}

fn pick_scope(
    tab: &ReviewTab,
    root: PathBuf,
    current_file: Option<PathBuf>,
    repository: Option<GitInfo>,
) {
    let mut choices = vec![{
        let (tab, root) = (tab.clone(), root.clone());
        Choice::new("Entire repository", move || {
            ask_title(&tab, &root, ReviewScope::Repository)
        })
        .described("Review everything, independent of Git")
    }];
    if let Some(file) = current_file {
        let (tab, root) = (tab.clone(), root.clone());
        let relative = tab.review.relative_path(&file).unwrap_or_default();
        choices.push(
            Choice::new("Current file", move || {
                let uri = review_store::path_to_file_uri(&file.to_string_lossy());
                ask_title(&tab, &root, ReviewScope::Paths { uris: vec![uri] })
            })
            .described(relative),
        );
    }
    if let Some(repository) = repository {
        let info = Rc::new(repository);
        {
            let (tab, root, info) = (tab.clone(), root.clone(), info.clone());
            choices.push(
                Choice::new("Branch", move || pick_branch(&tab, root, info))
                    .described("Compare a branch against a base"),
            );
        }
        let (tab, root) = (tab.clone(), root.clone());
        choices.push(
            Choice::new("Commit", move || {
                let tab_then = tab.clone();
                tab.pick(Picker::prompt(
                    "Commit to review (SHA or ref)",
                    "HEAD",
                    "Review this commit (Enter)",
                    move |reference| {
                        let reference = reference.trim().to_string();
                        if reference.is_empty() {
                            return;
                        }
                        let git_root = root.clone();
                        let shown = reference.clone();
                        let tab = tab_then.clone();
                        tab_then.later(
                            move || {
                                git(
                                    &git_root,
                                    &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
                                )
                            },
                            move |sha| match sha {
                                Some(sha) => ask_title(&tab, &root, ReviewScope::Commit { sha }),
                                None => tab.error(format!(
                                    "'{shown}' is not a commit in this repository."
                                )),
                            },
                        );
                    },
                ));
            })
            .described("Review a single commit"),
        );
    }
    tab.pick(Picker::choose("What do you want to review?", choices));
}

fn pick_branch(tab: &ReviewTab, root: PathBuf, git: Rc<GitInfo>) {
    let choices = git
        .branches
        .iter()
        .map(|head| {
            let description = if *head == git.current { "current" } else { "" };
            let (tab, root, git, head) = (tab.clone(), root.clone(), git.clone(), head.clone());
            Choice::new(head.clone(), move || {
                let default_base = ["main", "master"]
                    .into_iter()
                    .find(|base| git.branches.iter().any(|branch| branch == base));
                let mut bases: Vec<&String> =
                    git.branches.iter().filter(|branch| **branch != head).collect();
                bases.sort_by_key(|branch| Some(branch.as_str()) != default_base);
                let choices = bases
                    .into_iter()
                    .map(|base| {
                        let (tab, root, head, base) =
                            (tab.clone(), root.clone(), head.clone(), base.clone());
                        Choice::new(base.clone(), move || {
                            ask_title(&tab, &root, ReviewScope::Branch { base, head })
                        })
                    })
                    .collect();
                tab.pick(Picker::choose("Base to compare the branch against", choices));
            })
            .described(description)
        })
        .collect();
    tab.pick(Picker::choose("Branch to review", choices));
}

/// How a scope reads, as the panel's summary line shows it.
pub fn scope_label(scope: &ReviewScope, review: &ReviewWorkspaceData) -> String {
    match scope {
        ReviewScope::Repository => "Entire repository".to_string(),
        ReviewScope::Paths { uris } => uris
            .iter()
            .map(|uri| {
                review_store::file_uri_to_path(uri)
                    .and_then(|path| review.relative_path(std::path::Path::new(&path)))
                    .unwrap_or_else(|| uri.clone())
            })
            .collect::<Vec<_>>()
            .join(", "),
        ReviewScope::Branch { base, head } => format!("{base}...{head}"),
        ReviewScope::Commit { sha } => {
            format!("Commit {}", sha.get(..10).unwrap_or(sha))
        }
    }
}

fn ask_title(tab: &ReviewTab, root: &std::path::Path, scope: ReviewScope) {
    let default_title = match &scope {
        ReviewScope::Repository => format!(
            "Review of {}",
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        other => scope_label(other, &tab.review),
    };
    let Some(uri) = tab.uri() else {
        return;
    };
    let (tab_then, root) = (tab.clone(), root.to_path_buf());
    tab.pick(Picker::prompt(
        "Review title",
        default_title.clone(),
        "Start the review (Enter)",
        move |title| {
            let title = Some(title.trim().to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or(default_title);
            let active = tab_then.review.active;
            let tab = tab_then.clone();
            tab_then.write(
                "start the review",
                move |store| {
                    Ok(store.create(CreateReviewParams {
                        workspace_root: uri,
                        title,
                        scope,
                        author: review_store::current_user(&root),
                    })?)
                },
                move |review| {
                    active.set(Some(review.id.clone()));
                    tab.show_panel();
                },
            );
        },
    ));
}

/// The review switcher: the open reviews, the active one marked; New review… at the end.
pub fn switch_review(tab: &ReviewTab) {
    let active = tab.review.active.get_untracked();
    let mut choices: Vec<Choice> = tab
        .review
        .open_reviews()
        .into_iter()
        .map(|review| {
            let current = active.as_deref() == Some(review.id.as_str());
            let open = review
                .threads
                .iter()
                .filter(|thread| thread.status != ThreadStatus::Resolved)
                .count();
            let signal = tab.review.active;
            let id = review.id.clone();
            Choice::new(review.title.clone(), move || signal.set(Some(id)))
                .described(format!(
                    "{}{open} open thread{}",
                    if current { "active · " } else { "" },
                    if open == 1 { "" } else { "s" }
                ))
        })
        .collect();
    {
        let tab = tab.clone();
        choices.push(Choice::new("New review…", move || new_review(&tab)));
    }
    if !tab.review.archived_reviews().is_empty() {
        let tab = tab.clone();
        choices.push(Choice::new("Show archived reviews…", move || show_archived(&tab)));
    }
    tab.pick(Picker::choose("Switch to a review of this repository", choices));
}

pub fn rename_review(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let tab_then = tab.clone();
    let id = review.id.clone();
    tab.pick(Picker::prompt(
        "Review title",
        review.title.clone(),
        "Rename the review (Enter)",
        move |title| {
            let title = title.trim().to_string();
            if title.is_empty() {
                return;
            }
            tab_then.write("rename the review", move |store| Ok(store.rename(&id, title)?), |_| {});
        },
    ));
}

pub fn delete_review(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let tab_then = tab.clone();
    let id = review.id.clone();
    let threads = review.threads.len();
    tab.pick(Picker::choose(
        "Delete the review? This cannot be undone.",
        vec![
            Choice::new("Delete", move || {
                tab_then.write("delete the review", move |store| Ok(store.delete(&id)?), |_| {});
            })
            .described(format!(
                "\"{}\" and its {threads} thread(s)",
                review.title
            )),
            Choice::new("Cancel", || {}),
        ],
    ));
}

pub fn archive_review(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let tab_then = tab.clone();
    let id = review.id.clone();
    let title = review.title.clone();
    tab.write(
        "archive the review",
        move |store| Ok(store.archive(&id, true)?),
        move |archived: Review| {
            let next = tab_then
                .review
                .open_reviews()
                .into_iter()
                .find(|review| review.id != archived.id)
                .map(|review| review.id.clone());
            tab_then.review.active.set(next);
            tab_then.info(format!(
                "Archived \"{title}\". Reopen it from ⋯ → Show archived reviews."
            ));
        },
    );
}

pub fn show_archived(tab: &ReviewTab) {
    let archived = tab.review.archived_reviews();
    if archived.is_empty() {
        tab.info("No archived reviews.");
        return;
    }
    let choices = archived
        .into_iter()
        .map(|review| {
            let tab = tab.clone();
            let id = review.id.clone();
            let when = review
                .archived_at
                .and_then(|at| chrono::DateTime::from_timestamp_millis(at as i64))
                .map(|at| at.with_timezone(&chrono::Local).format("%b %-d, %Y").to_string())
                .unwrap_or_default();
            let threads = review.threads.len();
            Choice::new(review.title.clone(), move || {
                let active = tab.review.active;
                tab.write(
                    "unarchive the review",
                    move |store| Ok(store.archive(&id, false)?),
                    move |review: Review| active.set(Some(review.id)),
                );
            })
            .described(format!(
                "{threads} thread{} · archived {when}",
                if threads == 1 { "" } else { "s" }
            ))
        })
        .collect();
    tab.pick(Picker::choose("Reopen an archived review", choices));
}

// ---- The ⋯ menu ----

pub fn review_actions(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        show_archived(tab);
        return;
    };
    let tab_then = tab.clone();
    let lookup = review.clone();
    tab.later(
        move || super::submit::github_target(&lookup),
        move |github| actions_menu(&tab_then, &review, github),
    );
}

fn actions_menu(
    tab: &ReviewTab,
    review: &Review,
    github: Option<review_github::GitHubTarget>,
) {
    let action = |label: &str, command: LapceWorkbenchCommand| {
        let common = tab.common.clone();
        Choice::new(label, move || {
            common.lapce_command.send(crate::command::LapceCommand {
                kind: crate::command::CommandKind::Workbench(command),
                data: None,
            })
        })
    };
    let mut choices = Vec::new();
    if let Some(github) = github {
        let tab = tab.clone();
        choices.push(Choice::new(
            format!("Post review to GitHub ({}#{})…", github.repo, github.number),
            move || super::submit::post_from_menu(&tab),
        ));
    }
    choices.push(
        action("Overview", LapceWorkbenchCommand::ReviewOpenOverview)
            .described("the change or repository, and every finding"),
    );
    choices.push(action("Submit review…", LapceWorkbenchCommand::ReviewSubmit));
    choices.push(action("Go to comment…", LapceWorkbenchCommand::ReviewGoToComment));
    choices.push(action(
        "Comment on the repository",
        LapceWorkbenchCommand::ReviewCommentRepository,
    ));
    choices.push(action(
        if review.agent.is_some() { "Change agent…" } else { "Connect agent…" },
        LapceWorkbenchCommand::ReviewConnectAgent,
    ));
    if is_acp(review.agent.as_ref()) {
        choices.push(action(
            "Choose agent model…",
            LapceWorkbenchCommand::ReviewChooseAgentModel,
        ));
    }
    choices.push(action(
        "Add Co-Review to an agent harness (MCP)…",
        LapceWorkbenchCommand::ReviewSetupHarness,
    ));
    choices.push(action("Install agent skills…", LapceWorkbenchCommand::ReviewInstallSkills));
    choices.push(action("New review…", LapceWorkbenchCommand::ReviewNew));
    choices.push(action("Rename review…", LapceWorkbenchCommand::ReviewRename));
    choices.push(action("Archive review", LapceWorkbenchCommand::ReviewArchive));
    if !tab.review.archived_reviews().is_empty() {
        choices.push(action(
            "Show archived reviews…",
            LapceWorkbenchCommand::ReviewShowArchived,
        ));
    }
    choices.push(action("Delete review…", LapceWorkbenchCommand::ReviewDelete));
    tab.pick(Picker::choose(
        "Review actions",
        choices,
    ));
}

// ---- Threads ----

/// Go to Comment…: every open thread and proposed finding of the active review; picking one opens it at its code.
pub fn go_to_comment(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let mut threads: Vec<&ReviewThread> = review
        .threads
        .iter()
        .filter(|thread| thread.status != ThreadStatus::Resolved)
        .collect();
    threads.sort_by_cached_key(|thread| location_label(thread, &tab.review, &review));
    let choices = threads
        .into_iter()
        .map(|thread| {
            let first_line: String = thread
                .messages
                .first()
                .and_then(|message| message.body.lines().next())
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            let messages = thread.messages.len();
            let last_author = thread
                .messages
                .last()
                .map(|message| message.author.name.clone())
                .unwrap_or_default();
            let description = format!(
                "{} · {}{messages} message{} · {last_author}",
                location_label(thread, &tab.review, &review),
                if thread.status == ThreadStatus::Proposed { "Proposed finding · " } else { "" },
                if messages == 1 { "" } else { "s" },
            );
            let (tab, review, thread) = (tab.clone(), review.clone(), thread.clone());
            Choice::new(format!("#{} {first_line}", thread.number), move || {
                open_thread(&tab, &review, &thread)
            })
            .described(description)
        })
        .collect();
    tab.pick(Picker::choose("Open threads and proposed findings", choices));
}

/// The file of a location: its URI's path, or a pull request comment's file in the review's code folder.
fn location_file(review: &Review, location: &CodeLocation) -> Option<(PathBuf, Option<usize>)> {
    if let Some(anchor) = &location.patch_anchor
        && let (Some(path), Some(code)) = (
            anchor.path.as_deref(),
            review.bundle.as_ref().and_then(|bundle| bundle.code.as_deref()),
        )
    {
        let line = anchor.line.or(anchor.start_line).map(|line| line.saturating_sub(1) as usize);
        return Some((PathBuf::from(code).join(path), line));
    }
    let path = PathBuf::from(review_store::file_uri_to_path(location.uri.as_deref()?)?);
    let line = location.range.as_ref().map(|range| range.start.line as usize);
    Some((path, line))
}

/// Opens a thread at its code (Lapce's go-to-location); threads without a file show in the panel.
pub fn open_thread(tab: &ReviewTab, review: &Review, thread: &ReviewThread) {
    tab.review.active.set(Some(review.id.clone()));
    tab.review.selected_thread.set(Some(thread.id.clone()));
    let file = location_file(review, &thread.location)
        .filter(|_| !matches!(thread.location.kind, LocationKind::Repository | LocationKind::Directory))
        .filter(|(path, _)| path.is_file());
    match file {
        Some((path, line)) => tab.common.internal_command.send(InternalCommand::JumpToLocation {
            location: EditorLocation {
                path,
                position: line.map(EditorPosition::Line),
                scroll_offset: None,
                ignore_unconfirmed: false,
                same_editor_tab: false,
            },
        }),
        None => tab.show_panel(),
    }
}

/// Opens a file of the workspace, by its path relative to the root.
pub fn open_relative(tab: &ReviewTab, relative: &str) {
    let Some(root) = &tab.review.root else {
        return;
    };
    tab.common.internal_command.send(InternalCommand::OpenFile {
        path: root.join(relative),
    });
}

/// Where a thread is, relative to the review's root: `path:line`, `path:start-end`, `path/` or Repository.
pub fn location_label(thread: &ReviewThread, workspace: &ReviewWorkspaceData, review: &Review) -> String {
    let group = group_key(&thread.location, workspace, review);
    if let Some(anchor) = &thread.location.patch_anchor
        && let Some(line) = anchor.line.or(anchor.start_line)
    {
        return format!("{group}:{line}");
    }
    match &thread.location.range {
        Some(range) if group != "Repository" => {
            let (start, end) = (range.start.line + 1, range.end.line + 1);
            if start == end {
                format!("{group}:{start}")
            } else {
                format!("{group}:{start}-{end}")
            }
        }
        _ => group,
    }
}

/// A thread's group in the panel: its file (a pull request comment's file, not the patch), folder or Repository.
pub fn group_key(location: &CodeLocation, workspace: &ReviewWorkspaceData, review: &Review) -> String {
    if location.kind == LocationKind::Repository {
        return "Repository".to_string();
    }
    if let Some(path) = location.patch_anchor.as_ref().and_then(|anchor| anchor.path.clone()) {
        return path;
    }
    let Some(path) = location.uri.as_deref().and_then(review_store::file_uri_to_path) else {
        return "Repository".to_string();
    };
    let path = PathBuf::from(path);
    let relative = review
        .bundle
        .as_ref()
        .and_then(|bundle| {
            path.strip_prefix(&bundle.dir)
                .ok()
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        })
        .or_else(|| workspace.relative_path(&path))
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    if location.kind == LocationKind::Directory {
        format!("{relative}/")
    } else {
        relative
    }
}

/// Comment on the repository: a draft in the Review panel.
pub fn comment_on_repository(window_tab: &WindowTabData) {
    let Some(review) = window_tab.review.active_review_untracked() else {
        return;
    };
    let location = CodeLocation {
        kind: LocationKind::Repository,
        uri: None,
        range: None,
        symbol: None,
        anchor: None,
        doc_anchor: None,
        patch_anchor: None,
        tracked: None,
        extra: serde_json::Map::new(),
    };
    super::drafts::add_to(window_tab, &review.id, location, ThreadIntent::Comment);
    window_tab.panel.show_panel(&PanelKind::Review);
}

// ---- Viewed files ----

/// Mark File as Viewed / Not Viewed: the file in the editor, in the active repository review.
pub fn toggle_viewed(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    if review.bundle.is_some() {
        return;
    }
    let Some(path) = tab
        .main_split
        .active_editor
        .get_untracked()
        .and_then(|editor| editor.doc().content.get_untracked().path().cloned())
    else {
        return;
    };
    let Some(relative) = tab.review.relative_path(&path) else {
        tab.info("Only files of this repository can be marked as viewed.");
        return;
    };
    let viewed = review
        .viewed
        .as_ref()
        .is_some_and(|viewed| viewed.contains_key(&relative));
    let id = review.id.clone();
    tab.write(
        "mark the file",
        move |store| Ok(store.set_viewed(&id, &[relative], !viewed)?),
        |_| {},
    );
}

/// Whether the active review counts `path` as viewed (marked, whatever happened since).
pub fn is_marked_viewed(workspace: &ReviewWorkspaceData, review: &Review, path: &std::path::Path) -> bool {
    workspace
        .relative_path(path)
        .is_some_and(|relative| review.viewed.as_ref().is_some_and(|viewed| viewed.contains_key(&relative)))
}

// ---- Agents ----

/// Connect agent…: a detected ACP agent, a custom command, or none.
pub fn connect_agent(tab: &ReviewTab) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let tab_then = tab.clone();
    tab.later(detect_agents, move |presets| agent_picker(&tab_then, &review, presets));
}

fn agent_picker(tab: &ReviewTab, review: &Review, presets: Vec<AgentConfig>) {
    let current = review.agent.clone();
    let mut choices: Vec<Choice> = presets
        .into_iter()
        .map(|agent| {
            let connected = current.as_ref().is_some_and(|current| current.id == agent.id);
            let description = format!(
                "{}{}",
                command_line(&agent),
                if connected { " · Connected" } else { "" }
            );
            let (tab, review_id) = (tab.clone(), review.id.clone());
            Choice::new(agent.name.clone(), move || set_acp_agent(&tab, review_id, agent))
                .described(description)
        })
        .collect();
    {
        let (tab, review_id) = (tab.clone(), review.id.clone());
        let value = current
            .as_ref()
            .filter(|agent| agent.id == "custom")
            .map(command_line)
            .unwrap_or_default();
        choices.push(
            Choice::new("Custom ACP command…", move || {
                let tab_then = tab.clone();
                tab.pick(Picker::prompt(
                    "Command that starts an ACP agent on stdio (run in the repository root)",
                    value,
                    "Use this command (Enter), e.g. npx -y @agentclientprotocol/claude-agent-acp",
                    move |line| {
                        let Some(agent) = custom_agent(&line) else {
                            return;
                        };
                        let tab = tab_then.clone();
                        tab_then.pick(Picker::prompt(
                            "Display name for this agent",
                            agent.name.clone(),
                            "Connect the agent (Enter)",
                            move |name| {
                                let name = name.trim().to_string();
                                let agent = AgentConfig {
                                    name: if name.is_empty() { agent.name.clone() } else { name },
                                    ..agent
                                };
                                set_acp_agent(&tab, review_id, agent);
                            },
                        ));
                    },
                ));
            })
            .described("Any agent that speaks ACP over stdio"),
        );
    }
    if current.is_some() {
        let (tab, review_id) = (tab.clone(), review.id.clone());
        choices.push(Choice::new("Disconnect agent", move || {
            let Some(core) = ReviewCore::get() else {
                return;
            };
            let agents = Arc::clone(&core.agents);
            let disconnecting = review_id.clone();
            tab.write(
                "disconnect the agent",
                move |store| {
                    agents.disconnect(&disconnecting);
                    Ok(store.set_agent(&disconnecting, None)?)
                },
                |_| {},
            );
        }));
    }
    tab.pick(Picker::choose(
        "Agent for this review (Agent Client Protocol)",
        choices,
    ));
}

fn set_acp_agent(tab: &ReviewTab, review_id: String, agent: AgentConfig) {
    let Some(core) = ReviewCore::get() else {
        return;
    };
    let agents = Arc::clone(&core.agents);
    let tab_then = tab.clone();
    tab.write(
        "connect the agent",
        move |_| agents.connect(&review_id, agent),
        move |_| choose_agent_settings(&tab_then, false),
    );
}

/// Choose Agent Model…: the model, then the reasoning effort, the agent offers over ACP. Escape keeps the current
/// choice. `explicit`: asked for, so say when there is nothing to choose.
pub fn choose_agent_settings(tab: &ReviewTab, explicit: bool) {
    let Some(review) = tab.review.active_review_untracked() else {
        return;
    };
    let Some(agent) = review.agent.clone().filter(|agent| is_acp(Some(agent))) else {
        if explicit {
            tab.info("Connect an ACP agent to the review first (Review: Connect ACP Agent…).");
        }
        return;
    };
    let Some(core) = ReviewCore::get() else {
        return;
    };
    tab.info(format!("Asking {} which models it offers…", agent.name));
    let agents = Arc::clone(&core.agents);
    let review_id = review.id.clone();
    let tab_then = tab.clone();
    tab.later(
        move || agents.list_options(&review_id).map_err(|error| format!("{error:#}")),
        move |offered| match offered {
            Ok(settings) if settings.is_empty() => {
                if explicit {
                    tab_then.info(format!(
                        "{} doesn't offer a choice of model over ACP; it uses its own default.",
                        agent.name
                    ));
                }
            }
            Ok(settings) => pick_setting(&tab_then, review.id.clone(), settings, 0),
            Err(error) => tab_then.error(format!("Could not start {}: {error}", agent.name)),
        },
    );
}

fn pick_setting(tab: &ReviewTab, review_id: String, settings: Vec<AgentSetting>, index: usize) {
    let Some(setting) = settings.get(index).cloned() else {
        return;
    };
    let settings = Rc::new(settings);
    let mut options = setting.options.clone();
    // The current choice first, then the agent's order.
    options.sort_by_key(|option| option.value != setting.current);
    let choices = options
        .into_iter()
        .map(|option| {
            let current = option.value == setting.current;
            let description = option
                .description
                .clone()
                .or_else(|| (option.name != option.value).then(|| option.value.clone()))
                .unwrap_or_default();
            let (tab, review_id, settings, setting_id) =
                (tab.clone(), review_id.clone(), settings.clone(), setting.id.clone());
            let label = format!("{}{}", if current { "✓ " } else { "" }, option.name);
            Choice::new(label, move || {
                let Some(core) = ReviewCore::get() else {
                    return;
                };
                let agents = Arc::clone(&core.agents);
                let (chosen_review, value) = (review_id.clone(), option.value.clone());
                let tab_then = tab.clone();
                tab.write(
                    "choose the agent's model",
                    move |_| agents.set_option(&chosen_review, &setting_id, &value),
                    move |_| {
                        pick_setting(&tab_then, review_id, Rc::unwrap_or_clone(settings), index + 1)
                    },
                );
            })
            .described(description)
        })
        .collect();
    let placeholder = if setting.category == "model" {
        "The agent's model (Escape keeps the current one)"
    } else {
        "The agent's reasoning effort (Escape keeps the current one)"
    };
    tab.pick(Picker::choose(placeholder, choices));
}

/// The agent's name and its status for the panel: model chosen, whether an MCP agent is listening.
pub fn agent_label(agent: &AgentConfig) -> String {
    let chosen = agent.settings_label.clone().unwrap_or_else(|| {
        agent
            .settings
            .as_ref()
            .map(|settings| settings.values().cloned().collect::<Vec<_>>().join(" · "))
            .unwrap_or_default()
    });
    if is_acp(Some(agent)) && !chosen.is_empty() {
        format!("{} · {chosen}", agent.name)
    } else {
        agent.name.clone()
    }
}

/// The open threads of the active review are counted from this, as the panel's filters do.
pub fn is_out_of_scope(thread: &ReviewThread) -> bool {
    thread.status != ThreadStatus::Resolved
        && thread.location.tracked.as_ref().is_some_and(|tracked| {
            matches!(
                tracked.status,
                review_model::AnchorStatus::Removed | review_model::AnchorStatus::Ambiguous
            )
        })
}

/// Opens the overview of the active review (Overview link).
pub fn open_overview(tab: &ReviewTab) {
    if let Some(review) = tab.review.active_review_untracked() {
        tab.reveal(ReviewPage::overview(&review.id));
    }
}

/// Whether `review` is the window tab's active review, as signals see it.
pub fn is_active(workspace: &ReviewWorkspaceData, review_id: &str) -> bool {
    workspace.active.with(|active| active.as_deref() == Some(review_id))
}
