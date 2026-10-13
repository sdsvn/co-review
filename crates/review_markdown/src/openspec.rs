//! A review directory's OpenSpec change, shown under its document as cards: the proposal, then one card per
//! capability spec (`renderOpenSpec` in `document-review-widget.tsx`).

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSpecCard {
    pub title: String,
    pub markdown: String,
    /// Shown open at first (the proposal).
    pub open: bool,
}

/// The cards of the OpenSpec change in `dir` (`proposal.md`, `specs/<capability>/spec.md`). Reads files: call it
/// off the UI thread.
pub fn openspec_cards(dir: &Path) -> Vec<OpenSpecCard> {
    let mut cards = Vec::new();
    if let Ok(proposal) = fs::read_to_string(dir.join("proposal.md")) {
        static HEADING: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?m)^#\s+([^\r\n]+)$").ok());
        static FIRST_LINE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^#\s+[^\r\n]+\n").ok());
        let heading = HEADING
            .as_ref()
            .and_then(|heading| heading.captures(&proposal))
            .and_then(|captures| captures.get(1))
            .map(|heading| heading.as_str().trim());
        let markdown = match FIRST_LINE.as_ref() {
            Some(first) => first.replace(&proposal, "").into_owned(),
            None => proposal.clone(),
        };
        cards.push(OpenSpecCard {
            title: heading.map_or_else(|| "Proposal".to_string(), str::to_string),
            markdown,
            open: true,
        });
    }
    let mut capabilities: Vec<(String, std::path::PathBuf)> = fs::read_dir(dir.join("specs"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
                .collect()
        })
        .unwrap_or_default();
    capabilities.sort();
    for (name, path) in capabilities {
        if let Ok(spec) = fs::read_to_string(path.join("spec.md")) {
            cards.push(OpenSpecCard {
                title: format!("spec: {name}"),
                markdown: spec,
                open: false,
            });
        }
    }
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proposal_comes_first_then_the_specs_by_name() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        fs::write(dir.path().join("proposal.md"), "# Add limits\n\nWhy.\n").ok();
        for name in ["zeta", "alpha"] {
            fs::create_dir_all(dir.path().join("specs").join(name)).ok();
            fs::write(dir.path().join("specs").join(name).join("spec.md"), format!("## {name}\n")).ok();
        }
        let cards = openspec_cards(dir.path());
        let titles: Vec<&str> = cards.iter().map(|card| card.title.as_str()).collect();
        assert_eq!(titles, ["Add limits", "spec: alpha", "spec: zeta"]);
        assert_eq!(cards[0].markdown, "\nWhy.\n");
        assert!(cards[0].open && !cards[1].open);
    }
}
