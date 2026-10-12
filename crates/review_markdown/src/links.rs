//! Links on a page (`openLink` in `document-review-widget.tsx`): the web, a patch of the review directory, another
//! page, or a place in the code (`src/a.ts:42`, `src/a.ts:42-50`, `src/a.ts#L42-L50`).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// A web page, or any other `scheme:` link: the system opens it.
    External(String),
    /// `#/patch/<name>`: the review directory's `<name>.patch`.
    Patch(String),
    /// `#…`: a place on the same page.
    Fragment(String),
    /// A file or folder: `path` as written (absolute ones are from the review directory), with a line range.
    Path {
        path: String,
        line: Option<u32>,
        end_line: Option<u32>,
    },
}

/// What a link's `href` points at.
pub fn parse_link(href: &str) -> LinkTarget {
    static SCHEME: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").ok());
    static PLACE: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"^(.*?)(?::(\d+)(?:-(\d+))?|#L(\d+)(?:-L?(\d+))?)?$").ok());
    if let Some(patch) = href.strip_prefix("#/patch/") {
        return LinkTarget::Patch(patch.to_string());
    }
    if href.starts_with('#') {
        return LinkTarget::Fragment(href.to_string());
    }
    // A Windows drive (`C:\…`) is a path, not a scheme.
    let drive = href.len() > 2 && href.as_bytes().get(1) == Some(&b':') && matches!(href.as_bytes().get(2), Some(b'\\' | b'/'));
    if !drive && SCHEME.as_ref().is_some_and(|scheme| scheme.is_match(href)) && !href.starts_with("file:") {
        return LinkTarget::External(href.to_string());
    }
    let decoded = percent_decode(href.strip_prefix("file://").unwrap_or(href));
    let captures = PLACE.as_ref().and_then(|place| place.captures(&decoded));
    let group = |index: usize| {
        captures
            .as_ref()
            .and_then(|captures| captures.get(index))
            .and_then(|found| found.as_str().parse::<u32>().ok())
            .filter(|line| *line > 0)
    };
    let path = captures
        .as_ref()
        .and_then(|captures| captures.get(1))
        .map_or(decoded.as_str(), |found| found.as_str());
    let path = path.split('#').next().unwrap_or("").to_string();
    LinkTarget::Path {
        path,
        line: group(2).or_else(|| group(4)),
        end_line: group(3).or_else(|| group(5)),
    }
}

/// The file or folder a path link names: tried next to the page, then from the review directory and the folder
/// above it (the repository, for a bundle such as `<repo>/okf`), then from the workspace; an absolute path only from
/// the review directory.
pub fn resolve_link(path: &str, page_dir: &Path, bundle_dir: Option<&Path>, root: Option<&Path>) -> Option<PathBuf> {
    let relative = path.trim_start_matches('/');
    let bases: Vec<PathBuf> = if path.starts_with('/') {
        bundle_dir.map(Path::to_path_buf).into_iter().collect()
    } else {
        let mut bases = vec![page_dir.to_path_buf()];
        if let Some(dir) = bundle_dir {
            bases.push(dir.to_path_buf());
            bases.extend(dir.parent().map(Path::to_path_buf));
        }
        bases.extend(root.map(Path::to_path_buf));
        bases
    };
    bases
        .into_iter()
        .map(|base| if relative.is_empty() { base } else { base.join(relative) })
        .find(|candidate| candidate.exists())
}

/// `decodeURI`, leniently: `%XX` sequences that don't make UTF-8 stay as they are.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        match (byte, hex) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                index += 3;
            }
            _ => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(path: &str, line: Option<u32>, end_line: Option<u32>) -> LinkTarget {
        LinkTarget::Path {
            path: path.to_string(),
            line,
            end_line,
        }
    }

    #[test]
    fn links_name_code_places_pages_and_the_web() {
        assert_eq!(parse_link("src/limiter.go:42"), place("src/limiter.go", Some(42), None));
        assert_eq!(parse_link("src/a.ts:42-50"), place("src/a.ts", Some(42), Some(50)));
        assert_eq!(parse_link("src/a.ts#L10-L20"), place("src/a.ts", Some(10), Some(20)));
        assert_eq!(parse_link("src/a.ts#L7"), place("src/a.ts", Some(7), None));
        assert_eq!(parse_link("/packages/store.md"), place("/packages/store.md", None, None));
        assert_eq!(parse_link("docs/x.md#usage"), place("docs/x.md", None, None));
        assert_eq!(parse_link("my%20file.md"), place("my file.md", None, None));
        assert_eq!(parse_link("https://example.com/a:1"), LinkTarget::External("https://example.com/a:1".into()));
        assert_eq!(parse_link("mailto:a@b.c"), LinkTarget::External("mailto:a@b.c".into()));
        assert_eq!(parse_link("#/patch/limiter"), LinkTarget::Patch("limiter".into()));
        assert_eq!(parse_link("#usage"), LinkTarget::Fragment("#usage".into()));
    }

    #[test]
    fn paths_resolve_next_to_the_page_then_from_the_bundle_and_the_repository() {
        let repo = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let bundle = repo.path().join("okf");
        let pages = bundle.join("packages");
        std::fs::create_dir_all(&pages).ok();
        std::fs::create_dir_all(repo.path().join("src")).ok();
        std::fs::write(repo.path().join("src/limiter.go"), "").ok();
        std::fs::write(bundle.join("index.md"), "").ok();
        std::fs::write(pages.join("store.md"), "").ok();
        assert_eq!(
            resolve_link("src/limiter.go", &pages, Some(&bundle), None),
            Some(repo.path().join("src/limiter.go"))
        );
        assert_eq!(resolve_link("store.md", &pages, Some(&bundle), None), Some(pages.join("store.md")));
        assert_eq!(resolve_link("/index.md", &pages, Some(&bundle), None), Some(bundle.join("index.md")));
        assert_eq!(resolve_link("/src/limiter.go", &pages, Some(&bundle), None), None);
        assert_eq!(resolve_link("missing.go", &pages, Some(&bundle), None), None);
    }
}
