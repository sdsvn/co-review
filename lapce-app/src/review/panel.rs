//! The Review panel (`PanelKind::Review`). Sections are added with `PanelBuilder`, each folding state a
//! `PanelSection` variant.

use std::{rc::Rc, sync::Arc};

use floem::{
    IntoView, View,
    reactive::{SignalGet, SignalWith},
    style::CursorStyle,
    views::{Decorators, container, dyn_container, dyn_stack, label, scroll},
};
use review_model::Review;

use super::{ReviewPage, reveal_page};
use crate::{
    config::color::LapceColor,
    panel::{data::PanelSection, position::PanelPosition, view::PanelBuilder},
    window_tab::WindowTabData,
};

pub fn review_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    PanelBuilder::new(config, position)
        .add(
            "Reviews",
            reviews_section(window_tab_data.clone()),
            window_tab_data.panel.section_open(PanelSection::Reviews),
        )
        .build()
        .debug_name("Review Panel")
}

/// The workspace's open reviews by title, most recently updated first; a click opens the review's overview.
fn reviews_section(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let review = window_tab_data.review.clone();
    let (reviews, loaded) = (review.reviews, review.loaded);
    let has_open = move || {
        reviews.with(|reviews| {
            reviews.iter().any(|review| review.archived_at.is_none())
        })
    };
    let message = move |text: &'static str| {
        label(move || text).style(move |s| {
            s.padding_horiz(10.0)
                .padding_vert(6.0)
                .color(config.get().color(LapceColor::PANEL_FOREGROUND_DIM))
        })
    };
    let list =
        move || {
            let window_tab_data = window_tab_data.clone();
            scroll(
                dyn_stack(
                    move || {
                        reviews.with(|reviews| {
                            reviews
                                .iter()
                                .filter(|review| review.archived_at.is_none())
                                .cloned()
                                .collect::<Vec<Arc<Review>>>()
                        })
                    },
                    |review| (review.id.clone(), review.title.clone()),
                    move |review| {
                        let window_tab_data = window_tab_data.clone();
                        let title = review.title.clone();
                        label(move || title.clone())
                            .on_click_stop(move |_| {
                                reveal_page(
                                    &window_tab_data,
                                    ReviewPage::overview(&review.id),
                                );
                            })
                            .style(move |s| {
                                let config = config.get();
                                s.padding_horiz(10.0)
                                    .padding_vert(4.0)
                                    .width_full()
                                    .cursor(CursorStyle::Pointer)
                                    .hover(|s| {
                                        s.background(config.color(
                                            LapceColor::PANEL_HOVERED_BACKGROUND,
                                        ))
                                    })
                            })
                    },
                )
                .style(|s| s.flex_col().width_full()),
            )
            .style(|s| s.absolute().size_full())
        };
    container(
        dyn_container(
            move || (loaded.get(), has_open()),
            move |state| match state {
                (false, _) => message("Loading reviews…").into_any(),
                (true, false) => message("No reviews yet").into_any(),
                (true, true) => list().into_any(),
            },
        )
        .style(|s| s.size_full()),
    )
    .style(|s| s.size_full())
}
