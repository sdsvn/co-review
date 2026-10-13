//! Rendered Markdown review pages: a review directory's document (a design, a knowledge-bundle page, a Markdown file
//! of a code review) and a review's overview page (`overview.md` under `$CO_REVIEW_HOME`, rewritten as the review
//! changes and reloaded here). Comments go on selected text (Comment, Ask Agent, Propose Edit), on the whole
//! document, on a design step, on a diagram, a diagram node or an edge, and show after the block they are about.
//! HTML is not rendered: it opens in the system browser.
//!
//! The Theia app's `document-review-widget.tsx`; the page's text and anchors come from `review_markdown`.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, SystemTime},
};

use floem::{
    IntoView, View,
    action::exec_after,
    event::EventListener,
    ext_event::create_ext_action,
    reactive::{
        Memo, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal,
    },
    style::CursorStyle,
    views::{Decorators, dyn_container, dyn_stack, empty, h_stack, label, scroll, stack, v_stack, v_stack_from_iter},
};
use review_markdown::{Block, LinkTarget, OpenSpecCard, Page, StepNode, locate, openspec_cards, parse_link, resolve_link};
use review_mcp::Services;
use review_model::{
    AnchorStatus, CodeAnchor, CodeLocation, DocAnchor, DocAnchorType, LocationKind, Proposal, ProposalStatus,
    ReviewThread, ThreadOptions, ThreadStatus,
};

use super::{
    ReviewCore, ReviewPage, ReviewPageKind,
    design_tree::anchor_base,
    inline::draft_view,
    markdown_view::{Action, LeafKey, Mark, PageContext, TOP, blocks_view, selection_actions},
    thread::{ThreadViewOptions, reply_box, run, text_button, thread_view},
};
use crate::{config::color::LapceColor, window_tab::WindowTabData};

/// How often the page looks for a new version of its file.
const POLL: Duration = Duration::from_millis(1000);

/// A page's file, read and rendered.
struct Loaded {
    path: PathBuf,
    uri: String,
    /// The review's overview page.
    overview: bool,
    /// An HTML file: not rendered.
    html: bool,
    page: Rc<Page>,
    cards: Vec<(OpenSpecCard, Rc<Page>)>,
    images: Rc<HashMap<String, Vec<u8>>>,
    modified: Option<SystemTime>,
}

/// A suggested edit being written (drafts don't carry one).
#[derive(Clone, Debug, PartialEq)]
struct EditDraft {
    id: String,
    anchor: DocAnchor,
    before: String,
}

/// What is shown after a block.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Item {
    Thread(String),
    Draft(String),
    Edit(String),
}

/// Where everything on the page is.
#[derive(Clone, Debug, Default, PartialEq)]
struct Placement {
    slots: HashMap<LeafKey, Vec<Item>>,
    marks: Vec<Mark>,
    steps: Vec<String>,
    parts: Vec<(String, String)>,
}

