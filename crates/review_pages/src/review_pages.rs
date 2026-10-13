//! Co-Review's overview pages: the Markdown front page of every review, kept up to date as the
//! review changes, with cheap sections written at once and slow sections (repository overview,
//! blast radius) built in the background.

mod overview_page;
mod pages;
mod repo_overview;
mod text;

pub use overview_page::{OverviewParts, PatchFileSummary, PatchSummary, Radius, overview_page};
pub use pages::{COALESCE_DELAY, OverviewPages, RepositorySections, SectionBuilder};
pub use repo_overview::{ENTRY_POINT, OverviewInput, map_overview, repository_overview};
