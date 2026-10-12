/// Review-directory document and patch discovery (pure functions over file name lists).
use crate::patch::DOCUMENT_NAMES;

/// Find the document of a review directory from a list of file names in that directory.
/// Returns the first match in priority order: `index.markdown`, `index.md`, `INDEX.md`,
/// `README.md`, `readme.md`, then the alphabetically first `*.pseudocode.md`.
pub fn document_of(file_names: &[&str]) -> Option<String> {
    for name in DOCUMENT_NAMES {
        if file_names.contains(name) {
            return Some((*name).to_string());
        }
    }
    let mut pseudocode: Vec<&str> = file_names
        .iter()
        .filter(|f| f.ends_with(".pseudocode.md"))
        .copied()
        .collect();
    pseudocode.sort();
    pseudocode.first().map(|s| (*s).to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_document_of_priority() {
        let files = vec!["README.md", "index.md", "foo.txt"];
        assert_eq!(document_of(&files), Some("index.md".to_string()));
    }

    #[test]
    fn test_document_of_pseudocode_fallback() {
        let files = vec!["foo.txt", "beta.pseudocode.md", "alpha.pseudocode.md"];
        assert_eq!(document_of(&files), Some("alpha.pseudocode.md".to_string()));
    }

    #[test]
    fn test_document_of_none() {
        let files = vec!["foo.txt", "bar.rs"];
        assert_eq!(document_of(&files), None);
    }

    #[test]
    fn test_patches_of() {
        let files = vec!["z.patch", "a.diff", "readme.md", "b.patch"];
        assert_eq!(patches_of(&files), vec!["a.diff", "b.patch", "z.patch"]);
    }

    #[test]
    fn test_patches_of_empty() {
        let files: Vec<&str> = vec!["readme.md"];
        let result: Vec<String> = patches_of(&files);
        assert!(result.is_empty());
    }
}