/// The page of a review's overview or of a Markdown document.
pub fn document_page(window_tab: Rc<WindowTabData>, page: ReviewPage) -> impl View {
    let config = window_tab.common.config;
    let cx = Scope::current();
    let loaded: RwSignal<Option<Rc<Loaded>>> = create_rw_signal(None);
    let failure: RwSignal<Option<String>> = create_rw_signal(None);
    let review_id = page.review_id.clone();

    // Read and render the file off the UI thread, then again whenever it changes.
    let reload: Rc<dyn Fn()> = {
        let page = page.clone();
        let reading = Rc::new(Cell::new(false));
        Rc::new(move || {
            let Some(core) = ReviewCore::get() else {
                failure.set(Some("The review core is not running.".to_string()));
                return;
            };
            if reading.replace(true) {
                return;
            }
            let reading = reading.clone();
            let send = create_ext_action(cx, move |result: Result<LoadedData, String>| {
                reading.set(false);
                match result {
                    Ok(data) => {
                        tracing::debug!(
                            "review page {}: {} blocks, {} diagrams, {} OpenSpec cards, design {}, {} warnings",
                            data.path.display(),
                            data.page.blocks.len(),
                            data.page.diagrams.len(),
                            data.cards.len(),
                            data.page.design,
                            data.page.format.warnings.len()
                        );
                        failure.set(None);
                        loaded.set(Some(Rc::new(data.into_loaded())));
                    }
                    Err(error) => failure.set(Some(error)),
                }
            });
            let (services, page) = (core.services.clone(), page.clone());
            let spawned = std::thread::Builder::new()
                .name("ReviewPageLoad".to_owned())
                .spawn(move || send(load(&services, &page)));
            if let Err(error) = spawned {
                tracing::error!("reading the review page: {error}");
            }
        })
    };
    reload();
    let alive = watch(loaded, reload.clone());

    let review = {
        let (reviews, review_id) = (window_tab.review.reviews, review_id.clone());
        create_memo(move |_| reviews.with(|reviews| reviews.iter().find(|review| review.id == review_id).cloned()))
    };
    let has_agent = create_memo(move |_| review.with(|review| review.as_ref().is_some_and(|r| r.agent.is_some())));
    let uri = create_memo(move |_| loaded.with(|loaded| loaded.as_ref().map(|loaded| loaded.uri.clone())));
    let threads = create_memo(move |_| {
        let Some(uri) = uri.get() else {
            return Vec::new();
        };
        review.with(|review| {
            review
                .as_ref()
                .map(|review| {
                    review
                        .threads
                        .iter()
                        .filter(|thread| on_page(&thread.location, &uri))
                        .cloned()
                        .collect::<Vec<ReviewThread>>()
                })
                .unwrap_or_default()
        })
    });
    let drafts = {
        let (all, review_id) = (window_tab.review.drafts, review_id.clone());
        create_memo(move |_| {
            let Some(uri) = uri.get() else {
                return Vec::new();
            };
            all.with(|drafts| {
                drafts
                    .iter()
                    .filter(|draft| draft.review_id == review_id && on_page(&draft.location, &uri))
                    .filter_map(|draft| Some((draft.id.clone(), draft.location.doc_anchor.clone()?)))
                    .collect::<Vec<(String, DocAnchor)>>()
            })
        })
    };
    let edits: RwSignal<Vec<EditDraft>> = create_rw_signal(Vec::new());
    // Threads folded away by the reviewer, and resolved ones they unfolded.
    let hidden: RwSignal<HashSet<String>> = create_rw_signal(HashSet::new());
    let shown: RwSignal<HashSet<String>> = create_rw_signal(HashSet::new());
    let placement = create_memo(move |_| {
        let page = loaded.with(|loaded| loaded.as_ref().map(|loaded| loaded.page.clone()));
        let Some(page) = page else {
            return Placement::default();
        };
        threads.with(|threads| {
            drafts.with(|drafts| {
                edits.with(|edits| {
                    hidden.with(|hidden| shown.with(|shown| place(&page, threads, drafts, edits, hidden, shown)))
                })
            })
        })
    });

    create_effect(move |_| {
        placement.with(|placement| {
            let mut slots: Vec<(String, usize)> = placement
                .slots
                .iter()
                .map(|(key, items)| (if *key == TOP { "top".to_string() } else { key.to_string() }, items.len()))
                .collect();
            slots.sort();
            tracing::debug!(
                "review page comments: slots {slots:?}, {} marks, steps {:?}, diagram parts {:?}",
                placement.marks.len(),
                placement.steps,
                placement.parts
            );
        })
    });
    let selection = create_rw_signal(None);
    let actions_at = create_rw_signal(None);
    let level = create_rw_signal(2);
    let marks = create_memo(move |_| placement.with(|placement| placement.marks.clone()));
    let marked_steps = create_memo(move |_| placement.with(|placement| placement.steps.clone()));
    let marked_parts = create_memo(move |_| placement.with(|placement| placement.parts.clone()));
    let page_text = create_memo(move |_| loaded.with(|loaded| loaded.as_ref().map(|loaded| loaded.page.clone())));

    let on_comment: Rc<dyn Fn(DocAnchor, Action)> = {
        let window_tab = window_tab.clone();
        let review_id = review_id.clone();
        Rc::new(move |anchor: DocAnchor, action: Action| {
            let Some(uri) = uri.get_untracked() else {
                return;
            };
            window_tab.review.active.set(Some(review_id.clone()));
            if action == Action::ProposeEdit {
                if let Some(before) = anchor.exact.clone() {
                    edits.update(|edits| {
                        edits.push(EditDraft {
                            id: format!("edit-{}", uuid_like()),
                            anchor,
                            before,
                        })
                    });
                }
                return;
            }
            super::drafts::add(&window_tab, location_of(&uri, anchor), action.intent(), |_| {});
        })
    };
    let on_link: Rc<dyn Fn(String)> = {
        let window_tab = window_tab.clone();
        let review_id = review_id.clone();
        Rc::new(move |href: String| {
            let Some(loaded) = loaded.get_untracked() else {
                return;
            };
            open_link(&window_tab, &review_id, &loaded.path, &href);
        })
    };
    let on_mark: Rc<dyn Fn(String)> = Rc::new(move |thread_id: String| {
        hidden.update(|hidden| {
            hidden.remove(&thread_id);
        });
        shown.update(|shown| {
            shown.insert(thread_id);
        });
    });
    let slot: Rc<dyn Fn(LeafKey) -> floem::AnyView> = {
        let window_tab = window_tab.clone();
        let review_id = review_id.clone();
        Rc::new(move |key: LeafKey| slot_view(window_tab.clone(), review_id.clone(), key, placement, hidden, edits, uri))
    };

    let header = header(window_tab.clone(), page.clone(), loaded, review, on_comment.clone());
    let body = {
        let window_tab = window_tab.clone();
        dyn_container(
            move || (loaded.get(), failure.get()),
            move |(loaded, failure)| {
                let Some(loaded) = loaded else {
                    let text = failure.unwrap_or_else(|| "Loading…".to_string());
                    return label(move || text.clone())
                        .style(move |s| s.padding(24.0).color(config.get().color(LapceColor::EDITOR_DIM)))
                        .into_any();
                };
                if loaded.html {
                    return html_notice(window_tab.clone(), loaded.path.clone()).into_any();
                }
                let ctx = PageContext {
                    config,
                    selection,
                    actions_at,
                    content: Rc::new(RefCell::new(None)),
                    marks,
                    marked_steps,
                    marked_parts,
                    level,
                    texts: Rc::new(RefCell::new(Vec::new())),
                    on_link: on_link.clone(),
                    on_mark: on_mark.clone(),
                    on_comment: on_comment.clone(),
                    slot: slot.clone(),
                    images: loaded.images.clone(),
                    diagrams: Rc::new(loaded.page.diagrams.clone()),
                };
                page_body(&ctx, &loaded, page_text, has_agent).into_any()
            },
        )
        .style(|s| s.width_full())
    };
    v_stack((header, scroll(body).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0_f32).width_full())))
        .style(move |s| {
            let config = config.get();
            s.size_full()
                .flex_col()
                .background(config.color(LapceColor::EDITOR_BACKGROUND))
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
        })
        .on_cleanup(move || alive.set(false))
        .debug_name("Review Document Page")
}

