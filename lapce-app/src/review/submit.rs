//! Submitting a review: the submit bar (where the verdict is: not submitted, waiting for the agent, the agent has it),
//! the form (Approve / Request changes / Comment and a message), and for a pull request, posting to GitHub after the
//! reviewer has seen, and could edit, exactly what goes out.

use std::{cell::Cell, collections::HashMap, path::Path, rc::Rc, sync::Arc};

use floem::{
    IntoView, View,
    reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_rw_signal},
    style::CursorStyle,
    views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, v_stack},
};
use review_github::{GitHubEdits, GitHubPreview, GitHubTarget};
use review_model::{Review, ReviewBundle, ReviewDecision, VerdictToAgent};

use super::{
    ReviewCore,
    commands::ReviewTab,
    thread::{reply_box, text_button},
};
use crate::{config::color::LapceColor, settings::checkbox, window_tab::WindowTabData};


pub fn decision_label(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Approve => "Approve",
        ReviewDecision::RequestChanges => "Request changes",
        ReviewDecision::Comment => "Comment",
    }
}

/// The pull request the review's directory points at (`PR.md`); reads files, so call it off the UI thread.
pub fn github_target(review: &Review) -> Option<GitHubTarget> {
    let dir = review.bundle.as_ref()?.dir.clone();
    review_github::resolve_target(Path::new(&dir), review)
}

thread_local! {
    /// The next submit form starts with "Also post to GitHub" checked (⋯ → Post review to GitHub…).
    static POST_NEXT: Cell<bool> = const { Cell::new(false) };
}

/// ⋯ → Post review to GitHub…: the submit form, set to post.
pub fn post_from_menu(tab: &ReviewTab) {
    POST_NEXT.with(|post| post.set(true));
    tab.show_panel();
    tab.review.submitting.set(false);
    tab.review.submitting.set(true);
}

/// The submit bar, or the form while it is open. Shown for reviews with an agent or a review directory.
pub fn submit_section(window_tab_data: Rc<WindowTabData>) -> impl View {
    let review = window_tab_data.review.clone();
    let submitting = review.submitting;
    let note: RwSignal<Option<(String, Option<String>, bool)>> = create_rw_signal(None);
    let shown = move || {
        review
            .active_review()
            .is_some_and(|review| review.bundle.is_some() || review.agent.is_some())
    };
    dyn_container(
        move || (shown(), submitting.get()),
        move |(shown, open)| match (shown, open) {
            (false, _) => empty().into_any(),
            (true, false) => submit_bar(window_tab_data.clone(), note).into_any(),
            (true, true) => submit_form(window_tab_data.clone(), note).into_any(),
        },
    )
    .style(|s| s.width_full())
}

fn submit_bar(
    window_tab_data: Rc<WindowTabData>,
    note: RwSignal<Option<(String, Option<String>, bool)>>,
) -> impl View {
    let config = window_tab_data.common.config;
    let review = window_tab_data.review.clone();
    let submitting = review.submitting;
    let status = move || {
        let Some(review) = review.active_review() else {
            return String::new();
        };
        let agent = review
            .agent
            .as_ref()
            .map_or("the agent".to_string(), |agent| agent.name.clone());
        match &review.verdict {
            None => "Not submitted yet".to_string(),
            Some(verdict) => format!(
                "Round {}: {}{}",
                verdict.count,
                decision_label(verdict.decision),
                match verdict.to_agent {
                    Some(VerdictToAgent::Waiting) => format!(" · waiting for {agent} to pick it up"),
                    Some(VerdictToAgent::Delivered) => format!(" · {agent} has it"),
                    None => String::new(),
                }
            ),
        }
    };
    v_stack((
        h_stack((
            label(status).style(move |s| {
                s.color(config.get().color(LapceColor::PANEL_FOREGROUND_DIM))
                    .flex_grow(1.0_f32)
                    .min_width(0.0)
            }),
            link("Submit review…", config, move || submitting.set(true)),
        ))
        .style(|s| s.width_full().items_center()),
        label(move || note.get().map(|(text, _, _)| text).unwrap_or_default()).style(move |s| {
            let config = config.get();
            let error = note.with(|note| note.as_ref().is_some_and(|(_, _, error)| *error));
            s.apply_if(note.with(Option::is_none), |s| s.hide())
                .color(config.color(if error {
                    LapceColor::LAPCE_ERROR
                } else {
                    LapceColor::PANEL_FOREGROUND_DIM
                }))
                .margin_top(4.0)
        }),
        link("Open on GitHub", config, move || {
            if let Some((_, Some(url), _)) = note.get_untracked()
                && let Err(error) = open::that(&url)
            {
                tracing::error!("could not open {url}: {error}");
            }
        })
        .style(move |s| {
            s.apply_if(note.with(|note| !note.as_ref().is_some_and(|(_, url, _)| url.is_some())), |s| s.hide())
        }),
    ))
    .style(|s| s.width_full().padding_horiz(10.0).padding_vert(6.0).flex_col())
}

