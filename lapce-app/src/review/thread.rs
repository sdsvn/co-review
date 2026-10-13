//! The shared thread view: a review thread's messages (author, role, Markdown body), the agent's streaming answer
//! with its steps and tool activity, permission requests as buttons, **Stop**, the status line ("waiting for
//! <agent>", "<agent> is on it"), resolve and reopen, accept and dismiss for proposed findings and suggested edits,
//! and the reply box. Used inline in editors (`review::inline`), in the Review panel and on review pages.
//!
//! The view derives from the window tab's reviews (`ReviewWorkspaceData::reviews`), which follow the store; the
//! agent's streaming answer arrives the same way (the agent service writes it into the store as it streams). Every
//! store write runs off the UI thread ([`run`]).

use std::{path::PathBuf, rc::Rc, sync::Arc};

use floem::{
    IntoView, View,
    event::{Event, EventListener},
    keyboard::Modifiers,
    kurbo::Rect,
    reactive::{
        Memo, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_memo,
        create_rw_signal,
    },
    style::CursorStyle,
    views::{
        Decorators, dyn_container, dyn_stack, empty, h_stack, label,
        rich_text, v_stack, v_stack_from_iter,
    },
};
use lapce_core::{command::FocusCommand, mode::Mode};
use review_agents::{AgentService, answers};
use review_mcp::Services;
use review_model::{
    AgentActivity, AgentState, AnchorStatus, MessageStatus, ParticipantKind,
    PermissionRequest, Proposal, ProposalStatus, Review, ReviewMessage,
    ReviewThread, ThreadIntent, ThreadStatus, ToolStatus,
};

use super::ReviewCore;
use crate::{
    command::{CommandExecuted, CommandKind, InternalCommand, LapceCommand},
    config::{LapceConfig, color::LapceColor},
    editor::{
        EditorData,
        location::{EditorLocation, EditorPosition},
    },
    keypress::{KeyPressFocus, condition::Condition},
    markdown::{MarkdownContent, parse_markdown},
    text_input::TextInputBuilder,
    window_tab::WindowTabData,
};

/// How a thread is shown.
#[derive(Clone, Default)]
pub struct ThreadViewOptions {
    /// Inline in an editor: always expanded; `on_hide` folds it back to its gutter icon.
    pub inline: bool,
    /// Called by the thread's collapse button (inline) — the caller decides what folding means.
    pub on_hide: Option<Rc<dyn Fn()>>,
    /// Called when the thread's location label is clicked (panel: open the thread at its code).
    pub on_open: Option<Rc<dyn Fn()>>,
}

/// A review thread, live: it follows the store until the thread is gone (then it shows nothing).
pub fn thread_view(
    window_tab_data: Rc<WindowTabData>,
    review_id: String,
    thread_id: String,
    options: ThreadViewOptions,
) -> impl View {
    let config = window_tab_data.common.config;
    let reviews = window_tab_data.review.reviews;
    let listening = ReviewCore::get().map(|core| core.listening);
    let found = {
        let (review_id, thread_id) = (review_id.clone(), thread_id.clone());
        create_memo(move |_| {
            let listening = listening.is_some_and(|listening| {
                listening.with(|listening| listening.get(&review_id).copied().unwrap_or(false))
            });
            reviews.with(|reviews| {
                let review = reviews.iter().find(|review| review.id == review_id)?;
                let thread = review
                    .threads
                    .iter()
                    .find(|thread| thread.id == thread_id)?
                    .clone();
                Some(ThreadContext {
                    agent_name: review.agent.as_ref().map(|agent| agent.name.clone()),
                    link: review.agent.as_ref().map(|agent| agent_link(agent, listening)),
                    stand_in: review.agent.as_ref().and_then(acp_stand_in),
                    has_agent: review.agent.is_some(),
                    agent_ids: review
                        .agent
                        .as_ref()
                        .map(|agent| vec![agent.id.clone(), format!("agent:{}", agent.id)])
                        .unwrap_or_default(),
                    thread,
                })
            })
        })
    };
    let thread = create_memo(move |_| found.with(|found| found.as_ref().map(|f| f.thread.clone())));
    let expanded = create_rw_signal(true);
    let ids = (review_id, thread_id);

    let header = thread_header(
        window_tab_data.clone(),
        ids.clone(),
        found,
        expanded,
        options.clone(),
    );
    let body = {
        let window_tab_data = window_tab_data.clone();
        let ids = ids.clone();
        dyn_container(
            move || expanded.get() || options.inline,
            move |shown| {
                if shown {
                    thread_body(window_tab_data.clone(), ids.clone(), thread, found)
                        .into_any()
                } else {
                    preview(thread, config).into_any()
                }
            },
        )
        .style(|s| s.width_full())
    };
    v_stack((header, body))
        .style(move |s| {
            let config = config.get();
            let resolved = thread
                .with(|thread| thread.as_ref().map(|t| t.status == ThreadStatus::Resolved))
                .unwrap_or(false);
            s.width_full()
                .flex_col()
                .padding(8.0)
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_BACKGROUND))
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .apply_if(resolved, |s| s.color(config.color(LapceColor::EDITOR_DIM)))
                .apply_if(thread.with(|t| t.is_none()), |s| s.hide())
        })
        .debug_name("Review Thread")
}

/// What the header needs besides the thread: the review's agent.
#[derive(Clone, PartialEq)]
struct ThreadContext {
    thread: ReviewThread,
    agent_name: Option<String>,
    /// How the review's agent answers.
    link: Option<AgentLink>,
    /// The ACP agent that can answer here instead of the review's MCP agent.
    stand_in: Option<review_model::AgentConfig>,
    has_agent: bool,
    /// The ids the review's agent writes as (`<id>` over MCP, `agent:<id>` over ACP).
    agent_ids: Vec<String>,
}

type Ids = (String, String);