/// The page's content: the comments on the whole document, the blocks, the OpenSpec cards; and the selection's
/// actions over them.
fn page_body(ctx: &PageContext, loaded: &Loaded, page: Memo<Option<Rc<Page>>>, has_agent: Memo<bool>) -> impl View + use<> {
    let (selection, actions_at) = (ctx.selection, ctx.actions_at);
    let mut parts: Vec<floem::AnyView> = vec![(ctx.slot)(TOP), blocks_view(ctx, &loaded.page.blocks).into_any()];
    if !loaded.cards.is_empty() {
        parts.push(openspec_view(ctx, loaded).into_any());
    }
    let content = v_stack_from_iter(parts).style(|s| s.width_full().max_width(980.0).padding_horiz(32.0).padding_vert(20.0));
    let page_view = stack((content, selection_actions(ctx, page, has_agent)))
        .on_event_cont(EventListener::PointerDown, move |_| {
            selection.set(None);
            actions_at.set(None);
        })
        .style(|s| s.width_full().flex_col().items_center());
    *ctx.content.borrow_mut() = Some(page_view.id());
    page_view
}

/// The review directory's OpenSpec change, as cards under the document: the proposal, then one per capability.
fn openspec_view(ctx: &PageContext, loaded: &Loaded) -> impl View + use<> {
    let config = ctx.config;
    // The cards' text isn't the page's: it can be read, not commented on.
    let card_ctx = PageContext {
        selection: create_rw_signal(None),
        actions_at: create_rw_signal(None),
        texts: Rc::new(RefCell::new(Vec::new())),
        on_comment: Rc::new(|_, _| {}),
        slot: Rc::new(|_| empty().into_any()),
        ..ctx.clone()
    };
    let name = loaded
        .cards
        .first()
        .map(|_| "OpenSpec change".to_string())
        .unwrap_or_default();
    let mut rows: Vec<floem::AnyView> = vec![
        label(move || name.clone())
            .style(|s| s.font_size(18.0).font_bold().margin_top(24.0).margin_bottom(8.0))
            .into_any(),
    ];
    for (card, page) in &loaded.cards {
        let open = create_rw_signal(card.open);
        let title = card.title.clone();
        let blocks = page.blocks.clone();
        let card_ctx = card_ctx.clone();
        rows.push(
            v_stack((
                label(move || format!("{} {}", if open.get() { "\u{25be}" } else { "\u{25b8}" }, title))
                    .on_click_stop(move |_| open.update(|open| *open = !*open))
                    .style(|s| s.font_bold().padding(8.0).cursor(CursorStyle::Pointer).selectable(false)),
                dyn_container(
                    move || open.get(),
                    move |open| {
                        if open {
                            blocks_view(&card_ctx, &blocks).style(|s| s.padding_horiz(12.0).padding_bottom(8.0)).into_any()
                        } else {
                            empty().into_any()
                        }
                    },
                ),
            ))
            .style(move |s| {
                s.width_full()
                    .margin_bottom(8.0)
                    .border(1.0)
                    .border_radius(6.0)
                    .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            })
            .into_any(),
        );
    }
    v_stack_from_iter(rows).style(|s| s.width_full())
}