/// A clickable word in the panel's link colour.
pub fn link(
    text: &'static str,
    config: floem::reactive::ReadSignal<Arc<crate::config::LapceConfig>>,
    on_click: impl Fn() + 'static,
) -> floem::views::Label {
    label(move || text)
        .on_click_stop(move |_| on_click())
        .style(move |s| {
            s.cursor(CursorStyle::Pointer)
                .color(config.get().color(LapceColor::EDITOR_LINK))
                .margin_left(8.0)
        })
}

/// The preview of a GitHub post, as the reviewer edits it.
#[derive(Clone)]
struct PostPreview {
    target: GitHubTarget,
    preview: GitHubPreview,
    body: RwSignal<String>,
    comments: Vec<(String, RwSignal<String>)>,
}

fn submit_form(
    window_tab_data: Rc<WindowTabData>,
    note: RwSignal<Option<(String, Option<String>, bool)>>,
) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let submitting = tab.review.submitting;
    let decision = create_rw_signal(ReviewDecision::Comment);
    let summary = create_rw_signal(String::new());
    let post = create_rw_signal(POST_NEXT.with(|post| post.replace(false)));
    let target: RwSignal<Option<GitHubTarget>> = create_rw_signal(None);
    let preview: RwSignal<Option<PostPreview>> = create_rw_signal(None);
    let busy = create_rw_signal(false);
    if let Some(review) = tab.review.active_review_untracked() {
        tab.later(move || github_target(&review), move |found| target.set(found));
    }

    let decisions = h_stack((
        decision_button(ReviewDecision::Approve, decision, config),
        decision_button(ReviewDecision::RequestChanges, decision, config),
        decision_button(ReviewDecision::Comment, decision, config),
    ))
    .style(|s| s.margin_bottom(6.0));
    let submit = {
        let tab = tab.clone();
        Rc::new(move || {
            if busy.get_untracked() {
                return;
            }
            let Some(review) = tab.review.active_review_untracked() else {
                return;
            };
            let (decision, summary_text) = (decision.get_untracked(), summary.get_untracked().trim().to_string());
            match (post.get_untracked(), target.get_untracked()) {
                (true, Some(github)) => {
                    // Posting too: show exactly what goes to GitHub first; nothing is submitted until it is confirmed.
                    // Whose pull request it is takes `gh`, so the preview is built off the UI thread.
                    let Some(core) = ReviewCore::get() else {
                        return;
                    };
                    let runner = Arc::clone(&core.services.github);
                    busy.set(true);
                    tab.later(
                        move || {
                            let own = decision != ReviewDecision::Comment
                                && review_github::own_pull_request(runner.as_ref(), &github) == Some(true);
                            let built = review_github::preview(&review, decision, &summary_text, &HashMap::new(), own);
                            (github, built)
                        },
                        move |(github, built)| {
                            busy.set(false);
                            preview.set(Some(PostPreview {
                                body: create_rw_signal(built.body.clone()),
                                comments: built
                                    .comments
                                    .iter()
                                    .map(|comment| (comment.thread_id.clone(), create_rw_signal(comment.body.clone())))
                                    .collect(),
                                target: github,
                                preview: built,
                            }));
                        },
                    );
                }
                _ => {
                    busy.set(true);
                    submit_review(&tab, review.id.clone(), decision, summary_text, None, note, busy);
                }
            }
        })
    };
    let form = {
        let window_tab_data = window_tab_data.clone();
        let (submit, submit_button) = (submit.clone(), submit.clone());
        v_stack((
            decisions,
            reply_box(
                window_tab_data,
                "Message to the agent (optional)",
                String::new(),
                move |_| submit(),
                move |text| summary.set(text),
            ),
            h_stack((
                checkbox(move || post.get(), config)
                    .on_click_stop(move |_| post.update(|post| *post = !*post)),
                label(move || {
                    target
                        .get()
                        .map(|github| {
                            format!(
                                "Also post to GitHub: {}#{}{}",
                                github.repo,
                                github.number,
                                github
                                    .posted_round
                                    .map(|round| format!(" (round {round} was posted)"))
                                    .unwrap_or_default()
                            )
                        })
                        .unwrap_or_default()
                })
                .on_click_stop(move |_| post.update(|post| *post = !*post))
                .style(|s| s.margin_left(6.0).cursor(CursorStyle::Pointer)),
            ))
            .style(move |s| s.margin_top(6.0).items_center().apply_if(target.with(Option::is_none), |s| s.hide())),
            h_stack((
                empty().style(|s| s.flex_grow(1.0_f32)),
                link("Cancel", config, move || submitting.set(false)),
                text_button("Submit review".to_string(), true, move || submit_button(), config),
            ))
            .style(|s| s.width_full().items_center().margin_top(6.0)),
        ))
        .style(|s| s.width_full().flex_col())
    };
    let previewing = move || preview.with(Option::is_some);
    let preview_panel = dyn_container(
        move || preview.get().map(|shown| shown.target.url.clone()),
        {
            let window_tab_data = window_tab_data.clone();
            move |shown_url| match (shown_url, preview.get_untracked()) {
                (Some(_), Some(shown)) => preview_view(
                    window_tab_data.clone(),
                    tab.clone(),
                    shown,
                    decision.get_untracked(),
                    summary.get_untracked().trim().to_string(),
                    preview,
                    note,
                    busy,
                )
                .into_any(),
                _ => empty().into_any(),
            }
        },
    )
    .style(move |s| s.width_full().apply_if(!previewing(), |s| s.hide()));
    v_stack((
        form.style(move |s| s.apply_if(previewing(), |s| s.hide())),
        preview_panel,
    ))
    .style(move |s| {
        s.width_full()
            .flex_col()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

fn decision_button(
    choice: ReviewDecision,
    decision: RwSignal<ReviewDecision>,
    config: floem::reactive::ReadSignal<Arc<crate::config::LapceConfig>>,
) -> impl View {
    label(move || decision_label(choice))
        .on_click_stop(move |_| decision.set(choice))
        .style(move |s| {
            let config = config.get();
            let active = decision.get() == choice;
            s.padding_horiz(8.0)
                .padding_vert(2.0)
                .margin_right(4.0)
                .border(1.0)
                .border_radius(4.0)
                .cursor(CursorStyle::Pointer)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .apply_if(active, |s| {
                    s.background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
                        .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
                })
        })
}

/// What will be posted to GitHub, every part editable: the review's body and each line comment. Nothing is submitted
/// or posted until **Post to GitHub**; **Back** returns to the form.
#[allow(clippy::too_many_arguments)]
fn preview_view(
    window_tab_data: Rc<WindowTabData>,
    tab: ReviewTab,
    shown: PostPreview,
    decision: ReviewDecision,
    summary: String,
    preview: RwSignal<Option<PostPreview>>,
    note: RwSignal<Option<(String, Option<String>, bool)>>,
    busy: RwSignal<bool>,
) -> impl View {
    let config = window_tab_data.common.config;
    // As GitHub will take it: on your own pull request, a comment that says the decision (the note says why).
    let event = if shown.preview.event == "COMMENT" { ReviewDecision::Comment } else { decision };
    let heading = format!(
        "Post to {}#{} as \"{}\"{}",
        shown.target.repo,
        shown.target.number,
        decision_label(event),
        if shown.preview.comments.is_empty() {
            String::new()
        } else {
            format!(
                ", with {} line comment{}",
                shown.preview.comments.len(),
                if shown.preview.comments.len() == 1 { "" } else { "s" }
            )
        }
    );
    let body = shown.body;
    let comments: Vec<(String, String, RwSignal<String>)> = shown
        .preview
        .comments
        .iter()
        .zip(shown.comments.iter())
        .map(|(comment, (thread_id, text))| {
            let line = match comment.start_line {
                Some(start) => format!("{start}-{}", comment.line),
                None => comment.line.to_string(),
            };
            let side = if comment.side == "LEFT" { " (old side)" } else { "" };
            (thread_id.clone(), format!("{}:{line}{side}", comment.path), *text)
        })
        .collect();
    let edits = comments.clone();
    let confirm = move || {
        if busy.get_untracked() {
            return;
        }
        let Some(review) = tab.review.active_review_untracked() else {
            return;
        };
        let edits = GitHubEdits {
            body: body.get_untracked(),
            comments: edits
                .iter()
                .map(|(thread_id, _, text)| (thread_id.clone(), text.get_untracked()))
                .collect(),
        };
        busy.set(true);
        preview.set(None);
        submit_review(
            &tab,
            review.id.clone(),
            decision,
            summary.clone(),
            Some((shown.target.clone(), edits)),
            note,
            busy,
        );
    };
    let dim = move || config.get().color(LapceColor::PANEL_FOREGROUND_DIM);
    let own_note = shown.preview.note.clone();
    let has_note = own_note.is_some();
    v_stack((
        label(move || heading.clone()).style(|s| s.font_bold().margin_bottom(6.0)),
        label(move || own_note.clone().unwrap_or_default())
            .style(move |s| s.color(dim()).margin_bottom(6.0).apply_if(!has_note, |s| s.hide())),
        label(|| "The review's message").style(move |s| s.color(dim())),
        reply_box(
            window_tab_data.clone(),
            "The review's message on GitHub",
            body.get_untracked(),
            |_| {},
            move |text| body.set(text),
        ),
        dyn_stack(
            move || comments.clone(),
            |(thread_id, _, _)| thread_id.clone(),
            move |(_, place, text)| {
                v_stack((
                    label(move || place.clone()).style(move |s| s.color(dim()).margin_top(6.0)),
                    reply_box(
                        window_tab_data.clone(),
                        "The line comment",
                        text.get_untracked(),
                        |_| {},
                        move |edited| text.set(edited),
                    ),
                ))
                .style(|s| s.width_full().flex_col())
            },
        )
        .style(|s| s.width_full().flex_col()),
        h_stack((
            empty().style(|s| s.flex_grow(1.0_f32)),
            link("Back", config, move || preview.set(None)),
            text_button("Post to GitHub".to_string(), true, confirm, config),
        ))
        .style(|s| s.width_full().items_center().margin_top(8.0)),
    ))
    .style(|s| s.width_full().flex_col())
}

/// Submits the verdict (the agent gets it with every open comment), then posts it to GitHub when asked.
fn submit_review(
    tab: &ReviewTab,
    review_id: String,
    decision: ReviewDecision,
    summary: String,
    github: Option<(GitHubTarget, GitHubEdits)>,
    note: RwSignal<Option<(String, Option<String>, bool)>>,
    busy: RwSignal<bool>,
) {
    let Some(core) = ReviewCore::get() else {
        busy.set(false);
        return;
    };
    let (store, runner) = (Arc::clone(&core.services.store), Arc::clone(&core.services.github));
    let user = review_store::current_user(tab.review.root.as_deref().unwrap_or(Path::new(".")));
    let submitting = tab.review.submitting;
    let posting = github.as_ref().map(|(target, _)| format!("Posting to {}#{}…", target.repo, target.number));
    note.set(posting.map(|text| (text, None, false)));
    tab.later(
        move || -> Result<Option<(String, Option<String>)>, String> {
            let review = store
                .submit(&review_id, decision, summary.clone(), user)
                .map_err(|error| format!("Could not submit the review: {error}"))?;
            let Some((target, edits)) = github else {
                return Ok(None);
            };
            let posted = review_github::publish(
                runner.as_ref(),
                &review,
                &target,
                decision,
                &summary,
                &HashMap::new(),
                Some(&edits),
            )
            .map_err(|error| format!("{error:#}"))?;
            let round = review.verdict.as_ref().map_or(0, |verdict| verdict.count);
            if let Some(bundle) = store.get(&review_id).and_then(|review| review.bundle) {
                let bundle = ReviewBundle {
                    github: Some(review_model::BundleGitHub {
                        url: posted.url.clone(),
                        round,
                        posted_at: chrono::Utc::now().timestamp_millis().max(0) as u64,
                    }),
                    ..bundle
                };
                if let Err(error) = store.set_bundle(&review_id, Some(bundle)) {
                    tracing::error!("could not record the GitHub post: {error}");
                }
            }
            let mut text = format!(
                "Posted to GitHub: {} line comment{}{}.",
                posted.comments,
                if posted.comments == 1 { "" } else { "s" },
                if posted.in_body > 0 {
                    format!(", {} in the body", posted.in_body)
                } else {
                    String::new()
                }
            );
            for extra in &posted.notes {
                text.push(' ');
                text.push_str(extra);
            }
            Ok(Some((text, Some(posted.url))))
        },
        move |result| {
            busy.set(false);
            match result {
                Ok(posted) => {
                    submitting.set(false);
                    note.set(posted.map(|(text, url)| (text, url, false)));
                }
                Err(error) => {
                    submitting.set(false);
                    note.set(Some((error, None, true)));
                }
            }
        },
    );
}
