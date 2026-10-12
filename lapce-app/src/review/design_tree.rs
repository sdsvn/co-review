//! A design's step tree (`## Design`, `## Open Questions`, `*.pseudocode.md`): L1 · L2 · L3 steps, folded to the
//! level chosen in the section's header, with their `?` `⚠` `✎` badges, the reason after ` — ` muted, and a comment
//! action per step (`tree-node` anchors, by `review_design::step_id`).

use floem::{
    IntoView, View,
    reactive::{SignalGet, SignalUpdate, SignalWith, create_effect, create_rw_signal},
    style::CursorStyle,
    views::{Decorators, dyn_container, empty, h_stack, label, v_stack_from_iter},
};
use review_markdown::StepNode;
use review_model::{DocAnchor, DocAnchorType};

use super::markdown_view::{Action, PageContext, comment_icon, step_text, with_slot};
use crate::config::color::LapceColor;

/// The steps of a tree; `design`: the Design section's, which the level switch folds.
pub fn tree_view(ctx: &PageContext, steps: &[StepNode], design: bool) -> impl View + use<> {
    let rows: Vec<floem::AnyView> = steps.iter().map(|step| step_view(ctx, step, design)).collect();
    v_stack_from_iter(rows).style(|s| s.width_full().margin_vert(4.0))
}

fn step_view(ctx: &PageContext, step: &StepNode, design: bool) -> floem::AnyView {
    let config = ctx.config;
    let has_children = !step.children.is_empty();
    let depth = step.depth;
    // Unfolded below the chosen level (the Design tree), or at the top level only (other trees); a click toggles it.
    let open = create_rw_signal(if design { depth < ctx.level.get_untracked() } else { depth == 1 });
    if design {
        let level = ctx.level;
        create_effect(move |_| {
            let level = level.get();
            open.set(depth < level);
        });
    }
    let chevron = label(move || if !has_children { " " } else if open.get() { "\u{25be}" } else { "\u{25b8}" })
        .on_click_stop(move |_| {
            if has_children {
                open.update(|open| *open = !*open);
            }
        })
        .style(move |s| {
            s.width(16.0)
                .margin_top(3.0)
                .selectable(false)
                .color(config.get().color(LapceColor::EDITOR_DIM))
                .apply_if(has_children, |s| s.cursor(CursorStyle::Pointer))
        });
    let comment = {
        let on_comment = ctx.on_comment.clone();
        let anchor = DocAnchor {
            anchor_type: DocAnchorType::TreeNode,
            node_id: Some(step.id.clone()),
            source: Some(step.text.text.clone()),
            ..anchor_base()
        };
        comment_icon(config, "Comment on this step", move || on_comment(anchor.clone(), Action::Comment))
    };
    let marked = {
        let (marked_steps, id) = (ctx.marked_steps, step.id.clone());
        move || marked_steps.with(|steps| steps.contains(&id))
    };
    let line = h_stack((
        chevron,
        step_text(ctx, step).style(|s| s.flex_grow(1.0_f32).flex_basis(0.0_f32)),
        comment,
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .items_start()
            .padding_vert(1.0)
            .border_radius(4.0)
            .apply_if(marked(), |s| s.background(config.color(LapceColor::LAPCE_WARN).multiply_alpha(0.12)))
            .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
    });
    let line = with_slot(ctx, step.text.start, line);
    let children = if has_children {
        let ctx = ctx.clone();
        let steps = step.children.clone();
        dyn_container(
            move || open.get(),
            move |open| {
                if open {
                    let rows: Vec<floem::AnyView> = steps.iter().map(|child| step_view(&ctx, child, design)).collect();
                    v_stack_from_iter(rows)
                        .style(move |s| {
                            s.width_full()
                                .margin_left(8.0)
                                .padding_left(8.0)
                                .border_left(1.0)
                                .border_color(config.get().color(LapceColor::LAPCE_BORDER))
                        })
                        .into_any()
                } else {
                    empty().into_any()
                }
            },
        )
        .style(|s| s.width_full())
        .into_any()
    } else {
        empty().into_any()
    };
    v_stack_from_iter([line, children]).style(|s| s.width_full()).into_any()
}

/// A document anchor with no fields set.
pub fn anchor_base() -> DocAnchor {
    DocAnchor {
        anchor_type: DocAnchorType::Document,
        exact: None,
        prefix: None,
        suffix: None,
        start_offset_hint: None,
        end_offset_hint: None,
        block_id: None,
        node_id: None,
        edge_id: None,
        source: None,
        occurrence: None,
        extra: Default::default(),
    }
}
