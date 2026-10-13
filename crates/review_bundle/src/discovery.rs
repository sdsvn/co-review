/// Review-directory document and patch discovery (pure functions over file name lists).
use crate::patch::DOCUMENT_NAMES;

/// Find the document of a review directory from a list of file names in that directory.
/// Returns the first match in priority order: `index.markdown`, `index.md`, `INDEX.md`,
/// `README.md`, `readme.md`, then the alphabetically first `*.pseudocode.md`.
pub fn document_of(file_names: &[&str]) -> Option<String> {
    DOCUMENT_NAMES
        .iter()
        .find(|name| file_names.contains(name))
        .or_else(|| {
            file_names
                .iter()
                .filter(|name| name.ends_with(".pseudocode.md"))
                .min()
        })
        .map(|name| (*name).to_string())
}

/// Find patch/diff files in a review directory from a list of file names.
/// Returns them sorted alphabetically (matching the TS behavior).
pub fn patches_of(file_names: &[&str]) -> Vec<String> {
    let mut patches: Vec<String> = file_names
        .iter()
        .filter(|f| f.ends_with(".patch") || f.ends_with(".diff"))
        .map(|f| (*f).to_string())
        .collect();
    patches.sort();
    patches
}