fn thread_header(
    window_tab_data: Rc<WindowTabData>,
    ids: Ids,
    found: Memo<Option<ThreadContext>>,
    expanded: RwSignal<bool>,
    options: ThreadViewOptions,
) -> impl View {
    let config = window_tab_data.common.config;
    let get = move |f: &dyn Fn(&ThreadContext) -> Option<String>| {
        found.with(|found| found.as_ref().and_then(f))
    };
    let toggle = (!options.inline).then(|| {
        label(move || if expanded.get() { "▾" } else { "▸" })
            .on_click_stop(move |_| expanded.update(|expanded| *expanded = !*expanded))
            .style(|s| s.padding_right(4.0).cursor(CursorStyle::Pointer).selectable(false))
    });
    let number = label(move || {
        found
            .with(|found| found.as_ref().map(|f| format!("#{}", f.thread.number)))
            .unwrap_or_default()
    })
    .style(move |s| s.font_bold().margin_right(6.0));
    let location = {
        let on_open = options.on_open.clone();
        let clickable = on_open.is_some();
        label(move || get(&|f| Some(location_label(&f.thread))).unwrap_or_default())
            .on_click_stop(move |_| {
                if let Some(on_open) = &on_open {
                    on_open();
                }
            })
            .style(move |s| {
                let config = config.get();
                s.margin_right(6.0)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .apply_if(clickable, |s| {
                        s.cursor(CursorStyle::Pointer)
                            .hover(|s| s.color(config.color(LapceColor::EDITOR_LINK)))
                    })
                    .apply_if(options.inline, |s| s.hide())
            })
    };
    let symbol = badge(
        move || get(&|f| f.thread.location.symbol.clone()),
        LapceColor::EDITOR_DIM,
        config,
    );
    let question = badge(
        move || {
            get(&|f| (f.thread.intent == ThreadIntent::Question).then(|| "question".to_string()))
        },
        LapceColor::EDITOR_LINK,
        config,
    );
    let severity = badge(
        move || {
            get(&|f| {
                f.thread.severity.map(|severity| {
                    serde_json::to_value(severity)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default()
                })
            })
        },
        LapceColor::LAPCE_WARN,
        config,
    );
    let labels = badge(
        move || get(&|f| f.thread.labels.as_ref().map(|labels| labels.join(" · ")).filter(|l| !l.is_empty())),
        LapceColor::EDITOR_DIM,
        config,
    );
    let anchor = badge(
        move || get(&|f| anchor_state(&f.thread)),
        LapceColor::LAPCE_WARN,
        config,
    );
    let status = badge(
        move || get(&|f| status_line(&f.thread, f.agent_name.as_deref(), f.link)),
        LapceColor::EDITOR_LINK,
        config,
    );
    let state = badge(
        move || {
            get(&|f| match f.thread.status {
                ThreadStatus::Resolved => Some("resolved".to_string()),
                ThreadStatus::Proposed => Some("proposed finding".to_string()),
                ThreadStatus::Open => None,
            })
        },
        LapceColor::EDITOR_DIM,
        config,
    );

    let actions = {
        let window_tab_data = window_tab_data.clone();
        let ids = ids.clone();
        let on_hide = options.on_hide.clone();
        dyn_container(
            move || {
                found.with(|found| {
                    found.as_ref().map(|f| {
                        let busy = matches!(
                            f.thread.agent_state,
                            Some(AgentState::Working | AgentState::WaitingForHuman)
                        );
                        let engaged = f.thread.messages.iter().any(|message| {
                            f.agent_ids.contains(&message.author.id)
                        });
                        let resolved = f.thread.status == ThreadStatus::Resolved;
                        // Already asked: it waits for the agent (asking again would do nothing).
                        let queued = f.thread.agent_state == Some(AgentState::Queued);
                        HeaderActions {
                            status: f.thread.status,
                            can_ask: f.has_agent && !resolved && !busy && !engaged && !queued,
                            agent_name: f.agent_name.clone().unwrap_or_default(),
                            link: f.link,
                            stand_in: f
                                .stand_in
                                .clone()
                                .filter(|_| !resolved && queued && f.link == Some(AgentLink::Away)),
                        }
                    })
                })
            },
            move |actions| {
                let Some(actions) = actions else {
                    return empty().into_any();
                };
                let mut buttons: Vec<Box<dyn View>> = Vec::new();
                if let Some(stand_in) = actions.stand_in.clone() {
                    let ids = ids.clone();
                    let window_tab_data = window_tab_data.clone();
                    let tip = format!(
                        "Start {} here over ACP and ask it now. It becomes the review's agent; {} can rejoin from its session with open_review.",
                        stand_in.name, actions.agent_name
                    );
                    buttons.push(Box::new(crate::app::tooltip_label(
                        config,
                        text_button(
                            format!("Ask now with {} (ACP)", stand_in.name),
                            true,
                            move || {
                                answer_here(&window_tab_data, &ids.0, stand_in.clone(), Some(ids.1.clone()))
                            },
                            config,
                        ),
                        move || tip.clone(),
                    )));
                }
                if actions.can_ask {
                    let ids = ids.clone();
                    let window_tab_data = window_tab_data.clone();
                    let link = actions.link.unwrap_or(AgentLink::Here);
                    let tip = ask_tooltip(&actions.agent_name, link, actions.stand_in.is_some());
                    buttons.push(Box::new(crate::app::tooltip_label(
                        config,
                        text_button(
                            ask_label(&actions.agent_name, link),
                            false,
                            move || ask_agent(&window_tab_data, &ids.0, &ids.1),
                            config,
                        ),
                        move || tip.clone(),
                    )));
                }
                let set_status = |status: ThreadStatus, text: &str, primary: bool| {
                    let ids = ids.clone();
                    let window_tab_data = window_tab_data.clone();
                    Box::new(text_button(
                        text.to_string(),
                        primary,
                        move || set_thread_status(&window_tab_data, &ids.0, &ids.1, status),
                        config,
                    )) as Box<dyn View>
                };
                match actions.status {
                    ThreadStatus::Proposed => {
                        buttons.push(set_status(ThreadStatus::Open, "Accept", true));
                        buttons.push(set_status(ThreadStatus::Resolved, "Dismiss", false));
                    }
                    ThreadStatus::Resolved => {
                        buttons.push(set_status(ThreadStatus::Open, "Reopen", false))
                    }
                    ThreadStatus::Open => {
                        buttons.push(set_status(ThreadStatus::Resolved, "Resolve", false))
                    }
                }
                if let Some(on_hide) = on_hide.clone() {
                    buttons.push(Box::new(
                        label(|| "▴")
                            .on_click_stop(move |_| on_hide())
                            .style(move |s| {
                                s.padding_horiz(6.0)
                                    .cursor(CursorStyle::Pointer)
                                    .selectable(false)
                                    .color(config.get().color(LapceColor::EDITOR_DIM))
                            }),
                    ));
                }
                h_stack_boxed(buttons).into_any()
            },
        )
    };

    let mut items: Vec<Box<dyn View>> = Vec::new();
    if let Some(toggle) = toggle {
        items.push(Box::new(toggle));
    }
    items.push(Box::new(number));
    items.push(Box::new(location));
    items.push(Box::new(symbol));
    items.push(Box::new(question));
    items.push(Box::new(severity));
    items.push(Box::new(labels));
    items.push(Box::new(anchor));
    items.push(Box::new(status));
    items.push(Box::new(state));
    items.push(Box::new(empty().style(|s| s.flex_grow(1.0_f32))));
    items.push(Box::new(actions));
    h_stack_boxed(items).style(|s| s.width_full().items_center().flex_wrap(floem::style::FlexWrap::Wrap))
}

#[derive(Clone, PartialEq)]
struct HeaderActions {
    status: ThreadStatus,
    can_ask: bool,
    agent_name: String,
    link: Option<AgentLink>,
    /// Offered while the review's MCP agent is away.
    stand_in: Option<review_model::AgentConfig>,
}

/// The first message, one line, for a folded thread.
fn preview(
    thread: Memo<Option<ReviewThread>>,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    label(move || {
        thread
            .with(|thread| {
                thread.as_ref().map(|thread| {
                    let first = thread
                        .messages
                        .first()
                        .map(|message| message.body.lines().next().unwrap_or("").to_string())
                        .unwrap_or_default();
                    match thread.messages.len() {
                        0 | 1 => first,
                        count => format!("{first}  (+{})", count - 1),
                    }
                })
            })
            .unwrap_or_default()
    })
    .style(move |s| {
        s.margin_top(4.0)
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .text_ellipsis()
            .max_width_full()
    })
}

