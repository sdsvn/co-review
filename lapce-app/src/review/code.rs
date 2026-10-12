//! From a change review's diff into its code (`bundle.code`, see `review_code`): the head file at a line, the base
//! version read-only, base and head side by side, and what the language server knows about a name on a new-side line,
//! asked at the same position in the head file without opening it.

use std::{path::PathBuf, rc::Rc, sync::Arc};

use floem::{
    ext_event::create_ext_action,
    reactive::SignalGet,
};
use lapce_rpc::proxy::ProxyResponse;
use lapce_xi_rope::Rope;
use lsp_types::{GotoDefinitionResponse, HoverContents, MarkedString, Position};
use review_model::{CodeLocation, Review};

use super::ReviewCore;
use crate::{
    command::InternalCommand,
    doc::{Doc, DocContent, DocHistory},
    editor::location::{EditorLocation, EditorPosition},
    editor_tab::{EditorTabChild, EditorTabChildSource},
    lsp::path_from_url,
    main_split::MainSplitData,
    markdown::{MarkdownContent, from_marked_string, parse_markdown},
    window_tab::WindowTabData,
};

/// The review's code folder, when it has one to step into.
pub fn code_folder(review: &Review) -> Option<PathBuf> {
    review
        .bundle
        .as_ref()
        .and_then(|bundle| bundle.code.as_deref())
        .map(PathBuf::from)
}

/// The head file of a repository-relative `file`.
pub fn head_path(review: &Review, file: &str) -> Option<PathBuf> {
    code_folder(review).map(|code| code.join(file))
}

/// The head file at `line` (1-based) and `column` (0-based), in the editor.
pub fn open_head(main_split: &MainSplitData, review: &Review, file: &str, line: u32, column: u32) {
    let Some(path) = head_path(review, file) else {
        return;
    };
    main_split.jump_to_location(
        EditorLocation {
            path,
            position: Some(EditorPosition::Position(Position {
                line: line.saturating_sub(1),
                character: column,
            })),
            scroll_offset: None,
            ignore_unconfirmed: false,
            same_editor_tab: false,
        },
        None,
    );
}

/// The base version of `file` at `line` (1-based), read-only. Read from git off the UI thread.
pub fn open_base(window_tab: &WindowTabData, review: Arc<Review>, file: String, line: u32) {
    let shown = window_tab.main_split.clone();
    let shown_file = file.clone();
    with_base(window_tab, review.clone(), file, move |base| {
        let Some(text) = base else {
            tracing::info!("{shown_file} has no base version");
            return;
        };
        let Some(head) = head_path(&review, &shown_file) else {
            return;
        };
        let doc = base_doc(&shown, head.clone(), "base", text);
        // A path no editor has, so the head file's editor is not reused for its base.
        let child = shown.get_editor_tab_child(
            EditorTabChildSource::Editor {
                path: PathBuf::from(format!("{} (base)", head.display())),
                doc,
            },
            false,
            false,
        );
        if let EditorTabChild::Editor(editor_id) = child
            && let Some(editor) = shown.editors.editor_untracked(editor_id)
        {
            editor.go_to_location(
                EditorLocation {
                    path: head,
                    position: Some(EditorPosition::Line(line.saturating_sub(1) as usize)),
                    scroll_offset: None,
                    ignore_unconfirmed: false,
                    same_editor_tab: false,
                },
                false,
                None,
            );
        }
    });
}

/// Base and head of `file` side by side (Lapce's diff editor), the head editable. A file the change adds has an empty
/// base, so all its lines show as added.
pub fn compare(window_tab: &WindowTabData, review: Arc<Review>, file: String, line: Option<u32>) {
    let shown = window_tab.main_split.clone();
    let shown_file = file.clone();
    with_base(window_tab, review.clone(), file, move |base| {
        let Some(head) = head_path(&review, &shown_file) else {
            return;
        };
        let version = if base.is_some() { "base" } else { "added" };
        let left = base_doc(&shown, head.clone(), version, base.unwrap_or_default());
        let (right, new_doc) = shown.get_doc(head.clone(), None);
        let child = shown.get_editor_tab_child(
            EditorTabChildSource::DiffEditor { left, right },
            false,
            false,
        );
        if let (Some(line), EditorTabChild::DiffEditor(diff_editor_id)) = (line, child)
            && let Some(diff_editor) = shown
                .diff_editors
                .get_untracked()
                .get(&diff_editor_id)
                .cloned()
        {
            diff_editor.right.go_to_location(
                EditorLocation {
                    path: head,
                    position: Some(EditorPosition::Line(line.saturating_sub(1) as usize)),
                    scroll_offset: None,
                    ignore_unconfirmed: false,
                    same_editor_tab: false,
                },
                new_doc,
                None,
            );
        }
    });
}

/// Reads the base of `file` off the UI thread, then calls `done` on it with the text (`None`: not in the base).
fn with_base(
    window_tab: &WindowTabData,
    review: Arc<Review>,
    file: String,
    done: impl FnOnce(Option<String>) + 'static,
) {
    let Some(core) = ReviewCore::get() else {
        return;
    };
    let folders = Arc::clone(&core.services.code_folders);
    let send = create_ext_action(window_tab.scope, done);
    let read = std::thread::Builder::new()
        .name("ReviewBaseFile".to_owned())
        .spawn(move || send(folders.base_file(&review, &file)));
    if let Err(error) = read {
        tracing::error!("could not read a base version: {error}");
    }
}