/// The page's header: what it is, Comment on the document, Open the source.
fn header(
    window_tab: Rc<WindowTabData>,
    page: ReviewPage,
    loaded: RwSignal<Option<Rc<Loaded>>>,
    review: Memo<Option<std::sync::Arc<review_model::Review>>>,
    on_comment: Rc<dyn Fn(DocAnchor, Action)>,
) -> impl View {
    let config = window_tab.common.config;
    let root = window_tab.review.root.clone();
    let what = move || {
        let title = review.with(|review| review.as_ref().map(|review| review.title.clone())).unwrap_or_default();
        loaded.with(|loaded| match loaded {
            Some(loaded) if loaded.overview => {
                format!("Overview of \u{201c}{title}\u{201d} \u{2014} kept up to date; comment on any of it")
            }
            Some(loaded) => root
                .as_ref()
                .and_then(|root| loaded.path.strip_prefix(root).ok())
                .unwrap_or(&loaded.path)
                .to_string_lossy()
                .into_owned(),
            None => page.kind.name().to_string(),
        })
    };
    let comment = text_button(
        "Comment on the document".to_string(),
        false,
        move || {
            on_comment(
                DocAnchor {
                    anchor_type: DocAnchorType::Document,
                    ..anchor_base()
                },
                Action::Comment,
            )
        },
        config,
    );
    let source = {
        let window_tab = window_tab.clone();
        text_button(
            "Open the source".to_string(),
            false,
            move || {
                if let Some(loaded) = loaded.get_untracked() {
                    super::thread::open_location(&window_tab, loaded.path.clone(), None);
                }
            },
            config,
        )
    };
    let version = move || {
        review
            .with(|review| review.as_ref().and_then(|review| review.bundle.as_ref()?.doc_version))
            .filter(|version| *version > 1)
            .map(|version| format!("version {version}"))
            .unwrap_or_default()
    };
    h_stack((
        label(what).style(|s| s.min_width(0.0).text_ellipsis()),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(version).style(move |s| s.margin_right(8.0).color(config.get().color(LapceColor::EDITOR_DIM))),
        comment,
        source,
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .items_center()
            .padding_horiz(12.0)
            .padding_vert(6.0)
            .border_bottom(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .color(config.color(LapceColor::EDITOR_DIM))
    })
}

/// HTML is not rendered: it opens in the system browser.
fn html_notice(window_tab: Rc<WindowTabData>, path: PathBuf) -> impl View {
    let config = window_tab.common.config;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    v_stack((
        label(move || format!("{name} is HTML, which Co-Review doesn't render.")),
        text_button(
            "Open in Browser".to_string(),
            true,
            move || {
                if let Err(error) = open::that(&path) {
                    tracing::error!("opening {} in the browser: {error}", path.display());
                }
            },
            config,
        )
        .style(|s| s.margin_top(12.0)),
    ))
    .style(|s| s.padding(32.0).items_start())
}

/// The comments placed after the block `key` (or above the page: [`TOP`]).
fn slot_view(
    window_tab: Rc<WindowTabData>,
    review_id: String,
    key: LeafKey,
    placement: Memo<Placement>,
    hidden: RwSignal<HashSet<String>>,
    edits: RwSignal<Vec<EditDraft>>,
    uri: Memo<Option<String>>,
) -> floem::AnyView {
    dyn_stack(
        move || placement.with(|placement| placement.slots.get(&key).cloned().unwrap_or_default()),
        |item| item.clone(),
        move |item| match item {
            Item::Thread(thread_id) => {
                let id = thread_id.clone();
                let options = ThreadViewOptions {
                    inline: false,
                    on_hide: Some(Rc::new(move || {
                        hidden.update(|hidden| {
                            hidden.insert(id.clone());
                        })
                    })),
                    on_open: None,
                };
                thread_view(window_tab.clone(), review_id.clone(), thread_id, options)
                    .style(|s| s.margin_vert(6.0))
                    .into_any()
            }
            Item::Draft(draft_id) => draft_view(window_tab.clone(), draft_id).style(|s| s.margin_vert(6.0)).into_any(),
            Item::Edit(edit_id) => edit_view(window_tab.clone(), review_id.clone(), edit_id, edits, uri).into_any(),
        },
    )
    .style(|s| s.width_full().flex_col())
    .into_any()
}

/// A suggested edit being written: the replacement for the quoted text, and why.
fn edit_view(
    window_tab: Rc<WindowTabData>,
    review_id: String,
    edit_id: String,
    edits: RwSignal<Vec<EditDraft>>,
    uri: Memo<Option<String>>,
) -> impl View {
    let config = window_tab.common.config;
    let Some(edit) = edits.with_untracked(|edits| edits.iter().find(|edit| edit.id == edit_id).cloned()) else {
        return empty().into_any();
    };
    let after = create_rw_signal(edit.before.clone());
    let why = create_rw_signal(String::new());
    let discard = {
        let edit_id = edit_id.clone();
        move || edits.update(|edits| edits.retain(|edit| edit.id != edit_id))
    };
    let send = {
        let (window_tab, edit, discard) = (window_tab.clone(), edit.clone(), discard.clone());
        move || {
            let after = after.get_untracked();
            let Some(uri) = uri.get_untracked() else {
                return;
            };
            if after == edit.before {
                return;
            }
            let body = Some(why.get_untracked().trim().to_string())
                .filter(|body| !body.is_empty())
                .unwrap_or_else(|| "Suggested edit.".to_string());
            let location = location_of(&uri, edit.anchor.clone());
            let (review_id, before) = (review_id.clone(), edit.before.clone());
            discard();
            run(&window_tab, "suggesting an edit", move |backend| {
                let (_, thread) = backend.services.store.create_thread(
                    &review_id,
                    location,
                    body,
                    backend.user(),
                    ThreadOptions {
                        proposal: Some(Proposal {
                            before,
                            after,
                            path: None,
                            start_line: None,
                            status: ProposalStatus::Pending,
                        }),
                        ..ThreadOptions::default()
                    },
                )?;
                backend.ask_if_answering(&review_id, &thread);
                Ok(())
            });
        }
    };
    let quote: String = edit.before.chars().take(60).collect();
    let send = Rc::new(send);
    let (send_on_enter, send_on_click) = (send.clone(), send.clone());
    v_stack((
        label(move || format!("Suggest an edit \u{b7} \u{201c}{quote}\u{201d}")).style(|s| s.font_bold().margin_bottom(4.0)),
        label(|| "Replace it with").style(move |s| s.font_size(11.0).color(config.get().color(LapceColor::EDITOR_DIM))),
        reply_box(
            window_tab.clone(),
            "The new text (Enter to send)",
            edit.before.clone(),
            move |text| {
                after.set(text);
                send_on_enter();
            },
            move |text| after.set(text),
        ),
        label(|| "Why (optional)").style(move |s| {
            s.margin_top(6.0).font_size(11.0).color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        reply_box(window_tab.clone(), "A note for the author", String::new(), |_| {}, move |text| why.set(text)),
        h_stack((
            empty().style(|s| s.flex_grow(1.0_f32)),
            text_button("Discard".to_string(), false, discard, config),
            text_button("Suggest edit".to_string(), true, move || send_on_click(), config),
        ))
        .style(|s| s.width_full().margin_top(6.0)),
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .margin_vert(6.0)
            .padding(8.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    })
    .into_any()
}

// -----------------------------------------------------------------------------------------------------------------
// Placing comments
// -----------------------------------------------------------------------------------------------------------------

/// The blocks comments are placed after: their text's range on the page, and the diagrams and steps by id.
#[derive(Default)]
struct Leaves {
    ranges: Vec<(usize, usize)>,
    diagrams: HashMap<String, LeafKey>,
    steps: HashMap<String, LeafKey>,
}

fn leaves(page: &Page) -> Leaves {
    fn steps(nodes: &[StepNode], leaves: &mut Leaves) {
        for node in nodes {
            leaves.ranges.push((node.text.start, node.text.end()));
            leaves.steps.entry(node.id.clone()).or_insert(node.text.start);
            steps(&node.children, leaves);
        }
    }
    fn walk(blocks: &[Block], page: &Page, leaves: &mut Leaves) {
        for block in blocks {
            match block {
                Block::Paragraph(inline) | Block::Heading { text: inline, .. } | Block::Code { text: inline, .. } => {
                    leaves.ranges.push((inline.start, inline.end()));
                }
                Block::Table { head, rows, .. } => {
                    let first = head.first().map(|cell| cell.start);
                    let last = rows.last().and_then(|row| row.last()).or(head.last()).map(|cell| cell.end());
                    if let (Some(first), Some(last)) = (first, last) {
                        leaves.ranges.push((first, last));
                    }
                }
                Block::Diagram { index, head } => {
                    leaves.ranges.push((head.start, head.end()));
                    if let Some(diagram) = page.diagrams.get(*index) {
                        leaves.diagrams.entry(diagram.id.clone()).or_insert(head.start);
                    }
                }
                Block::Quote(children) => walk(children, page, leaves),
                Block::List { items, .. } => items.iter().for_each(|item| walk(item, page, leaves)),
                Block::Section { blocks, .. } => walk(blocks, page, leaves),
                Block::Tree { steps: nodes, .. } => steps(nodes, leaves),
                Block::Meta(_) | Block::Warnings { .. } | Block::Rule => {}
            }
        }
    }
    let mut leaves = Leaves::default();
    walk(&page.blocks, page, &mut leaves);
    leaves.ranges.sort_unstable();
    leaves
}

/// Where an anchor is on the page: the block it goes after, and the text it marks.
fn place_anchor(page: &Page, leaves: &Leaves, anchor: &DocAnchor) -> Option<(LeafKey, Option<(usize, usize)>)> {
    match anchor.anchor_type {
        DocAnchorType::Document => Some((TOP, None)),
        DocAnchorType::Text => {
            let (start, end) = locate(&page.text, anchor)?;
            let last = end.saturating_sub(1).max(start);
            let key = leaves
                .ranges
                .iter()
                .rev()
                .find(|(leaf_start, _)| *leaf_start <= last)
                .map_or(TOP, |(leaf_start, _)| *leaf_start);
            Some((key, Some((start, end))))
        }
        DocAnchorType::MermaidBlock | DocAnchorType::MermaidNode | DocAnchorType::MermaidEdge => {
            leaves.diagrams.get(anchor.block_id.as_deref()?).map(|key| (*key, None))
        }
        DocAnchorType::TreeNode => leaves.steps.get(anchor.node_id.as_deref()?).map(|key| (*key, None)),
        DocAnchorType::Other => None,
    }
}

fn place(
    page: &Page,
    threads: &[ReviewThread],
    drafts: &[(String, DocAnchor)],
    edits: &[EditDraft],
    hidden: &HashSet<String>,
    shown: &HashSet<String>,
) -> Placement {
    let leaves = leaves(page);
    let mut placement = Placement::default();
    for thread in threads {
        let Some(anchor) = &thread.location.doc_anchor else {
            continue;
        };
        // Its text was removed, or can't be told apart from other text: in the Review panel only.
        let status = thread.location.tracked.as_ref().map(|tracked| tracked.status);
        if thread.status != ThreadStatus::Resolved && matches!(status, Some(AnchorStatus::Removed | AnchorStatus::Ambiguous)) {
            continue;
        }
        let placed = place_anchor(page, &leaves, anchor);
        if let Some((_, Some((start, end)))) = placed {
            placement.marks.push(Mark {
                start,
                end,
                thread_id: thread.id.clone(),
            });
        }
        if placed.is_some() {
            match anchor.anchor_type {
                DocAnchorType::TreeNode => placement.steps.extend(anchor.node_id.clone()),
                DocAnchorType::MermaidNode | DocAnchorType::MermaidEdge => {
                    if let (Some(block), Some(part)) =
                        (anchor.block_id.clone(), anchor.node_id.clone().or_else(|| anchor.edge_id.clone()))
                    {
                        placement.parts.push((block, part));
                    }
                }
                _ => {}
            }
        }
        let folded = (thread.status == ThreadStatus::Resolved || hidden.contains(&thread.id)) && !shown.contains(&thread.id);
        // A comment whose place is gone shows above the page.
        if folded && placed.is_some() {
            continue;
        }
        let key = placed.map_or(TOP, |(key, _)| key);
        placement.slots.entry(key).or_default().push(Item::Thread(thread.id.clone()));
    }
    for (draft_id, anchor) in drafts {
        let key = place_anchor(page, &leaves, anchor).map_or(TOP, |(key, _)| key);
        placement.slots.entry(key).or_default().push(Item::Draft(draft_id.clone()));
    }
    for edit in edits {
        let key = place_anchor(page, &leaves, &edit.anchor).map_or(TOP, |(key, _)| key);
        placement.slots.entry(key).or_default().push(Item::Edit(edit.id.clone()));
    }
    placement
}

/// Whether a comment's location is on the page's file.
fn on_page(location: &CodeLocation, uri: &str) -> bool {
    if location.kind != LocationKind::Document {
        return false;
    }
    let Some(location_uri) = location.uri.as_deref() else {
        return false;
    };
    location_uri == uri
        || review_store::file_uri_to_path(location_uri)
            .zip(review_store::file_uri_to_path(uri))
            .is_some_and(|(a, b)| review_store::real_path(Path::new(&a)) == review_store::real_path(Path::new(&b)))
}

/// A comment's location on the page's file (as the Theia app records it).
fn location_of(uri: &str, anchor: DocAnchor) -> CodeLocation {
    let text = anchor.exact.clone().or_else(|| anchor.source.clone()).unwrap_or_default();
    CodeLocation {
        kind: LocationKind::Document,
        uri: Some(uri.to_string()),
        range: None,
        symbol: None,
        anchor: Some(CodeAnchor {
            text,
            before: None,
            after: None,
            syntax: None,
        }),
        doc_anchor: Some(anchor),
        patch_anchor: None,
        tracked: None,
        extra: Default::default(),
    }
}

fn uuid_like() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}")
}

// -----------------------------------------------------------------------------------------------------------------
// Links
// -----------------------------------------------------------------------------------------------------------------

/// Follows a link: the web in the system browser, another page as a review page, code in the editor at its line.
fn open_link(window_tab: &Rc<WindowTabData>, review_id: &str, page_path: &Path, href: &str) {
    let page_dir = page_path.parent().unwrap_or(Path::new("/"));
    let review = super::thread::review_of(window_tab, review_id);
    let bundle_dir = review
        .as_ref()
        .and_then(|review| review.bundle.as_ref())
        .map(|bundle| PathBuf::from(&bundle.dir));
    match parse_link(href) {
        LinkTarget::External(url) => {
            if let Err(error) = open::that(&url) {
                tracing::error!("opening {url}: {error}");
            }
        }
        LinkTarget::Patch(_) => super::reveal_page(window_tab, ReviewPage::pull_request(review_id)),
        LinkTarget::Fragment(_) => {}
        LinkTarget::Path { path, line, .. } => {
            let root = window_tab.review.root.clone();
            let Some(target) = resolve_link(&path, page_dir, bundle_dir.as_deref(), root.as_deref()) else {
                tracing::warn!("no file {path} for the link on {}", page_path.display());
                return;
            };
            let markdown = target
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("md" | "markdown")));
            let patch = target
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("patch" | "diff")));
            let html = target
                .extension()
                .is_some_and(|extension| matches!(extension.to_str(), Some("html" | "htm")));
            if target.is_dir() {
                tracing::info!("{} is a folder", target.display());
            } else if (markdown || html) && line.is_none() {
                super::reveal_page(window_tab, ReviewPage::document(review_id, &target));
            } else if patch && line.is_none() {
                super::reveal_page(window_tab, ReviewPage::pull_request(review_id));
            } else {
                super::thread::open_location(window_tab, target, line.map(|line| line.saturating_sub(1) as usize));
            }
        }
    }
}

