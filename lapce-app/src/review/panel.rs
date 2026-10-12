//! The Review panel (`PanelKind::Review`): the review switcher with New and ⋯, the scope and the agent (its model,
//! and whether an MCP agent is listening, on hover), the Overview link, coverage, the submit bar, drafts, the filters
//! (Open, Proposed, Resolved, Out of scope, All, with counts), and the threads grouped by file or by area (their
//! first label). Clicking a thread's location opens it at its code.

use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use floem::{
    IntoView, View,
    reactive::{
        Memo, ReadSignal, SignalGet, SignalUpdate, SignalWith,
        create_memo, create_rw_signal,
    },
    style::CursorStyle,
    views::{
        Decorators, dyn_container, dyn_stack, empty, h_stack, label, scroll,
        v_stack,
    },
};
use review_model::{
    AgentTransport, Review, ReviewCoverage, ReviewThread, ThreadStatus,
};

use super::{
    ReviewCore,
    commands::{self, ReviewTab},
    submit::{link, submit_section},
    thread::{ThreadViewOptions, reply_box, text_button, thread_view},
    workspace::ReviewDraft,
};
use crate::{
    app::tooltip_label,
    config::{LapceConfig, color::LapceColor},
    panel::{data::PanelSection, position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Filter {
    Open,
    Proposed,
    Resolved,
    OutOfScope,
    All,
}

impl Filter {
    fn name(self) -> &'static str {
        match self {
            Filter::Open => "Open",
            Filter::Proposed => "Proposed",
            Filter::Resolved => "Resolved",
            Filter::OutOfScope => "Out of scope",
            Filter::All => "All",
        }
    }

    fn keeps(self, thread: &ReviewThread) -> bool {
        let out = commands::is_out_of_scope(thread);
        match self {
            Filter::All => true,
            Filter::OutOfScope => out,
            Filter::Open => thread.status == ThreadStatus::Open && !out,
            Filter::Proposed => thread.status == ThreadStatus::Proposed && !out,
            Filter::Resolved => thread.status == ThreadStatus::Resolved,
        }
    }
}

type Config = ReadSignal<Arc<LapceConfig>>;

pub fn review_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let window_tab_data_open = window_tab_data.panel.section_open(PanelSection::Reviews);
    let content = panel_content(window_tab_data);
    PanelBuilder::new(config, position)
        .add(
            "Review",
            content,
            window_tab_data_open,
        )
        .build()
        .debug_name("Review Panel")
}

fn dim(config: Config) -> impl Fn() -> floem::peniko::Color + Copy {
    move || config.get().color(LapceColor::PANEL_FOREGROUND_DIM)
}

fn panel_content(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let review = window_tab_data.review.clone();
    let loaded = review.loaded;
    // Whether there is an active review, not which: switching reviews keeps the review view (its memos follow the
    // new review). Rebuilding it on a switch disposed memos its list was still reading in the same update (a panic
    // when an agent showed another review in an open window).
    let has_active = create_memo(move |_| review.active_review().is_some());
    let body = {
        let window_tab_data = window_tab_data.clone();
        dyn_container(
            move || (loaded.get(), has_active.get()),
            move |state| match state {
                (false, _) => message("Loading reviews…", config).into_any(),
                (true, false) => empty_state(window_tab_data.clone()).into_any(),
                (true, true) => review_view(window_tab_data.clone()).into_any(),
            },
        )
        .style(|s| s.width_full())
    };
    scroll(
        v_stack((
            closing_banner(window_tab_data.clone()),
            header(window_tab_data.clone()),
            body,
        ))
        .style(|s| s.width_full().flex_col()),
    )
    .style(|s| s.absolute().size_full())
}

fn message(text: &'static str, config: Config) -> impl View {
    label(move || text).style(move |s| {
        s.padding_horiz(10.0)
            .padding_vert(6.0)
            .color(config.get().color(LapceColor::PANEL_FOREGROUND_DIM))
    })
}