/// A read-only document holding a version of `path`, highlighted as `path`.
fn base_doc(main_split: &MainSplitData, path: PathBuf, version: &str, text: String) -> Rc<Doc> {
    let doc = Doc::new_history(
        main_split.scope,
        DocContent::History(DocHistory {
            path,
            version: version.to_string(),
        }),
        main_split.editors,
        main_split.common.clone(),
    );
    doc.init_content(Rope::from(text));
    Rc::new(doc)
}

/// Runs `then` once the head file is loaded into the proxy (and so its language server has it open).
fn with_loaded_head(window_tab: &WindowTabData, path: PathBuf, then: impl FnOnce() + 'static) {
    let (doc, _) = window_tab.main_split.get_doc(path, None);
    let loaded = doc.loaded;
    if loaded.get_untracked() {
        then();
        return;
    }
    let then = std::cell::Cell::new(Some(then));
    window_tab.scope.create_effect(move |_| {
        if loaded.get()
            && let Some(then) = then.take()
        {
            then();
        }
    });
}

/// Goes to the definition of what is at `line` (1-based), `column` (0-based) of the head file; without an answer from
/// a language server, opens the head file there, where the editor can try.
pub fn go_to_definition(window_tab: &WindowTabData, review: Arc<Review>, file: String, line: u32, column: u32) {
    let Some(path) = head_path(&review, &file) else {
        return;
    };
    let fallback = window_tab.main_split.clone();
    let internal_command = window_tab.common.internal_command;
    let send = create_ext_action(window_tab.scope, move |location: Option<EditorLocation>| match location {
        Some(location) => internal_command.send(InternalCommand::JumpToLocation { location }),
        None => open_head(&fallback, &review, &file, line, column),
    });
    let proxy = window_tab.common.proxy.clone();
    let asked = path.clone();
    with_loaded_head(window_tab, path, move || {
        let position = Position {
            line: line.saturating_sub(1),
            character: column,
        };
        proxy.get_definition(0, asked, position, move |result| {
            let location = match result {
                Ok(ProxyResponse::GetDefinitionResponse { definition, .. }) => match definition {
                    GotoDefinitionResponse::Scalar(location) => Some((location.uri, location.range.start)),
                    GotoDefinitionResponse::Array(locations) => locations
                        .into_iter()
                        .next()
                        .map(|location| (location.uri, location.range.start)),
                    GotoDefinitionResponse::Link(links) => links
                        .into_iter()
                        .next()
                        .map(|link| (link.target_uri, link.target_selection_range.start)),
                },
                _ => None,
            };
            send(location.map(|(uri, start)| EditorLocation {
                path: path_from_url(&uri),
                position: Some(EditorPosition::Position(start)),
                scroll_offset: None,
                ignore_unconfirmed: false,
                same_editor_tab: false,
            }));
        });
    });
}

/// The language server's hover for what is at `line` (1-based), `column` (0-based) of the head file, as rendered
/// Markdown; `done` is not called without an answer.
pub fn hover(
    window_tab: &WindowTabData,
    review: &Review,
    file: &str,
    line: u32,
    column: u32,
    done: impl FnOnce(Vec<MarkdownContent>) + 'static,
) {
    let Some(path) = head_path(review, file) else {
        return;
    };
    let config = window_tab.common.config;
    let send = create_ext_action(window_tab.scope, move |hover: lsp_types::Hover| {
        let content = hover_content(hover, &config.get_untracked());
        if !content.is_empty() {
            done(content);
        }
    });
    let proxy = window_tab.common.proxy.clone();
    let asked = path.clone();
    with_loaded_head(window_tab, path, move || {
        let position = Position {
            line: line.saturating_sub(1),
            character: column,
        };
        proxy.get_hover(0, asked, position, move |result| {
            if let Ok(ProxyResponse::HoverResponse { hover, .. }) = result {
                send(hover);
            }
        });
    });
}

fn hover_content(hover: lsp_types::Hover, config: &crate::config::LapceConfig) -> Vec<MarkdownContent> {
    match hover.contents {
        HoverContents::Scalar(MarkedString::String(text)) => parse_markdown(&text, 1.8, config),
        HoverContents::Scalar(MarkedString::LanguageString(code)) => {
            parse_markdown(&format!("```{}\n{}\n```", code.language, code.value), 1.8, config)
        }
        HoverContents::Array(array) => array
            .into_iter()
            .map(|text| from_marked_string(text, config))
            .reduce(|mut contents, more| {
                contents.push(MarkdownContent::Separator);
                contents.extend(more);
                contents
            })
            .unwrap_or_default(),
        HoverContents::Markup(content) => parse_markdown(&content.value, 1.8, config),
    }
}

/// Where a comment written in the editor goes: on lines the change's diff shows (added or context, reading as they do
/// in the diff), a comment on the diff, so it is on the pull-request page and posted to GitHub like one made there;
/// anywhere else `location` itself. Reads the review's patches: call it off the UI thread, with
/// `ReviewCore::services.code_folders`, before creating a thread from the editor.
pub fn comment_location(
    folders: &review_code::CodeFolders,
    review: &Review,
    location: CodeLocation,
) -> CodeLocation {
    folders.on_diff(review, &location).unwrap_or(location)
}