// -----------------------------------------------------------------------------------------------------------------
// Loading
// -----------------------------------------------------------------------------------------------------------------

/// What the loading thread sends: `Loaded` without its `Rc`s.
struct LoadedData {
    path: PathBuf,
    overview: bool,
    html: bool,
    page: Page,
    cards: Vec<(OpenSpecCard, Page)>,
    images: HashMap<String, Vec<u8>>,
    modified: Option<SystemTime>,
}

impl LoadedData {
    fn into_loaded(self) -> Loaded {
        Loaded {
            uri: review_store::path_to_file_uri(&self.path.to_string_lossy()),
            path: self.path,
            overview: self.overview,
            html: self.html,
            page: Rc::new(self.page),
            cards: self.cards.into_iter().map(|(card, page)| (card, Rc::new(page))).collect(),
            images: Rc::new(self.images),
            modified: self.modified,
        }
    }
}

/// The page's file: the document, or the review's overview page (written now if it isn't yet), else the review
/// directory's document (a design or a knowledge bundle has no overview page: its document is its front page).
fn file_of(services: &Services, page: &ReviewPage) -> Result<(PathBuf, bool), String> {
    if let Some(path) = &page.path {
        return Ok((PathBuf::from(path), false));
    }
    if page.kind != ReviewPageKind::Overview {
        return Err("This page has no file.".to_string());
    }
    match services.pages.write(&page.review_id, false) {
        Ok(Some(file)) => Ok((file, true)),
        Ok(None) => services
            .store
            .get(&page.review_id)
            .and_then(|review| review.bundle)
            .and_then(|bundle| review_bundle::service::document_of(Path::new(&bundle.dir)))
            .map(|document| (document, false))
            .ok_or_else(|| "This review has no overview page.".to_string()),
        Err(error) => Err(format!("The overview page could not be written: {error:#}")),
    }
}

