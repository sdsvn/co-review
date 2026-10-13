//! Review pages: editor-tab children (like Settings and Keymap) that show a review's overview, document or pull
//! request. Opened with `MainSplitData::open_review_page`, or `review::reveal_page` to show the Review panel too.

use std::{rc::Rc, sync::Arc};

use floem::{
    IntoView, View,
    reactive::{Memo, ReadSignal, SignalGet, create_memo},
};
use serde::{Deserialize, Serialize};

use crate::{
    config::{LapceConfig, color::LapceColor, icon::LapceIcons},
    editor_tab::EditorTabChildViewInfo,
    window_tab::WindowTabData,
};

pub type ReviewPageId = floem::views::editor::id::Id;

/// Which review page a tab shows. A review has at most one tab per page: opening it again selects that tab.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPage {
    pub review_id: String,
    pub kind: ReviewPageKind,
    /// A document page's file (`document.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The tab's title, when not the page kind's or the file's name (a design's document: the design's title). Not
    /// part of which page it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PartialEq for ReviewPage {
    fn eq(&self, other: &Self) -> bool {
        self.review_id == other.review_id && self.kind == other.kind && self.path == other.path
    }
}

impl Eq for ReviewPage {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReviewPageKind {
    Overview,
    /// A change review's patches as a pull request (`pull_request.rs`).
    PullRequest,
    /// A Markdown document of the review, rendered (`document.rs`).
    Document,
}

impl ReviewPage {
    fn new(review_id: &str, kind: ReviewPageKind, path: Option<String>) -> Self {
        Self { review_id: review_id.to_string(), kind, path, title: None }
    }

    pub fn overview(review_id: &str) -> Self {
        Self::new(review_id, ReviewPageKind::Overview, None)
    }

    pub fn pull_request(review_id: &str) -> Self {
        Self::new(review_id, ReviewPageKind::PullRequest, None)
    }

    pub fn document(review_id: &str, path: &std::path::Path) -> Self {
        Self::new(review_id, ReviewPageKind::Document, Some(path.to_string_lossy().into_owned()))
    }

    /// The same page, its tab titled `title`.
    pub fn titled(self, title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..self
        }
    }
}

impl ReviewPageKind {
    pub fn name(self) -> &'static str {
        match self {
            ReviewPageKind::Overview => "Overview",
            ReviewPageKind::PullRequest => "Pull Request",
            ReviewPageKind::Document => "Document",
        }
    }
}

/// The tab header of a review page.
pub fn review_page_view_info(
    page: &ReviewPage,
    config: ReadSignal<Arc<LapceConfig>>,
) -> Memo<EditorTabChildViewInfo> {
    let name = page.title.clone().unwrap_or_else(|| match &page.path {
        Some(path) if page.kind == ReviewPageKind::Document => std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone()),
        _ => page.kind.name().to_string(),
    });
    let path = page.path.clone().map(std::path::PathBuf::from);
    create_memo(move |_| {
        let config = config.get();
        EditorTabChildViewInfo {
            icon: config.ui_svg(LapceIcons::REVIEW),
            color: Some(config.color(LapceColor::LAPCE_ICON_ACTIVE)),
            name: name.clone(),
            path: path.clone(),
            confirmed: None,
            is_pristine: true,
        }
    })
}

/// The content of a review page's tab.
pub fn review_page_view(window_tab_data: Rc<WindowTabData>, page: ReviewPage) -> impl View {
    match page.kind {
        ReviewPageKind::Overview | ReviewPageKind::Document => {
            super::document::document_page(window_tab_data, page).into_any()
        }
        ReviewPageKind::PullRequest => {
            super::pull_request::pull_request_page(window_tab_data, page.review_id).into_any()
        }
    }
}
