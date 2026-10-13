//! The floating **Comment** / **Ask Agent** bar under a selection in a code editor (inline-review: starting a thread
//! from a selection is one click away). It shows once the selection settles (the mouse is up and it hasn't changed
//! for a moment), just below the selection's end (above when there's no room), while the editor has focus and the
//! file is in the workspace; it hides when the selection is empty (Escape clears it), changes, scrolls off screen
//! or the editor loses focus. **Comment on Symbol** shows when the selection is one identifier. Its buttons do what
//! the `Review:` commands do for the selection. It lives on the threads overlay ([`super::inline::editor_threads`]),
//! which lets pointer events through; only the bar takes them.

use std::{rc::Rc, time::Duration};

use floem::{
    IntoView, View,
    action::exec_after,
    event::{Event, EventListener},
    kurbo::{Point, Rect},
    reactive::{
        RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo,
        create_rw_signal,
    },
    style::CursorStyle,
    views::{Decorators, dyn_container, h_stack_from_iter, label},
};
use lapce_core::buffer::rope_text::RopeText;
use review_model::ThreadIntent;

use super::inline::{comment_on_selection, comment_on_symbol, is_identifier, selected_range};
use crate::{
    config::color::LapceColor, editor::EditorData, window_tab::{Focus, WindowTabData},
};

/// How long a selection stays unchanged before the bar shows.
const SETTLE: Duration = Duration::from_millis(200);
/// Space between the selection's line and the bar.
const GAP: f64 = 4.0;
/// Space kept between the bar and the overlay's edges.
const MARGIN: f64 = 4.0;

/// A selection: its offsets, `start < end`.
type Span = (usize, usize);

/// Whether the bar shows for the current selection.
#[derive(Clone, Copy, Debug, Default)]
struct BarState {
    /// The current non-empty selection.
    selection: Option<Span>,
    /// The selection that stayed unchanged with the mouse up.
    settled: Option<Span>,
    /// The selection a bar button was used on.
    used: Option<Span>,
    /// The selected text is more than whitespace.
    has_text: bool,
    /// The mouse is down in the editor (a selection being dragged).
    dragging: bool,
    focused: bool,
    in_workspace: bool,
}

fn shows(state: BarState) -> bool {
    let Some(selection) = state.selection else {
        return false;
    };
    state.settled == Some(selection)
        && state.used != Some(selection)
        && state.has_text
        && !state.dragging
        && state.focused
        && state.in_workspace
}

/// Where the bar goes in the overlay, from the selection end's row (`row_top`..`row_bottom`) and `x`, in overlay
/// coordinates: below the row, or above it when there's no room below. `None` when the row is off the visible text
/// (`text_top`..`text_bottom`).
fn place(
    row_top: f64,
    row_bottom: f64,
    x: f64,
    bar: (f64, f64),
    overlay: (f64, f64),
    text_top: f64,
    text_bottom: f64,
) -> Option<Point> {
    let (bar_width, bar_height) = bar;
    let (overlay_width, overlay_height) = overlay;
    if row_bottom <= text_top || row_top >= text_bottom {
        return None;
    }
    let below = row_bottom + GAP;
    let above = row_top - GAP - bar_height;
    let y = if below + bar_height <= overlay_height.min(text_bottom) || above < text_top.max(0.0) {
        below
    } else {
        above
    };
    let x = x.min(overlay_width - bar_width - MARGIN).max(MARGIN);
    Some(Point::new(x, y))
}