fn load(services: &Services, page: &ReviewPage) -> Result<LoadedData, String> {
    let (path, overview) = file_of(services, page)?;
    let modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
    let html = path
        .extension()
        .is_some_and(|extension| matches!(extension.to_str(), Some("html" | "htm")));
    let source = if html {
        String::new()
    } else {
        std::fs::read_to_string(&path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?
    };
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let rendered = review_markdown::render(&source, &name);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let images = rendered
        .inlines()
        .iter()
        .flat_map(|inline| inline.images.iter())
        .filter(|image| !image.url.contains(':'))
        .filter_map(|image| Some((image.url.clone(), std::fs::read(dir.join(&image.url)).ok()?)))
        .collect();
    // The review directory's OpenSpec change, under its document.
    let bundle = services.store.get(&page.review_id).and_then(|review| review.bundle);
    let cards = bundle
        .as_ref()
        .filter(|bundle| review_store::real_path(Path::new(&bundle.dir)) == review_store::real_path(&dir))
        .and_then(|bundle| bundle.openspec.clone())
        .map(|openspec| {
            openspec_cards(Path::new(&openspec))
                .into_iter()
                .map(|card| {
                    let page = review_markdown::render(&card.markdown, "card.md");
                    (card, page)
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(LoadedData {
        path,
        overview,
        html,
        page: rendered,
        cards,
        images,
        modified,
    })
}

/// Reloads the page when its file changes (agents revise documents; the overview page follows the review).
fn watch(loaded: RwSignal<Option<Rc<Loaded>>>, reload: Rc<dyn Fn()>) -> Rc<Cell<bool>> {
    let alive = Rc::new(Cell::new(true));
    fn tick(loaded: RwSignal<Option<Rc<Loaded>>>, reload: Rc<dyn Fn()>, alive: Rc<Cell<bool>>) {
        exec_after(POLL, move |_| {
            if !alive.get() {
                return;
            }
            // Gone with its window (closing a window disposes its scope before its views are cleaned up): stop.
            let Some(changed) = changed_on_disk(loaded) else {
                return;
            };
            if changed {
                reload();
            }
            tick(loaded, reload, alive);
        });
    }
    tick(loaded, reload, alive.clone());
    alive
}

/// Whether the loaded page's file changed on disk since it was read; None once the page's signal is disposed.
fn changed_on_disk(loaded: RwSignal<Option<Rc<Loaded>>>) -> Option<bool> {
    loaded.try_with_untracked(|loaded| {
        Some(loaded?.as_ref().is_some_and(|loaded| {
            std::fs::metadata(&loaded.path).and_then(|metadata| metadata.modified()).ok() != loaded.modified
        }))
    })
}

/// Opens a Markdown file rendered, as a page of the window's review (the active one, else the latest), when the
/// window has one. Returns whether it did; opening at a line goes to the editor instead (the caller's own path).
pub fn open_rendered(window_tab: &WindowTabData, path: &Path) -> bool {
    let markdown = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown"));
    if !markdown {
        return false;
    }
    let review = window_tab
        .review
        .active_review_untracked()
        .or_else(|| window_tab.review.open_reviews().into_iter().next());
    let Some(review) = review else {
        return false;
    };
    super::reveal_page(window_tab, ReviewPage::document(&review.id, path));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(id: &str, anchor: DocAnchor) -> ReviewThread {
        serde_json::from_value(serde_json::json!({
            "id": id, "number": 1, "status": "open", "intent": "comment", "createdAt": 0, "updatedAt": 0,
            "messages": [],
            "location": { "kind": "document", "uri": "file:///d/index.md", "docAnchor": anchor },
        }))
        .unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn comments_go_after_the_block_they_are_about() {
        let page = review_markdown::render(
            "---\nco-review: design\n---\n# Retry\n\n## Context\nThe job runner retries.\n\n```mermaid\n%% id: flow\nflowchart TD\n  a[A] --> b[B]\n```\n\n## Design\n- Retry failures\n  - Back off\n",
            "index.md",
        );
        let text = page.text.find("job runner").unwrap_or_default();
        let selection = review_markdown::text_anchor(&page.text, text, text + "job runner".len())
            .unwrap_or_else(|| panic!("an anchor"));
        let threads = vec![
            thread("text", selection),
            thread("node", DocAnchor {
                anchor_type: DocAnchorType::MermaidNode,
                block_id: Some("flow".into()),
                node_id: Some("a".into()),
                ..anchor_base()
            }),
            thread("step", DocAnchor {
                anchor_type: DocAnchorType::TreeNode,
                node_id: Some("Back off".into()),
                ..anchor_base()
            }),
            thread("doc", DocAnchor { anchor_type: DocAnchorType::Document, ..anchor_base() }),
            thread("gone", DocAnchor {
                anchor_type: DocAnchorType::TreeNode,
                node_id: Some("Removed step".into()),
                ..anchor_base()
            }),
        ];
        let placement = place(&page, &threads, &[], &[], &HashSet::new(), &HashSet::new());
        let slot_of = |id: &str| {
            placement
                .slots
                .iter()
                .find(|(_, items)| items.contains(&Item::Thread(id.to_string())))
                .map(|(key, _)| *key)
        };
        let leaves = leaves(&page);
        let paragraph = leaves.ranges.iter().find(|(start, end)| *start <= text && text < *end).map(|(start, _)| *start);
        assert_eq!(slot_of("text"), paragraph);
        assert_eq!(slot_of("node"), leaves.diagrams.get("flow").copied());
        assert_eq!(slot_of("step"), leaves.steps.get("Back off").copied());
        assert_eq!(slot_of("doc"), Some(TOP));
        // A step that is gone: above the page.
        assert_eq!(slot_of("gone"), Some(TOP));
        assert_eq!(placement.marks.len(), 1);
        assert_eq!(placement.marks[0].start, text);
        assert_eq!(placement.steps, ["Back off"]);
        assert_eq!(placement.parts, [("flow".to_string(), "a".to_string())]);
    }

    #[test]
    fn a_page_whose_window_closed_stops_watching_its_file() {
        let scope = Scope::new();
        let loaded: RwSignal<Option<Rc<Loaded>>> = scope.create_rw_signal(None);
        assert_eq!(changed_on_disk(loaded), Some(false));
        // Closing a window disposes its scope; the next poll must stop, not panic (it quit the whole app).
        scope.dispose();
        assert_eq!(changed_on_disk(loaded), None);
    }
}