fn thread_body(
    window_tab_data: Rc<WindowTabData>,
    ids: Ids,
    thread: Memo<Option<ReviewThread>>,
    found: Memo<Option<ThreadContext>>,
) -> impl View {
    let config = window_tab_data.common.config;
    let proposal = {
        let window_tab_data = window_tab_data.clone();
        let ids = ids.clone();
        dyn_container(
            move || thread.with(|thread| thread.as_ref().and_then(|t| t.proposal.clone())),
            move |proposal| match proposal {
                Some(proposal) => {
                    proposal_view(window_tab_data.clone(), ids.clone(), proposal).into_any()
                }
                None => empty().into_any(),
            },
        )
    };
    let messages = {
        let window_tab_data = window_tab_data.clone();
        dyn_stack(
            move || {
                thread
                    .with(|thread| thread.as_ref().map(|t| t.messages.clone()))
                    .unwrap_or_default()
            },
            |message| {
                (
                    message.id.clone(),
                    message.body.len(),
                    format!("{:?}", message.status),
                    message.activity.as_ref().map(|activity| {
                        activity
                            .iter()
                            .map(|a| format!("{}:{:?}", a.id, a.status))
                            .collect::<Vec<_>>()
                    }),
                    message
                        .permission
                        .as_ref()
                        .map(|p| (p.id.clone(), p.outcome.clone())),
                )
            },
            move |message| message_view(window_tab_data.clone(), message),
        )
        .style(|s| s.flex_col().width_full())
    };
    let stop = {
        let ids = ids.clone();
        let window_tab_data = window_tab_data.clone();
        text_button(
            "Stop".to_string(),
            false,
            move || stop_agent(&window_tab_data, &ids.0, &ids.1),
            config,
        )
        .style(move |s| {
            let busy = found.with(|found| {
                found.as_ref().is_some_and(|f| {
                    matches!(
                        f.thread.agent_state,
                        Some(AgentState::Working | AgentState::WaitingForHuman)
                    ) || f
                        .thread
                        .messages
                        .last()
                        .is_some_and(|m| m.status == Some(MessageStatus::Streaming))
                })
            });
            s.margin_top(4.0).apply_if(!busy, |s| s.hide())
        })
    };
    let reply = {
        let resolved = create_memo(move |_| {
            thread.with(|t| t.as_ref().is_some_and(|t| t.status == ThreadStatus::Resolved))
        });
        let window_tab_data = window_tab_data.clone();
        let ids = ids.clone();
        reply_box(
            window_tab_data.clone(),
            "Reply… (Enter to send)",
            String::new(),
            move |body| reply(&window_tab_data, &ids.0, &ids.1, body),
            |_| {},
        )
        .style(move |s| s.margin_top(6.0).apply_if(resolved.get(), |s| s.hide()))
    };
    v_stack((proposal, messages, stop, reply)).style(|s| s.width_full().flex_col())
}

