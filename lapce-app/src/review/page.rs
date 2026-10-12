//! Review pages: editor-tab children (like Settings and Keymap) that show a review's overview, document or pull
//! request. Opened with `MainSplitData::open_review_page`, or `review::reveal_page` to show the Review panel too.

use std::{rc::Rc, sync::Arc};

use floem::{
    View,
    reactive::{Memo, ReadSignal, SignalGet, SignalWith, create_memo},
    views::{Decorators, container, label, stack},
};
use serde::{Deserialize, Serialize};

use crate::{
    config::{LapceConfig, color::LapceColor, icon::LapceIcons},
    editor_tab::EditorTabChildViewInfo,
    window_tab::WindowTabData,
};

pub type ReviewPageId = floem::views::editor::id::Id;

/// Which review page a tab shows. A review has at most one tab per page: opening it again selects that tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPage {
    pub review_id: String,
    pub kind: ReviewPageKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewPageKind {
    Overview,
}

impl ReviewPage {
    pub fn overview(review_id: &str) -> Self {
        Self {
            review_id: review_id.to_string(),
            kind: ReviewPageKind::Overview,
        }
    }
}

impl ReviewPageKind {
    pub fn name(self) -> &'static str {
        match self {
            ReviewPageKind::Overview => "Overview",
        }
    }
}

/// The tab header of a review page.
pub fn review_page_view_info(
    page: &ReviewPage,
    config: ReadSignal<Arc<LapceConfig>>,
) -> Memo<EditorTabChildViewInfo> {
    let name = page.kind.name();
    create_memo(move |_| {
        let config = config.get();
        EditorTabChildViewInfo {
            icon: config.ui_svg(LapceIcons::REVIEW),
            color: Some(config.color(LapceColor::LAPCE_ICON_ACTIVE)),
            name: name.to_string(),
            path: None,
            confirmed: None,
            is_pristine: true,
        }
    })
}

/// The content of a review page's tab.
pub fn review_page_view(
    window_tab_data: Rc<WindowTabData>,
    page: ReviewPage,
) -> impl View {
    let config = window_tab_data.common.config;
    let reviews = window_tab_data.review.reviews;
    let review_id = page.review_id.clone();
    let title = move || {
        reviews
            .with(|reviews| {
                reviews
                    .iter()
                    .find(|review| review.id == review_id)
                    .map(|review| review.title.clone())
            })
            .unwrap_or_else(|| "Review".to_string())
    };
    let kind = page.kind;
    container(
        stack((
            label(title).style(|s| s.font_size(20.0).font_bold()),
            label(move || kind.name()).style(move |s| {
                s.margin_top(4.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
            label(|| "This page is not built yet.").style(move |s| {
                s.margin_top(16.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
        ))
        .style(|s| s.flex_col().padding(24.0)),
    )
    .style(move |s| {
        s.size_full()
            .background(config.get().color(LapceColor::EDITOR_BACKGROUND))
            .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
    })
    .debug_name("Review Page")
}