/// The bar for an editor (on its threads overlay, whose window origin and size are `origin` and `size`).
pub fn selection_bar(
    window_tab: Rc<WindowTabData>,
    e_data: RwSignal<EditorData>,
    origin: RwSignal<Point>,
    size: RwSignal<Rect>,
) -> impl View {
    let (editor, editor_id, doc_signal, cursor, content_origin, find_focus) = e_data.with_untracked(|e| {
        (e.editor.clone(), e.id(), e.doc_signal(), e.cursor(), e.window_origin(), e.find_focus)
    });
    let config = window_tab.common.config;
    let focus = window_tab.common.focus;
    let active_editor = window_tab.main_split.active_editor;
    let dragging = editor.active;
    let (viewport, screen_lines, scroll_delta) = (editor.viewport, editor.screen_lines, editor.scroll_delta);

    let selection = create_memo(move |_| {
        let (start, end) = cursor.with(|cursor| selected_range(&cursor.mode));
        (start < end).then_some((start, end))
    });
    let settled = create_rw_signal(None::<Span>);
    let used = create_rw_signal(None::<Span>);
    let bar_size = create_rw_signal((0.0_f64, 0.0_f64));

    // Settles a selection once it stayed the same, with the mouse up, for a moment.
    create_effect(move |_| {
        let current = selection.get();
        let drag = dragging.get();
        if settled.get_untracked() != current {
            settled.set(None);
        }
        let Some(current) = current.filter(|_| !drag) else {
            return;
        };
        exec_after(SETTLE, move |_| {
            let still = selection.try_get_untracked() == Some(Some(current))
                && dragging.try_get_untracked() == Some(false);
            if still {
                settled.try_update(|settled| *settled = Some(current));
            }
        });
    });

    // What the settled selection is: more than whitespace, and whether it's one identifier.
    let selected = create_memo(move |_| {
        let (start, end) = settled.get()?;
        let doc = doc_signal.get();
        doc.buffer.with(|buffer| {
            let end = end.min(buffer.len());
            (start < end).then(|| {
                let text = buffer.slice_to_cow(start..end);
                (!text.trim().is_empty(), is_identifier(&text))
            })
        })
    });
    let in_workspace = {
        let root = window_tab.review.root.clone();
        create_memo(move |_| {
            let path = doc_signal.with(|doc| doc.content.with(|content| content.path().cloned()));
            match (path, &root) {
                (Some(path), Some(root)) => review_store::real_path(&path).starts_with(root),
                _ => false,
            }
        })
    };
    let focused = create_memo(move |_| {
        focus.with(|focus| matches!(focus, Focus::Workbench))
            && !find_focus.get()
            && active_editor.with(|editor| editor.as_ref().map(|editor| editor.id())) == Some(editor_id)
    });
    let visible = create_memo(move |_| {
        shows(BarState {
            selection: selection.get(),
            settled: settled.get(),
            used: used.get(),
            has_text: selected.get().is_some_and(|(has_text, _)| has_text),
            dragging: dragging.get(),
            focused: focused.get(),
            in_workspace: in_workspace.get(),
        })
    });
    let has_agent = {
        let review = window_tab.review.clone();
        create_memo(move |_| review.active_review().is_some_and(|review| review.agent.is_some()))
    };

    // The bar's place, from the editor's layout of the selection's end (the caret).
    let position = {
        let editor = editor.clone();
        create_memo(move |_| {
            if !visible.get() {
                return None;
            }
            let (offset, affinity) = cursor.with(|cursor| (cursor.offset(), cursor.affinity));
            let viewport = viewport.get();
            let line_height = config.get().editor.line_height() as f64;
            let row_y = screen_lines.with(|screen_lines| {
                screen_lines.iter_line_info().find_map(|info| {
                    let interval = info.vline_info.interval;
                    (interval.start <= offset && offset <= interval.end).then_some(info.vline_y)
                })
            })?;
            let x = editor.line_point_of_offset(offset, affinity).x;
            let (content, overlay) = (content_origin.get(), origin.get());
            let (dx, dy) = (content.x - overlay.x, content.y - overlay.y);
            let row_top = dy + row_y - viewport.y0;
            let size = size.get();
            place(
                row_top,
                row_top + line_height,
                dx + x - viewport.x0,
                bar_size.get(),
                (size.width(), size.height()),
                dy,
                dy + viewport.height(),
            )
        })
    };

    let button = move |text: &'static str, primary: bool, run: Rc<dyn Fn()>| {
        label(move || text)
            // On press, so the editor keeps its selection and focus.
            .on_event_stop(EventListener::PointerDown, move |_| run())
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .margin_right(4.0)
                    .border_radius(4.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .apply_if(primary, |s| {
                        s.background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
                            .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
                    })
                    .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
            })
    };
    let act = {
        let window_tab = window_tab.clone();
        Rc::new(move |action: Option<ThreadIntent>| {
            used.set(selection.get_untracked());
            match action {
                Some(intent) => comment_on_selection(&window_tab, intent),
                None => comment_on_symbol(&window_tab),
            }
        })
    };
    let multi_line = create_memo(move |_| {
        let (start, end) = settled.get()?;
        let doc = doc_signal.get();
        Some(doc.buffer.with(|buffer| {
            buffer.line_of_offset(start) != buffer.line_of_offset(end.min(buffer.len()))
        }))
    });
    dyn_container(
        move || {
            (
                has_agent.get(),
                multi_line.get().unwrap_or(false),
                selected.get().is_some_and(|(_, identifier)| identifier),
            )
        },
        move |(agent, multi_line, identifier)| {
            let mut buttons: Vec<floem::AnyView> = Vec::new();
            if agent {
                let act = act.clone();
                buttons.push(
                    button("Ask Agent", multi_line, Rc::new(move || act(Some(ThreadIntent::Question)))).into_any(),
                );
            }
            let comment = act.clone();
            buttons.push(
                button("Comment", !agent || !multi_line, Rc::new(move || comment(Some(ThreadIntent::Comment))))
                    .into_any(),
            );
            if identifier {
                let symbol = act.clone();
                buttons.push(button("Comment on Symbol", false, Rc::new(move || symbol(None))).into_any());
            }
            h_stack_from_iter(buttons).into_any()
        },
    )
    .on_resize(move |rect| {
        let measured = (rect.width(), rect.height());
        if bar_size.get_untracked() != measured {
            bar_size.set(measured);
        }
    })
    .on_event_stop(EventListener::PointerDown, |_| {})
    // Over the code, so it scrolls the editor as the code under it does.
    .on_event_stop(EventListener::PointerWheel, move |event| {
        if let Event::PointerWheel(pointer) = event {
            scroll_delta.set(pointer.delta);
        }
    })
    .style(move |s| {
        let config = config.get();
        match position.get() {
            Some(point) => s
                .absolute()
                .pointer_events_auto()
                .inset_left(point.x as f32)
                .inset_top(point.y as f32)
                .padding(4.0)
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_BACKGROUND))
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .box_shadow_blur(6.0)
                .box_shadow_color(config.color(LapceColor::LAPCE_DROPDOWN_SHADOW))
                .z_index(10),
            None => s.hide(),
        }
    })
    .debug_name("Review Selection Bar")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled_selection() -> BarState {
        BarState {
            selection: Some((10, 20)),
            settled: Some((10, 20)),
            used: None,
            has_text: true,
            dragging: false,
            focused: true,
            in_workspace: true,
        }
    }

    #[test]
    fn a_settled_selection_in_a_focused_workspace_file_shows_the_bar() {
        assert!(shows(settled_selection()));
    }

    #[test]
    fn the_bar_hides_until_the_selection_settles() {
        assert!(!shows(BarState { dragging: true, ..settled_selection() }));
        // Changed since it settled (extended with the keyboard, or a new drag).
        assert!(!shows(BarState { selection: Some((10, 25)), ..settled_selection() }));
        assert!(!shows(BarState { settled: None, ..settled_selection() }));
    }

    #[test]
    fn the_bar_hides_without_a_selection_focus_or_workspace_file() {
        assert!(!shows(BarState { selection: None, ..settled_selection() }));
        assert!(!shows(BarState { has_text: false, ..settled_selection() }));
        assert!(!shows(BarState { focused: false, ..settled_selection() }));
        assert!(!shows(BarState { in_workspace: false, ..settled_selection() }));
    }

    #[test]
    fn the_bar_hides_once_used_until_the_selection_changes() {
        assert!(!shows(BarState { used: Some((10, 20)), ..settled_selection() }));
        assert!(shows(BarState { used: Some((0, 5)), ..settled_selection() }));
    }

    #[test]
    fn the_bar_goes_below_the_selection_end_or_above_without_room() {
        let bar = (200.0, 30.0);
        let overlay = (800.0, 600.0);
        assert_eq!(
            place(100.0, 120.0, 300.0, bar, overlay, 0.0, 600.0),
            Some(Point::new(300.0, 124.0))
        );
        // Near the bottom: above the row.
        assert_eq!(
            place(560.0, 580.0, 300.0, bar, overlay, 0.0, 600.0),
            Some(Point::new(300.0, 526.0))
        );
        // Near the right edge: kept inside.
        assert_eq!(
            place(100.0, 120.0, 790.0, bar, overlay, 0.0, 600.0),
            Some(Point::new(596.0, 124.0))
        );
    }

    #[test]
    fn the_bar_hides_when_the_selection_end_scrolls_off_screen() {
        let (bar, overlay) = ((200.0, 30.0), (800.0, 600.0));
        assert_eq!(place(-40.0, -20.0, 300.0, bar, overlay, 0.0, 600.0), None);
        assert_eq!(place(610.0, 630.0, 300.0, bar, overlay, 0.0, 600.0), None);
        // Below the breadcrumbs: the text starts lower in the overlay.
        assert_eq!(place(10.0, 30.0, 300.0, bar, overlay, 30.0, 600.0), None);
    }
}