/// The window closes for an agent that is done with it: why, the countdown, **Keep open** and **Close now**.
fn closing_banner(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let closing = window_tab_data.review.closing;
    let keep = super::windows::keep_open_action(&window_tab_data);
    let close = super::windows::close_now_action(&window_tab_data);
    v_stack((
        label(move || {
            closing
                .get()
                .map(|notice| {
                    format!(
                        "{} This window closes in {} second{}.",
                        notice.message,
                        notice.seconds,
                        if notice.seconds == 1 { "" } else { "s" }
                    )
                })
                .unwrap_or_default()
        })
        .style(|s| s.width_full()),
        h_stack((
            empty().style(|s| s.flex_grow(1.0_f32)),
            text_button("Keep open".to_string(), true, keep, config),
            text_button("Close now".to_string(), false, close, config),
        ))
        .style(|s| s.width_full().margin_top(6.0).items_center()),
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .flex_col()
            .padding(10.0)
            .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
            .border_bottom(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .apply_if(closing.with(Option::is_none), |s| s.hide())
    })
}

/// The review switcher (the active review's title; click to switch), New review and the ⋯ menu.
fn header(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let review = window_tab_data.review.clone();
    let title = {
        let review = review.clone();
        move || {
            review
                .active_review()
                .map(|review| {
                    if review.archived_at.is_some() {
                        format!("{} (archived)", review.title)
                    } else {
                        review.title.clone()
                    }
                })
                .unwrap_or_else(|| "No active review".to_string())
        }
    };
    let has_more = {
        let review = review.clone();
        move || {
            review.active_review().is_some()
                || review.reviews.with(|reviews| reviews.iter().any(|review| review.archived_at.is_some()))
        }
    };
    let (switch_tab, new_tab, more_tab) = (tab.clone(), tab.clone(), tab);
    h_stack((
        tooltip_label(
            config,
            label(move || format!("{} ▾", title()))
                .on_click_stop(move |_| commands::switch_review(&switch_tab))
                .style(|s| {
                    s.cursor(CursorStyle::Pointer)
                        .font_bold()
                        .text_ellipsis()
                        .min_width(0.0)
                }),
            || "Active review (click to switch)",
        )
        .style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
        tooltip_label(
            config,
            label(|| "+").on_click_stop(move |_| commands::new_review(&new_tab)).style(|s| {
                s.cursor(CursorStyle::Pointer).padding_horiz(6.0).font_size(16.0)
            }),
            || "New review",
        ),
        tooltip_label(
            config,
            label(|| "⋯")
                .on_click_stop(move |_| commands::review_actions(&more_tab))
                .style(move |s| {
                    s.cursor(CursorStyle::Pointer)
                        .padding_horiz(6.0)
                        .font_size(16.0)
                }),
            || "More actions",
        )
        .style(move |s| s.apply_if(!has_more(), |s| s.hide())),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

fn empty_state(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let has_root = tab.review.root.is_some();
    let text = |text: &'static str| {
        label(move || text).style(move |s| {
            s.color(config.get().color(LapceColor::PANEL_FOREGROUND_DIM))
                .margin_bottom(8.0)
        })
    };
    v_stack((
        text(if has_root {
            "No review is active for this repository."
        } else {
            "Open a repository folder to review it."
        }),
        text("A review can span the whole repository, a branch, a commit, or selected files and folders."),
        text_button(
            "Start a Review".to_string(),
            true,
            move || commands::new_review(&tab),
            config,
        )
        .style(move |s| s.apply_if(!has_root, |s| s.hide())),
    ))
    .style(|s| s.flex_col().padding(10.0).width_full())
}

fn review_view(window_tab_data: Rc<WindowTabData>) -> impl View {
    let review = window_tab_data.review.clone();
    let active: Memo<Option<Arc<Review>>> = {
        let review = review.clone();
        create_memo(move |_| review.active_review())
    };
    v_stack((
        summary(window_tab_data.clone(), active),
        coverage_view(window_tab_data.clone(), active),
        submit_section(window_tab_data.clone()),
        drafts_view(window_tab_data.clone(), active),
        threads_view(window_tab_data, active),
    ))
    .style(|s| s.width_full().flex_col())
}

/// One quiet line: the scope, Overview, and the agent (with its model; an MCP agent's listening status on hover).
fn summary(window_tab_data: Rc<WindowTabData>, active: Memo<Option<Arc<Review>>>) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let workspace = tab.review.clone();
    let listening = ReviewCore::get().map(|core| core.listening);
    let is_listening = move || {
        let Some(review) = active.get() else {
            return false;
        };
        listening.is_some_and(|listening| {
            listening.with(|listening| listening.get(&review.id).copied().unwrap_or(false))
        })
    };
    let agent_title = move || {
        let Some(review) = active.get() else {
            return String::new();
        };
        match &review.agent {
            None => "Connect an agent to ask questions".to_string(),
            Some(agent) if agent.transport == Some(AgentTransport::Mcp) => {
                if is_listening() {
                    format!("{} is listening (via MCP)", agent.name)
                } else {
                    format!("{} is busy; questions are delivered when it checks in", agent.name)
                }
            }
            Some(agent) => format!(
                "{} (ACP): {} — model: {}",
                agent.name,
                review_agents::command_line(agent),
                agent.settings_label.clone().unwrap_or_else(|| "the agent's default".to_string())
            ),
        }
    };
    let agent_text = move || {
        active
            .get()
            .and_then(|review| review.agent.clone())
            .map(|agent| commands::agent_label(&agent))
            .unwrap_or_else(|| "Connect agent".to_string())
    };
    let is_mcp = move || {
        active.get().is_some_and(|review| {
            review.agent.as_ref().is_some_and(|agent| agent.transport == Some(AgentTransport::Mcp))
        })
    };
    let (overview_tab, agent_tab) = (tab.clone(), tab);
    let dim = dim(config);
    v_stack((
        label(move || {
            active
                .get()
                .map(|review| commands::scope_label(&review.scope, &workspace))
                .unwrap_or_default()
        })
        .style(move |s| s.color(dim()).text_ellipsis().min_width(0.0)),
        h_stack((
            tooltip_label(
                config,
                link("Overview", config, move || commands::open_overview(&overview_tab))
                    .style(|s| s.margin_left(0.0)),
                || "The front page: what this is, how it works, every finding and comment",
            ),
            empty().style(|s| s.flex_grow(1.0_f32)),
            tooltip_label(
                config,
                h_stack((
                    label(|| "●").style(move |s| {
                        let config = config.get();
                        s.margin_right(4.0)
                            .apply_if(!is_mcp(), |s| s.hide())
                            .color(if is_listening() {
                                config.color(LapceColor::LAPCE_ICON_ACTIVE)
                            } else {
                                config.color(LapceColor::PANEL_FOREGROUND_DIM)
                            })
                    }),
                    label(agent_text),
                ))
                .on_click_stop(move |_| commands::connect_agent(&agent_tab))
                .style(|s| s.cursor(CursorStyle::Pointer).items_center()),
                agent_title,
            ),
        ))
        .style(|s| s.width_full().items_center().margin_top(2.0)),
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

/// Viewed files, overall and per area; an area opens its next file not viewed yet. Repository reviews only.
fn coverage_view(window_tab_data: Rc<WindowTabData>, active: Memo<Option<Arc<Review>>>) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let coverage = tab.review.coverage;
    let expanded = create_rw_signal(true);
    let shown = move || {
        active.with(|review| review.as_ref().is_some_and(|review| review.bundle.is_none()))
            && coverage.with(|coverage| coverage.as_ref().is_some_and(|coverage| coverage.total > 0))
    };
    let dim = dim(config);
    v_stack((
        tooltip_label(
            config,
            h_stack((
                label(move || if expanded.get() { "▾" } else { "▸" }).style(|s| s.margin_right(4.0)),
                label(move || {
                    coverage
                        .get()
                        .map(|coverage| format!("Viewed {} of {} files", coverage.viewed, coverage.total))
                        .unwrap_or_default()
                })
                .style(|s| s.flex_grow(1.0_f32)),
                progress(
                    move || coverage.with(|c| c.as_ref().map_or((0, 1), |c| (c.viewed, c.total))),
                    config,
                ),
            ))
            .on_click_stop(move |_| expanded.update(|expanded| *expanded = !*expanded))
            .style(|s| s.width_full().items_center().cursor(CursorStyle::Pointer)),
            || "Files you marked as viewed (⌘⌥V in the editor); a file changed since counts as not viewed",
        )
        .style(|s| s.width_full()),
        dyn_stack(
            move || {
                if !expanded.get() {
                    return Vec::new();
                }
                coverage
                    .get()
                    .filter(|coverage| coverage.areas.len() > 1)
                    .map(|coverage: ReviewCoverage| coverage.areas)
                    .unwrap_or_default()
            },
            |area| (area.path.clone(), area.viewed, area.total, area.next.clone()),
            move |area| {
                let tab = tab.clone();
                let next = area.next.clone();
                let done = next.is_none();
                let tip = next
                    .clone()
                    .map(|next| format!("Open the next file to review: {next}"))
                    .unwrap_or_else(|| "All files viewed".to_string());
                let path = if area.path == "." { "(root)".to_string() } else { area.path.clone() };
                let (viewed, total) = (area.viewed, area.total);
                tooltip_label(
                    config,
                    h_stack((
                        label(move || if done { "✓" } else { "▸" }).style(|s| s.margin_right(4.0)),
                        label(move || path.clone()).style(|s| s.flex_grow(1.0_f32).text_ellipsis().min_width(0.0)),
                        label(move || format!("{viewed}/{total}")).style(move |s| s.color(dim()).margin_right(6.0)),
                        progress(move || (viewed, total), config),
                    ))
                    .on_click_stop(move |_| {
                        if let Some(next) = &next {
                            commands::open_relative(&tab, next);
                        }
                    })
                    .style(move |s| {
                        s.width_full()
                            .items_center()
                            .padding_left(12.0)
                            .apply_if(!done, |s| s.cursor(CursorStyle::Pointer))
                            .apply_if(done, |s| s.color(dim()))
                    }),
                    move || tip.clone(),
                )
                .style(|s| s.width_full())
            },
        )
        .style(|s| s.width_full().flex_col()),
    ))
    .style(move |s| {
        s.width_full()
            .flex_col()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            .apply_if(!shown(), |s| s.hide())
    })
}

fn progress(value: impl Fn() -> (u32, u32) + 'static + Copy, config: Config) -> impl View {
    let width = 60.0;
    h_stack((empty().style(move |s| {
        let (viewed, total) = value();
        let filled = width * f64::from(viewed) / f64::from(total.max(1));
        s.height_full()
            .width(filled as f32)
            .background(config.get().color(LapceColor::LAPCE_ICON_ACTIVE))
    }),))
    .style(move |s| {
        s.width(width as f32)
            .height(4.0)
            .border_radius(2.0)
            .background(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

/// Comments being written in the panel (those without a line: on the repository, a file or a folder). Enter sends.
fn drafts_view(window_tab_data: Rc<WindowTabData>, active: Memo<Option<Arc<Review>>>) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let drafts = tab.review.drafts;
    let visible = move || {
        let id = active.with(|review| review.as_ref().map(|review| review.id.clone()));
        drafts.with(|drafts| {
            drafts
                .iter()
                .filter(|draft| Some(&draft.review_id) == id.as_ref())
                .cloned()
                .collect::<Vec<ReviewDraft>>()
        })
    };
    let has = move || !visible().is_empty();
    v_stack((
        label(|| "Drafts").style(|s| s.font_bold().margin_bottom(4.0)),
        dyn_stack(
            visible,
            |draft| draft.id.clone(),
            move |draft| {
                let tab = tab.clone();
                let window_tab_data = window_tab_data.clone();
                let review = active.get_untracked();
                let place = review
                    .as_ref()
                    .map(|review| {
                        commands::group_key(&draft.location, &tab.review, review)
                    })
                    .unwrap_or_default();
                let (id, discard_id) = (draft.id.clone(), draft.id.clone());
                let intent = draft.intent;
                let (send_window_tab, discard_window_tab, body_window_tab) =
                    (window_tab_data.clone(), window_tab_data.clone(), window_tab_data.clone());
                v_stack((
                    h_stack((
                        label(move || place.clone()).style(|s| s.flex_grow(1.0_f32)),
                        link("Discard", config, move || {
                            super::drafts::discard(&discard_window_tab, &discard_id)
                        }),
                    ))
                    .style(|s| s.width_full().items_center()),
                    reply_box(
                        window_tab_data,
                        "Comment (Enter to send)",
                        draft.body.clone(),
                        {
                            let id = id.clone();
                            move |body| {
                                super::drafts::set_body(&send_window_tab, &id, body);
                                super::drafts::submit(&send_window_tab, &id, intent);
                            }
                        },
                        move |body| super::drafts::set_body(&body_window_tab, &id, body),
                    ),
                ))
                .style(|s| s.width_full().flex_col().margin_bottom(6.0))
            },
        )
        .style(|s| s.width_full().flex_col()),
    ))
    .style(move |s| {
        s.width_full()
            .flex_col()
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            .apply_if(!has(), |s| s.hide())
    })
}

/// The filters, By area, and the threads in their groups.
fn threads_view(window_tab_data: Rc<WindowTabData>, active: Memo<Option<Arc<Review>>>) -> impl View {
    let config = window_tab_data.common.config;
    let filter = create_rw_signal(Filter::Open);
    let filter_chosen = create_rw_signal(false);
    let by_area = create_rw_signal(false);
    let counts = create_memo(move |_| {
        active.with(|review| {
            let Some(review) = review else {
                return BTreeMap::new();
            };
            [Filter::Open, Filter::Proposed, Filter::Resolved, Filter::OutOfScope, Filter::All]
                .into_iter()
                .map(|filter| {
                    (filter.name(), review.threads.iter().filter(|thread| filter.keeps(thread)).count())
                })
                .collect::<BTreeMap<&'static str, usize>>()
        })
    });
    // Until the reviewer picks a filter, a review with only proposed findings (an agent's first pass) shows those.
    let effective = create_memo(move |_| {
        let chosen = filter.get();
        if !filter_chosen.get() && chosen == Filter::Open {
            let counts = counts.get();
            let count = |name| counts.get(name).copied().unwrap_or(0);
            if count("Open") == 0 && count("Proposed") > 0 {
                return Filter::Proposed;
            }
        }
        chosen
    });
    let has_labels = move || {
        active.with(|review| {
            review.as_ref().is_some_and(|review| {
                review.threads.iter().any(|thread| thread.labels.as_ref().is_some_and(|labels| !labels.is_empty()))
            })
        })
    };
    let filter_button = move |choice: Filter, hide_when_empty: bool| {
        label(move || format!("{} {}", choice.name(), counts.with(|c| c.get(choice.name()).copied().unwrap_or(0))))
            .on_click_stop(move |_| {
                filter.set(choice);
                filter_chosen.set(true);
            })
            .style(move |s| {
                let config = config.get();
                let count = counts.with(|c| c.get(choice.name()).copied().unwrap_or(0));
                s.padding_horiz(6.0)
                    .padding_vert(1.0)
                    .margin_right(4.0)
                    .border_radius(4.0)
                    .cursor(CursorStyle::Pointer)
                    .apply_if(effective.get() == choice, |s| {
                        s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                            .font_bold()
                    })
                    .apply_if(hide_when_empty && count == 0, |s| s.hide())
            })
    };
    let filters = h_stack((
        filter_button(Filter::Open, false),
        filter_button(Filter::Proposed, true),
        filter_button(Filter::Resolved, false),
        filter_button(Filter::OutOfScope, true),
        filter_button(Filter::All, false),
        empty().style(|s| s.flex_grow(1.0_f32)),
        tooltip_label(
            config,
            label(|| "By area").on_click_stop(move |_| by_area.update(|area| *area = !*area)).style(move |s| {
                let config = config.get();
                s.padding_horiz(6.0)
                    .border_radius(4.0)
                    .cursor(CursorStyle::Pointer)
                    .apply_if(by_area.get(), |s| {
                        s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND)).font_bold()
                    })
            }),
            || "Group threads by area (their first label) instead of by file",
        )
        .style(move |s| s.apply_if(!has_labels(), |s| s.hide())),
    ))
    .style(|s| s.width_full().items_center().padding_horiz(10.0).padding_vert(6.0).flex_wrap(floem::style::FlexWrap::Wrap));

    let workspace = window_tab_data.review.clone();
    let groups = create_memo(move |_| {
        let Some(review) = active.get() else {
            return Vec::new();
        };
        let filter = effective.get();
        let area = by_area.get() && has_labels();
        let mut threads: Vec<(String, u32, &ReviewThread)> = review
            .threads
            .iter()
            .filter(|thread| filter.keeps(thread))
            .map(|thread| {
                let key = commands::group_key(&thread.location, &workspace, &review);
                let line = thread
                    .location
                    .patch_anchor
                    .as_ref()
                    .and_then(|anchor| anchor.line.or(anchor.start_line))
                    .or_else(|| thread.location.range.as_ref().map(|range| range.start.line + 1))
                    .unwrap_or(0);
                (key, line, thread)
            })
            .collect();
        threads.sort_by(|a, b| {
            let key = |k: &str| if k == "Repository" { String::new() } else { k.to_string() };
            key(&a.0).cmp(&key(&b.0)).then(a.1.cmp(&b.1)).then(a.2.number.cmp(&b.2.number))
        });
        let mut groups: Vec<(String, bool, Vec<String>)> = Vec::new();
        for (key, _, thread) in threads {
            let key = if area {
                thread
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.first().cloned())
                    .unwrap_or_else(|| "Other".to_string())
            } else {
                key
            };
            match groups.iter_mut().find(|(existing, _, _)| *existing == key) {
                Some((_, _, ids)) => ids.push(thread.id.clone()),
                None => groups.push((key, area, vec![thread.id.clone()])),
            }
        }
        if area {
            groups.sort_by(|a, b| a.0.cmp(&b.0));
        }
        groups
    });
    let empty_hint = move || {
        let Some(review) = active.get() else {
            return String::new();
        };
        if !groups.with(Vec::is_empty) {
            return String::new();
        }
        if review.threads.is_empty() {
            if review.agent.is_some() {
                "Hover a line in the editor and click + to comment (drag for several lines), or select code and choose Ask Agent.".to_string()
            } else {
                "Hover a line in the editor and click + to comment (drag for several lines). To ask an agent about code, connect one (Review: Connect ACP Agent…).".to_string()
            }
        } else {
            format!("No {} threads.", effective.get().name().to_lowercase())
        }
    };
    let review_id = move || active.with(|review| review.as_ref().map(|review| review.id.clone()).unwrap_or_default());
    let dim = dim(config);
    v_stack((
        filters,
        label(empty_hint).style(move |s| {
            s.padding_horiz(10.0)
                .padding_vert(6.0)
                .color(dim())
                .apply_if(groups.with(|groups| !groups.is_empty()), |s| s.hide())
        }),
        dyn_stack(
            move || groups.get(),
            move |(key, area, ids)| (key.clone(), *area, ids.clone(), review_id()),
            move |(key, area, ids)| {
                group_view(window_tab_data.clone(), key, area, ids, active, review_id())
            },
        )
        .style(|s| s.width_full().flex_col()),
    ))
    .style(|s| s.width_full().flex_col())
}

fn group_view(
    window_tab_data: Rc<WindowTabData>,
    key: String,
    area: bool,
    ids: Vec<String>,
    active: Memo<Option<Arc<Review>>>,
    review_id: String,
) -> impl View {
    let config = window_tab_data.common.config;
    let tab = ReviewTab::of(&window_tab_data);
    let count = ids.len();
    let heading = if area { format!("{key}  {count}") } else { key.clone() };
    let selected = tab.review.selected_thread;
    let header_tab = tab.clone();
    let first = ids.first().cloned();
    v_stack((
        label(move || heading.clone())
            .on_click_stop(move |_| {
                if area {
                    return;
                }
                let Some(review) = active.get_untracked() else {
                    return;
                };
                if let Some(thread) = first
                    .as_ref()
                    .and_then(|id| review.threads.iter().find(|thread| &thread.id == id))
                {
                    commands::open_thread(&header_tab, &review, thread);
                }
            })
            .style(move |s| {
                let config = config.get();
                s.width_full()
                    .padding_horiz(10.0)
                    .padding_vert(4.0)
                    .font_bold()
                    .text_ellipsis()
                    .color(config.color(LapceColor::PANEL_FOREGROUND))
                    .background(config.color(LapceColor::PANEL_BACKGROUND))
                    .apply_if(!area, |s| s.cursor(CursorStyle::Pointer))
            }),
        dyn_stack(
            move || ids.clone(),
            |id| id.clone(),
            move |thread_id| {
                let tab = tab.clone();
                let open_id = thread_id.clone();
                let on_open: Rc<dyn Fn()> = Rc::new(move || {
                    let Some(review) = active.get_untracked() else {
                        return;
                    };
                    if let Some(thread) = review.threads.iter().find(|thread| thread.id == open_id) {
                        commands::open_thread(&tab, &review, thread);
                    }
                });
                let highlighted = thread_id.clone();
                thread_view(
                    window_tab_data.clone(),
                    review_id.clone(),
                    thread_id,
                    ThreadViewOptions {
                        on_open: Some(on_open),
                        ..ThreadViewOptions::default()
                    },
                )
                .into_view()
                .style(move |s| {
                    let config = config.get();
                    s.width_full()
                        .padding_horiz(10.0)
                        .padding_vert(4.0)
                        .apply_if(selected.get().as_deref() == Some(highlighted.as_str()), |s| {
                            s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                        })
                })
            },
        )
        .style(|s| s.width_full().flex_col()),
    ))
    .style(|s| s.width_full().flex_col())
}