fn message_view(window_tab_data: Rc<WindowTabData>, message: ReviewMessage) -> impl View {
    let config = window_tab_data.common.config;
    let streaming = message.status == Some(MessageStatus::Streaming);
    let role = match message.author.kind {
        ParticipantKind::Agent => "agent",
        ParticipantKind::System => "system",
        ParticipantKind::Human => "",
    };
    let state = match message.status {
        Some(MessageStatus::Streaming) => "working…",
        Some(MessageStatus::Cancelled) => "stopped",
        Some(MessageStatus::Error) => "error",
        _ => "",
    };
    let author = message.author.name.clone();
    let time = format_time(message.created_at);
    // Agents' messages are marked in the agent colour (the themes' `review.agent`).
    let agent = message.author.kind == ParticipantKind::Agent;
    let marked = move |dim: bool| {
        let config = config.get();
        if agent {
            config.color(super::theme::AGENT_COLOR)
        } else if dim {
            config.color(LapceColor::EDITOR_DIM)
        } else {
            config.color(LapceColor::EDITOR_FOREGROUND)
        }
    };
    let meta = h_stack((
        label(move || author.clone())
            .style(move |s| s.font_bold().margin_right(6.0).color(marked(false))),
        label(move || role).style(move |s| {
            s.margin_right(6.0)
                .color(marked(true))
                .apply_if(role.is_empty(), |s| s.hide())
        }),
        label(move || state).style(move |s| {
            s.margin_right(6.0)
                .color(config.get().color(LapceColor::EDITOR_LINK))
                .apply_if(state.is_empty(), |s| s.hide())
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || time.clone())
            .style(move |s| s.color(config.get().color(LapceColor::EDITOR_DIM))),
    ))
    .style(|s| s.width_full().items_center());

    let activity = activity_view(message.activity.clone().unwrap_or_default(), streaming, config);
    let text = message.body.clone();
    let body = (!text.trim().is_empty()).then(|| markdown_view(window_tab_data.clone(), &text));
    let permission = message
        .permission
        .clone()
        .map(|request| permission_view(window_tab_data.clone(), request));

    let mut items: Vec<Box<dyn View>> = vec![Box::new(meta), Box::new(activity)];
    if let Some(body) = body {
        items.push(Box::new(body));
    }
    if let Some(permission) = permission {
        items.push(Box::new(permission));
    }
    v_stack_boxed(items).style(move |s| {
        s.width_full()
            .flex_col()
            .margin_top(6.0)
            .padding_top(6.0)
            .border_top(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

/// The agent's steps: the last four while it streams, folded to "N steps" once done.
fn activity_view(
    activity: Vec<AgentActivity>,
    streaming: bool,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    if activity.is_empty() {
        return empty().into_any();
    }
    let open = create_rw_signal(false);
    let count = activity.len();
    let failed = activity.iter().filter(|a| a.status == ToolStatus::Failed).count();
    let summary = label(move || {
        let arrow = if open.get() { "▾" } else { "▸" };
        let steps = if count == 1 { "step" } else { "steps" };
        if failed > 0 {
            format!("{arrow} {count} {steps} · {failed} failed")
        } else {
            format!("{arrow} {count} {steps}")
        }
    })
    .on_click_stop(move |_| open.update(|open| *open = !*open))
    .style(move |s| {
        s.cursor(CursorStyle::Pointer)
            .selectable(false)
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .apply_if(streaming, |s| s.hide())
    });
    let earlier = count.saturating_sub(4);
    let more = label(move || format!("… {earlier} earlier steps")).style(move |s| {
        s.color(config.get().color(LapceColor::EDITOR_DIM))
            .apply_if(!streaming || earlier == 0, |s| s.hide())
    });
    let shown: Vec<AgentActivity> = if streaming {
        activity[earlier..].to_vec()
    } else {
        activity
    };
    let lines = v_stack_from_iter(shown.into_iter().map(move |activity| {
        let icon = match activity.status {
            ToolStatus::Pending => "○",
            ToolStatus::InProgress => "◌",
            ToolStatus::Completed => "✓",
            ToolStatus::Failed => "✗",
        };
        let title = activity.title.clone();
        label(move || format!("{icon} {title}")).style(move |s| {
            s.color(config.get().color(LapceColor::EDITOR_DIM))
                .text_ellipsis()
                .max_width_full()
        })
    }))
    .style(move |s| s.flex_col().apply_if(!streaming && !open.get(), |s| s.hide()));
    v_stack((summary, more, lines))
        .style(|s| s.flex_col().margin_top(2.0))
        .into_any()
}

fn permission_view(window_tab_data: Rc<WindowTabData>, request: PermissionRequest) -> impl View {
    let config = window_tab_data.common.config;
    if let Some(outcome) = &request.outcome {
        let chosen = request.options.iter().find(|option| option.id == *outcome);
        let text = match chosen {
            Some(option) => format!("{}: {}", option.name, request.title),
            None => format!("Cancelled: {}", request.title),
        };
        return label(move || text.clone())
            .style(move |s| s.margin_top(4.0).color(config.get().color(LapceColor::EDITOR_DIM)))
            .into_any();
    }
    let title = format!("The agent asks: {}", request.title);
    let buttons = request.options.iter().map(|option| {
        let (request_id, option_id) = (request.id.clone(), option.id.clone());
        let allow = option.kind.starts_with("allow");
        let window_tab_data = window_tab_data.clone();
        text_button(
            option.name.clone(),
            allow,
            move || answer_permission(&window_tab_data, &request_id, &option_id),
            config,
        )
        .into_any()
    });
    v_stack((
        label(move || title.clone()).style(|s| s.font_bold()),
        floem::views::h_stack_from_iter(buttons).style(|s| s.margin_top(4.0).gap(6.0)),
    ))
    .style(move |s| {
        s.flex_col()
            .margin_top(6.0)
            .padding(6.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(config.get().color(LapceColor::LAPCE_WARN))
    })
    .into_any()
}

/// A suggested edit as before/after lines, with Accept edit / Reject while pending.
fn proposal_view(window_tab_data: Rc<WindowTabData>, ids: Ids, proposal: Proposal) -> impl View {
    let config = window_tab_data.common.config;
    let place = match (&proposal.path, proposal.start_line) {
        (Some(path), Some(line)) => format!(" · {path}:{line}"),
        (Some(path), None) => format!(" · {path}"),
        _ => String::new(),
    };
    let head_text = format!("Suggested edit{place}");
    let decide = |accept: bool| {
        let (window_tab_data, ids) = (window_tab_data.clone(), ids.clone());
        move || decide_proposal(&window_tab_data, &ids.0, &ids.1, accept)
    };
    let decision = if proposal.status == ProposalStatus::Pending {
        h_stack((
            text_button("Reject".to_string(), false, decide(false), config),
            text_button("Accept edit".to_string(), true, decide(true), config),
        ))
        .style(|s| s.gap(6.0))
        .into_any()
    } else {
        let status = if proposal.status == ProposalStatus::Accepted {
            "accepted"
        } else {
            "rejected"
        };
        label(move || status)
            .style(move |s| s.color(config.get().color(LapceColor::EDITOR_DIM)))
            .into_any()
    };
    let lines = |text: &str, sign: char| {
        text.split('\n')
            .map(|line| format!("{sign} {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let before = lines(&proposal.before, '−');
    let after = lines(&proposal.after, '+');
    let code = move |text: String, color: &'static str| {
        label(move || text.clone()).style(move |s| {
            let config = config.get();
            s.font_family(config.editor.font_family.clone())
                .color(config.color(color))
                .width_full()
        })
    };
    v_stack((
        h_stack((
            label(move || head_text.clone()).style(|s| s.font_bold()),
            empty().style(|s| s.flex_grow(1.0_f32)),
            decision,
        ))
        .style(|s| s.width_full().items_center()),
        code(before, LapceColor::SOURCE_CONTROL_REMOVED),
        code(after, LapceColor::SOURCE_CONTROL_ADDED),
    ))
    .style(move |s| {
        s.flex_col()
            .width_full()
            .margin_top(6.0)
            .padding(6.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

/// A message body as Markdown (Lapce's renderer). `path:line`, `path#L10-L20` and `/page.md` in it open the code or
/// page when clicked.
pub fn markdown_view(window_tab_data: Rc<WindowTabData>, body: &str) -> impl View + use<> {
    let config = window_tab_data.common.config;
    let contents = parse_markdown(body, 1.5, &config.get_untracked());
    v_stack_from_iter(contents.into_iter().map(move |content| match content {
        MarkdownContent::Text(layout) => {
            let width = create_rw_signal(0.0f64);
            let window_tab_data = window_tab_data.clone();
            let hit_layout = layout.clone();
            rich_text(move || layout.clone())
                .on_resize(move |rect: Rect| width.set(rect.width()))
                .on_event_cont(EventListener::PointerUp, move |event| {
                    let Event::PointerUp(pointer) = event else {
                        return;
                    };
                    let mut layout = hit_layout.clone();
                    layout.set_size(width.get_untracked() as f32, f32::MAX);
                    if let Some(link) = link_at(&layout, pointer.pos.x, pointer.pos.y) {
                        open_link(&window_tab_data, &link);
                    }
                })
                .style(|s| s.max_width_pct(100.0))
                .into_any()
        }
        MarkdownContent::Image { .. } => empty().into_any(),
        MarkdownContent::Separator => empty()
            .style(move |s| {
                s.width_full()
                    .margin_vert(4.0)
                    .height(1.0)
                    .background(config.get().color(LapceColor::LAPCE_BORDER))
            })
            .into_any(),
    }))
    .style(|s| s.flex_col().width_full().margin_top(4.0))
}

/// A link in a message: code (`path:line`, `path:10-20`, `path#L10-L20`), a page (`/x.md`) or a web address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageLink {
    Code {
        path: String,
        line: Option<u32>,
        end_line: Option<u32>,
    },
    Web(String),
}

/// The link under a point of a laid-out text, if the word there is one.
fn link_at(layout: &floem::text::TextLayout, x: f64, y: f64) -> Option<MessageLink> {
    let cursor = layout.hit(x as f32, y as f32)?;
    let text = layout.lines().get(cursor.line)?.text();
    let index = cursor.index.min(text.len());
    let boundary = |c: char| c.is_whitespace() || "()[]<>\"'`,;".contains(c);
    let start = text[..index]
        .char_indices()
        .rev()
        .find(|(_, c)| boundary(*c))
        .map_or(0, |(at, c)| at + c.len_utf8());
    let end = text[index..]
        .char_indices()
        .find(|(_, c)| boundary(*c))
        .map_or(text.len(), |(at, _)| index + at);
    parse_link(text.get(start..end)?)
}

/// Parses a word of a message as a link.
pub fn parse_link(word: &str) -> Option<MessageLink> {
    let word = word.trim_end_matches(['.', ':', '!', '?']);
    if word.starts_with("http://") || word.starts_with("https://") {
        return Some(MessageLink::Web(word.to_string()));
    }
    let is_path = |path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        !path.is_empty()
            && !path.contains("://")
            && name.contains('.')
            && name
                .rsplit('.')
                .next()
                .is_some_and(|ext| ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
            && path
                .chars()
                .all(|c| c.is_alphanumeric() || "/._-@+~".contains(c))
    };
    let number = |text: &str| text.parse::<u32>().ok().filter(|n| *n > 0);
    // path#L10-L20
    if let Some((path, anchor)) = word.split_once("#L") {
        if !is_path(path) {
            return None;
        }
        let (start, end) = match anchor.split_once('-') {
            Some((start, end)) => (number(start), number(end.trim_start_matches('L'))),
            None => (number(anchor), None),
        };
        return Some(MessageLink::Code {
            path: path.to_string(),
            line: start,
            end_line: end,
        });
    }
    // path:10 or path:10-20
    if let Some((path, lines)) = word.split_once(':') {
        if !is_path(path) {
            return None;
        }
        let (start, end) = match lines.split_once('-') {
            Some((start, end)) => (number(start), number(end)),
            None => (number(lines), None),
        };
        start?;
        return Some(MessageLink::Code {
            path: path.to_string(),
            line: start,
            end_line: end,
        });
    }
    // /docs/x.md: a page of the repository
    if word.starts_with('/') && word.ends_with(".md") && is_path(word) {
        return Some(MessageLink::Code {
            path: word.to_string(),
            line: None,
            end_line: None,
        });
    }
    None
}

/// Opens a link: code at its line in the editor (paths relative to the workspace, or absolute when they exist), a
/// page as its file, a web address in the browser.
pub fn open_link(window_tab_data: &WindowTabData, link: &MessageLink) {
    match link {
        MessageLink::Web(url) => {
            if let Err(error) = open::that(url) {
                tracing::error!("could not open {url}: {error}");
            }
        }
        MessageLink::Code { path, line, .. } => {
            let Some(path) = resolve_path(window_tab_data, path) else {
                tracing::warn!("no file {path} in the workspace");
                return;
            };
            open_location(window_tab_data, path, line.map(|line| line.saturating_sub(1) as usize));
        }
    }
}

/// Opens `path` in the editor, at the 0-based `line`.
pub fn open_location(window_tab_data: &WindowTabData, path: PathBuf, line: Option<usize>) {
    window_tab_data
        .common
        .internal_command
        .send(InternalCommand::JumpToLocation {
            location: EditorLocation {
                path,
                position: line.map(EditorPosition::Line),
                scroll_offset: None,
                ignore_unconfirmed: false,
                same_editor_tab: false,
            },
        });
}

fn resolve_path(window_tab_data: &WindowTabData, path: &str) -> Option<PathBuf> {
    let root = window_tab_data.review.root.clone();
    let relative = path.trim_start_matches("./");
    let candidates = [
        root.as_ref().map(|root| root.join(relative.trim_start_matches('/'))),
        Some(PathBuf::from(path)),
    ];
    candidates.into_iter().flatten().find(|candidate| candidate.is_file())
}

/// A single-line input for replies and drafts: Enter calls `on_submit` with the text (and clears it), Escape calls
/// `on_escape` with the text.
pub fn reply_box(
    window_tab_data: Rc<WindowTabData>,
    placeholder: &'static str,
    initial: String,
    on_submit: impl Fn(String) + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl View {
    let common = window_tab_data.common.clone();
    let editors = window_tab_data.main_split.editors;
    let config = common.config;
    let cx = Scope::current();
    let editor = editors.make_local(cx, common);
    let id = editor.id();
    if !initial.is_empty() {
        editor.doc().reload(lapce_xi_rope::Rope::from(initial.as_str()), true);
    }
    {
        let doc = editor.doc();
        let buffer = doc.buffer;
        let last = create_rw_signal(initial);
        cx.create_effect(move |_| {
            let text = buffer.with(|buffer| buffer.to_string());
            if text != last.get_untracked() {
                last.set(text.clone());
                on_change(text);
            }
        });
    }
    let keys = ReplyKeys {
        editor: editor.clone(),
        on_submit: Rc::new(on_submit),
    };
    TextInputBuilder::new()
        .key_focus(keys)
        .build_editor(editor)
        .placeholder(move || placeholder.to_string())
        // Floem moves keyboard focus to a clicked view only if it is keyboard-navigable; without this a click
        // placed the caret here but typing kept going to the code editor.
        .keyboard_navigable()
        .on_cleanup(move || {
            editors.remove(id);
        })
        .style(move |s| {
            let config = config.get();
            s.width_full()
                .border(1.0)
                .border_radius(4.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::EDITOR_BACKGROUND))
        })
}

/// Keys of a reply box: editing goes to its editor, Enter submits.
#[derive(Clone)]
struct ReplyKeys {
    editor: EditorData,
    on_submit: Rc<dyn Fn(String)>,
}

impl std::fmt::Debug for ReplyKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplyKeys").finish_non_exhaustive()
    }
}

impl KeyPressFocus for ReplyKeys {
    fn get_mode(&self) -> Mode {
        Mode::Insert
    }

    fn check_condition(&self, condition: Condition) -> bool {
        matches!(condition, Condition::ListFocus | Condition::InputFocus)
    }

    fn run_command(
        &self,
        command: &LapceCommand,
        count: Option<usize>,
        mods: Modifiers,
    ) -> CommandExecuted {
        match &command.kind {
            CommandKind::Focus(FocusCommand::ListSelect) => {
                let doc = self.editor.doc();
                let text = doc.buffer.with_untracked(|buffer| buffer.to_string());
                if !text.trim().is_empty() {
                    (self.on_submit)(text.trim().to_string());
                    doc.reload(lapce_xi_rope::Rope::from(""), true);
                }
                CommandExecuted::Yes
            }
            CommandKind::Edit(_) | CommandKind::Move(_) | CommandKind::MultiSelection(_) => {
                self.editor.run_command(command, count, mods)
            }
            _ => CommandExecuted::No,
        }
    }

    fn receive_char(&self, c: &str) {
        self.editor.receive_char(c);
    }
}

/// A small text button; `primary` ones use the primary button colours.
pub fn text_button(
    text: String,
    primary: bool,
    on_click: impl Fn() + 'static,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    label(move || text.clone())
        .on_click_stop(move |_| on_click())
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(8.0)
                .padding_vert(2.0)
                .margin_left(4.0)
                .border(1.0)
                .border_radius(4.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .cursor(CursorStyle::Pointer)
                .selectable(false)
                .apply_if(primary, |s| {
                    s.background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
                        .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
                })
                .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
        })
}

fn badge(
    text: impl Fn() -> Option<String> + 'static,
    color: &'static str,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let text = create_memo(move |_| text());
    label(move || text.get().unwrap_or_default()).style(move |s| {
        let config = config.get();
        s.margin_right(6.0)
            .padding_horiz(4.0)
            .border(1.0)
            .border_radius(4.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .color(config.color(color))
            .apply_if(text.with(|t| t.is_none()), |s| s.hide())
    })
}

fn h_stack_boxed(items: Vec<Box<dyn View>>) -> floem::views::Stack {
    floem::views::h_stack_from_iter(items)
}

fn v_stack_boxed(items: Vec<Box<dyn View>>) -> floem::views::Stack {
    v_stack_from_iter(items)
}

/// Where a thread is, for its header: the file and lines, or what it is on.
pub fn location_label(thread: &ReviewThread) -> String {
    let location = &thread.location;
    let file = location
        .uri
        .as_deref()
        .and_then(review_store::file_uri_to_path)
        .map(|path| {
            PathBuf::from(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or(path)
        });
    match (&file, &location.range) {
        (Some(file), Some(range)) if range.end.line > range.start.line => {
            format!("{file}:{}-{}", range.start.line + 1, range.end.line + 1)
        }
        (Some(file), Some(range)) => format!("{file}:{}", range.start.line + 1),
        (Some(file), None) => file.clone(),
        (None, _) => "repository".to_string(),
    }
}

/// The anchor's state for its badge: tracking's verdict on the commented code.
fn anchor_state(thread: &ReviewThread) -> Option<String> {
    let tracked = thread.location.tracked.as_ref()?;
    match tracked.status {
        AnchorStatus::Active => None,
        AnchorStatus::Modified => Some("changed".to_string()),
        AnchorStatus::Removed => Some("code removed".to_string()),
        AnchorStatus::Ambiguous => Some(format!(
            "ambiguous: {} places",
            tracked
                .candidates
                .as_ref()
                .map_or("several".to_string(), |c| c.len().to_string())
        )),
    }
}

/// Where the reviewer's latest message is: waiting for the agent until it picks it up (an MCP agent that isn't
/// listening picks it up when it checks in from its session), then "<agent> is on it", or "needs your decision" while
/// the agent waits on a permission.
pub fn status_line(thread: &ReviewThread, agent_name: Option<&str>, link: Option<AgentLink>) -> Option<String> {
    if thread.status == ThreadStatus::Resolved {
        return None;
    }
    let agent = agent_name.unwrap_or("the agent");
    match thread.agent_state? {
        AgentState::Queued => Some(match link {
            Some(AgentLink::Away) => format!("waiting for {agent} to check in and pick it up"),
            Some(AgentLink::Listening) => format!("waiting for {agent} to pick it up"),
            Some(AgentLink::Here) | None => format!("waiting for {agent}"),
        }),
        AgentState::Working => Some(format!("{agent} is on it")),
        AgentState::WaitingForHuman => Some("needs your decision".to_string()),
        AgentState::Idle => None,
    }
}

/// How the review's agent answers questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLink {
    /// Started here over ACP: answers now.
    Here,
    /// Connected from its own session over MCP and waiting for the reviewer: picks questions up now.
    Listening,
    /// Connected from its own session over MCP and busy elsewhere: questions wait until it checks in.
    Away,
}

pub fn agent_link(agent: &review_model::AgentConfig, listening: bool) -> AgentLink {
    match agent.transport {
        Some(review_model::AgentTransport::Mcp) if listening => AgentLink::Listening,
        Some(review_model::AgentTransport::Mcp) => AgentLink::Away,
        _ => AgentLink::Here,
    }
}

/// The ask button's text: what asking does.
pub fn ask_label(agent: &str, link: AgentLink) -> String {
    match link {
        AgentLink::Away => format!("Ask {agent} — waits until it checks in"),
        AgentLink::Here | AgentLink::Listening => format!("Ask {agent}"),
    }
}

pub fn ask_tooltip(agent: &str, link: AgentLink, can_answer_here: bool) -> String {
    match link {
        AgentLink::Here => format!("{agent} runs here (ACP) and answers now"),
        AgentLink::Listening => format!("{agent} is connected from its own session (MCP) and listening: it picks the question up now"),
        AgentLink::Away if can_answer_here => format!(
            "{agent} is connected from its own session (MCP) and isn't listening: the question waits until it checks in. \
             Ask now with {agent} (ACP) answers here instead."
        ),
        AgentLink::Away => format!(
            "{agent} is connected from its own session (MCP) and isn't listening: the question waits until it checks in"
        ),
    }
}

/// The installed ACP agent that is the same agent as the review's MCP agent (connected from a harness), to answer
/// here instead: Oh My Pi for "Oh My Pi" or "omp-coding-agent", Claude Code for "claude-code", ….
pub fn acp_stand_in(agent: &review_model::AgentConfig) -> Option<review_model::AgentConfig> {
    if agent.transport != Some(review_model::AgentTransport::Mcp) {
        return None;
    }
    static INSTALLED: std::sync::OnceLock<Vec<review_model::AgentConfig>> = std::sync::OnceLock::new();
    INSTALLED
        .get_or_init(review_agents::detect_agents)
        .iter()
        .find(|preset| same_agent(&agent.name, preset))
        .cloned()
}

/// Whether an MCP client's name names the agent of an ACP preset: the same name or id, ignoring case and
/// punctuation, or the preset's id as its first word.
fn same_agent(client_name: &str, preset: &review_model::AgentConfig) -> bool {
    let plain = |text: &str| -> String {
        text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
    };
    let name = plain(client_name);
    let first_word = client_name
        .split(|c: char| !c.is_alphanumeric())
        .find(|word| !word.is_empty())
        .map(str::to_lowercase);
    !name.is_empty()
        && (name == plain(&preset.name)
            || name == plain(&preset.id)
            || first_word.as_deref() == Some(preset.id.as_str()))
}

/// **Ask now with <agent> (ACP)** / **Answer here instead**: `agent` (an ACP agent) becomes the review's agent and
/// answers every question waiting for the agent it replaces, and `thread_id` (asked by the reviewer just now). A
/// review has one agent, so the MCP agent's seat goes to it; that agent rejoins from its session with `open_review`.
pub fn answer_here(
    window_tab_data: &WindowTabData,
    review_id: &str,
    agent: review_model::AgentConfig,
    thread_id: Option<String>,
) {
    let review_id = review_id.to_string();
    run(window_tab_data, "starting the agent here", move |backend| {
        let review = backend.agents.connect(&review_id, agent)?;
        let mut asked: Vec<String> = review
            .threads
            .iter()
            .filter(|thread| {
                thread.status != ThreadStatus::Resolved && thread.agent_state == Some(AgentState::Queued)
            })
            .map(|thread| thread.id.clone())
            .collect();
        if let Some(thread_id) = thread_id
            && !asked.contains(&thread_id)
        {
            backend
                .services
                .store
                .set_thread_intent(&review_id, &thread_id, ThreadIntent::Question)?;
            asked.push(thread_id);
        }
        for thread_id in asked {
            backend.agents.ask(&review_id, &thread_id);
        }
        Ok(())
    });
}

fn format_time(millis: u64) -> String {
    let Some(time) = chrono::DateTime::from_timestamp_millis(millis as i64) else {
        return String::new();
    };
    let local = time.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%b %-d, %H:%M").to_string()
    }
}

// ---- Actions: store writes and agent calls, off the UI thread ----

/// What an action needs off the UI thread.
#[derive(Clone)]
pub struct Backend {
    pub services: Services,
    pub agents: Arc<AgentService>,
    /// The workspace folder, for the reviewer's name (git config).
    pub root: Option<PathBuf>,
}

impl Backend {
    pub fn of(window_tab_data: &WindowTabData) -> Option<Backend> {
        let core = ReviewCore::get()?;
        Some(Backend {
            services: core.services.clone(),
            agents: Arc::clone(&core.agents),
            root: window_tab_data.review.root.clone(),
        })
    }

    /// The reviewer, from the repository's git config.
    pub fn user(&self) -> review_model::Participant {
        let root = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
        review_store::current_user(&root)
    }

    /// The reviewer's reply in a thread; the review's ACP agent answers when it should.
    pub fn reply(
        &self,
        review_id: &str,
        thread_id: &str,
        body: String,
    ) -> anyhow::Result<ReviewThread> {
        let thread =
            self.services
                .store
                .add_message(review_id, thread_id, body, self.user(), None)?;
        self.ask_if_answering(review_id, &thread);
        Ok(thread)
    }

    /// A new thread from the reviewer (a question when `intent` asks); on lines a change review's diff shows it is a
    /// comment on the diff (`code::comment_location`). The review's ACP agent answers when it should.
    pub fn start_thread(
        &self,
        review_id: &str,
        location: review_model::CodeLocation,
        body: String,
        intent: ThreadIntent,
    ) -> anyhow::Result<ReviewThread> {
        let location = match self.services.store.get(review_id) {
            Some(review) if location.kind != review_model::LocationKind::Patch => {
                super::code::comment_location(&self.services.code_folders, &review, location)
            }
            _ => location,
        };
        let (_, thread) = self.services.store.create_thread(
            review_id,
            location,
            body,
            self.user(),
            review_model::ThreadOptions {
                intent: Some(intent),
                ..Default::default()
            },
        )?;
        self.ask_if_answering(review_id, &thread);
        Ok(thread)
    }

    /// After a reviewer's message: the review's ACP agent answers when it should (a question, or a thread it is part
    /// of).
    pub fn ask_if_answering(&self, review_id: &str, thread: &ReviewThread) {
        let Some(review) = self.services.store.get(review_id) else {
            return;
        };
        if answers(&review, thread) {
            self.agents.ask(review_id, &thread.id);
        }
    }
}

/// Runs `work` on a background thread with the review backend.
pub fn run(
    window_tab_data: &WindowTabData,
    what: &'static str,
    work: impl FnOnce(&Backend) -> anyhow::Result<()> + Send + 'static,
) {
    let Some(backend) = Backend::of(window_tab_data) else {
        tracing::error!("{what}: the review core is not running");
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("ReviewAction".to_owned())
        .spawn(move || {
            if let Err(error) = work(&backend) {
                tracing::error!("{what} failed: {error:#}");
            }
        });
    if let Err(error) = spawned {
        tracing::error!("{what}: {error}");
    }
}

pub fn reply(window_tab_data: &WindowTabData, review_id: &str, thread_id: &str, body: String) {
    let (review_id, thread_id) = (review_id.to_string(), thread_id.to_string());
    run(window_tab_data, "reply", move |backend| {
        backend.reply(&review_id, &thread_id, body)?;
        Ok(())
    });
}

pub fn set_thread_status(
    window_tab_data: &WindowTabData,
    review_id: &str,
    thread_id: &str,
    status: ThreadStatus,
) {
    let (review_id, thread_id) = (review_id.to_string(), thread_id.to_string());
    run(window_tab_data, "changing the thread's status", move |backend| {
        backend
            .services
            .store
            .set_thread_status(&review_id, &thread_id, status, backend.user())?;
        Ok(())
    });
}

/// **Ask <agent>**: the thread becomes a question and the review's agent answers it.
pub fn ask_agent(window_tab_data: &WindowTabData, review_id: &str, thread_id: &str) {
    let (review_id, thread_id) = (review_id.to_string(), thread_id.to_string());
    run(window_tab_data, "asking the agent", move |backend| {
        backend
            .services
            .store
            .set_thread_intent(&review_id, &thread_id, ThreadIntent::Question)?;
        backend.agents.ask(&review_id, &thread_id);
        Ok(())
    });
}

pub fn stop_agent(window_tab_data: &WindowTabData, review_id: &str, thread_id: &str) {
    let (review_id, thread_id) = (review_id.to_string(), thread_id.to_string());
    run(window_tab_data, "stopping the agent", move |backend| {
        backend.agents.stop(&review_id, &thread_id)
    });
}

pub fn answer_permission(window_tab_data: &WindowTabData, request_id: &str, option_id: &str) {
    let (request_id, option_id) = (request_id.to_string(), option_id.to_string());
    run(window_tab_data, "answering the agent", move |backend| {
        backend.services.decisions.resolve(&request_id, Some(option_id));
        Ok(())
    });
}

pub fn decide_proposal(
    window_tab_data: &WindowTabData,
    review_id: &str,
    thread_id: &str,
    accept: bool,
) {
    let (review_id, thread_id) = (review_id.to_string(), thread_id.to_string());
    run(window_tab_data, "deciding the suggested edit", move |backend| {
        backend.services.bundles.decide_proposal(&review_id, &thread_id, accept)
    });
}

/// The review of `review_id` in the window tab, as last read.
pub fn review_of(window_tab_data: &WindowTabData, review_id: &str) -> Option<Arc<Review>> {
    window_tab_data.review.reviews.with_untracked(|reviews| {
        reviews.iter().find(|review| review.id == review_id).cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoWindows;

    impl review_mcp::ReviewWindows for NoWindows {
        fn show(&self, _: &str, _: &std::path::Path, _: &str) -> String {
            String::new()
        }

        fn done(&self, _: &str, _: &str, _: Option<&str>) {}
    }

    /// A question started from the editor reaches the review's ACP agent (the fake one), whose answer streams into
    /// the thread; a follow-up reply goes to the same agent.
    #[test]
    fn questions_reach_the_acp_agent() {
        let agent_binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/debug/fake-acp-agent");
        if !agent_binary.exists() {
            eprintln!("skipped: build fake-acp-agent first (cargo build -p review_agents)");
            return;
        }
        ask_and_follow_up(&agent_binary.to_string_lossy(), &[], "Fake Agent", 200);
    }

    /// The same with a real ACP agent: `CO_REVIEW_TEST_ACP="omp acp" cargo test -p lapce-app real_acp -- --ignored`.
    #[test]
    #[ignore]
    fn real_acp_agent_answers() {
        let command = std::env::var("CO_REVIEW_TEST_ACP").unwrap_or_else(|_| "omp acp".into());
        let mut words = command.split_whitespace().map(str::to_string);
        let program = words.next().unwrap();
        let args: Vec<String> = words.collect();
        ask_and_follow_up(&program, &args, "Real Agent", 3600);
    }

    fn ask_and_follow_up(program: &str, args: &[String], name: &str, polls: usize) {
        let home = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(home.path()).unwrap().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("lib.rs"), "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();
        let store = Arc::new(review_store::ReviewStore::with_home(home.path().join("co-review")));
        let services = Services::new(Arc::clone(&store), Arc::new(NoWindows));
        let agents = AgentService::new(Arc::clone(&store), Arc::clone(&services.decisions));
        let backend = Backend { services, agents, root: Some(root.clone()) };
        let review = store
            .create(review_store::CreateReviewParams {
                workspace_root: review_store::path_to_file_uri(&root.to_string_lossy()),
                title: "Test".into(),
                scope: review_model::ReviewScope::Repository,
                author: backend.user(),
            })
            .unwrap();
        backend
            .agents
            .connect(
                &review.id,
                review_model::AgentConfig {
                    id: "test".into(),
                    name: name.into(),
                    transport: Some(review_model::AgentTransport::Acp),
                    command: Some(program.into()),
                    args: Some(if args.is_empty() && name == "Fake Agent" {
                        vec![home.path().join("agent.jsonl").to_string_lossy().into_owned()]
                    } else {
                        args.to_vec()
                    }),
                    settings: None,
                    settings_label: None,
                    extra: Default::default(),
                },
            )
            .unwrap();
        let location = review_model::CodeLocation {
            kind: review_model::LocationKind::Line,
            uri: Some(review_store::path_to_file_uri(&root.join("lib.rs").to_string_lossy())),
            range: Some(review_model::Range {
                start: review_model::Position { line: 1, character: 0 },
                end: review_model::Position { line: 1, character: 9 },
            }),
            symbol: None,
            anchor: Some(review_model::CodeAnchor {
                text: "    a + b".into(),
                before: None,
                after: None,
                syntax: None,
            }),
            doc_anchor: None,
            patch_anchor: None,
            tracked: None,
            extra: Default::default(),
        };
        let thread = backend
            .start_thread(&review.id, location, "What does this do?".into(), ThreadIntent::Question)
            .unwrap();
        assert!(thread.location.tracked.is_some(), "a new thread's code is tracked");
        let answered = |count: usize| {
            for _ in 0..polls {
                let thread = store
                    .get(&review.id)
                    .and_then(|review| review.threads.into_iter().find(|t| t.id == thread.id))
                    .unwrap();
                let done = thread.messages.len() >= count
                    && thread.messages.last().is_some_and(|m| {
                        m.author.kind == ParticipantKind::Agent && m.status == Some(MessageStatus::Done)
                    });
                if done {
                    return thread;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("the agent did not answer");
        };
        let first = answered(2);
        let answer = first.messages.last().unwrap();
        eprintln!("{name} answered: {}", answer.body);
        assert_eq!(answer.author.name, name);
        assert!(!answer.body.is_empty());
        if name == "Fake Agent" {
            assert!(answer.activity.as_ref().is_some_and(|activity| !activity.is_empty()));
        }
        assert_eq!(status_line(&first, Some(name), Some(AgentLink::Here)), None);
        backend.reply(&review.id, &thread.id, "And then?".into()).unwrap();
        let second = answered(4);
        assert_eq!(second.messages[2].body, "And then?");
        backend.agents.shutdown();
    }

    fn agent(id: &str, name: &str, transport: review_model::AgentTransport) -> review_model::AgentConfig {
        review_model::AgentConfig {
            id: id.into(),
            name: name.into(),
            transport: Some(transport),
            command: None,
            args: None,
            settings: None,
            settings_label: None,
            extra: Default::default(),
        }
    }

    /// A question waiting for an MCP agent says what it waits for; one for an ACP agent reads as before.
    #[test]
    fn a_queued_question_says_it_waits_for_the_agent_to_pick_it_up() {
        let mut thread: ReviewThread = serde_json::from_value(serde_json::json!({
            "id": "t", "number": 1, "location": { "kind": "line" }, "messages": [],
            "status": "open", "intent": "question", "agentState": "queued", "createdAt": 0, "updatedAt": 0,
        }))
        .unwrap();
        let mcp = agent("agent:mcp:Oh My Pi", "Oh My Pi", review_model::AgentTransport::Mcp);
        let away = agent_link(&mcp, false);
        assert_eq!(away, AgentLink::Away);
        assert_eq!(
            status_line(&thread, Some("Oh My Pi"), Some(away)).as_deref(),
            Some("waiting for Oh My Pi to check in and pick it up")
        );
        assert_eq!(
            status_line(&thread, Some("Oh My Pi"), Some(agent_link(&mcp, true))).as_deref(),
            Some("waiting for Oh My Pi to pick it up")
        );
        let acp = agent("omp", "Oh My Pi", review_model::AgentTransport::Acp);
        assert_eq!(agent_link(&acp, false), AgentLink::Here);
        assert_eq!(status_line(&thread, Some("Oh My Pi"), Some(AgentLink::Here)).as_deref(), Some("waiting for Oh My Pi"));
        assert_eq!(ask_label("Oh My Pi", AgentLink::Here), "Ask Oh My Pi");
        assert_eq!(ask_label("Oh My Pi", AgentLink::Away), "Ask Oh My Pi — waits until it checks in");
        thread.agent_state = Some(AgentState::Working);
        assert_eq!(status_line(&thread, Some("Oh My Pi"), Some(away)).as_deref(), Some("Oh My Pi is on it"));
    }

    /// An MCP client's name finds the ACP preset of the same agent.
    #[test]
    fn harness_agents_find_their_acp_preset() {
        let preset = |id: &str| {
            review_agents::AGENT_PRESETS
                .iter()
                .find(|preset| preset.id == id)
                .map(|preset| preset.config())
                .unwrap()
        };
        assert!(same_agent("Oh My Pi", &preset("omp")));
        assert!(same_agent("omp-coding-agent", &preset("omp")));
        assert!(same_agent("claude-code", &preset("claude")));
        assert!(same_agent("gemini-cli-mcp-client", &preset("gemini")));
        assert!(!same_agent("Oh My Pi", &preset("pi")));
        assert!(!same_agent("agent", &preset("omp")));
    }

    #[test]
    fn parses_code_links() {
        assert_eq!(
            parse_link("src/orders.ts:42"),
            Some(MessageLink::Code {
                path: "src/orders.ts".into(),
                line: Some(42),
                end_line: None
            })
        );
        assert_eq!(
            parse_link("service.go:10-12."),
            Some(MessageLink::Code {
                path: "service.go".into(),
                line: Some(10),
                end_line: Some(12)
            })
        );
        assert_eq!(
            parse_link("src/a.rs#L10-L20"),
            Some(MessageLink::Code {
                path: "src/a.rs".into(),
                line: Some(10),
                end_line: Some(20)
            })
        );
        assert_eq!(
            parse_link("/docs/design.md"),
            Some(MessageLink::Code {
                path: "/docs/design.md".into(),
                line: None,
                end_line: None
            })
        );
        assert_eq!(parse_link("note:"), None);
        assert_eq!(parse_link("12:30"), None);
        assert_eq!(
            parse_link("https://example.com/x"),
            Some(MessageLink::Web("https://example.com/x".into()))
        );
    }
}
