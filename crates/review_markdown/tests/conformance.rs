//! Replays what the Theia app's document page shows (recorded by `tests/gen/record.mjs`): the page's text, the
//! diagrams' ids, the anchors of selections and where anchors are marked.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use pretty_assertions::assert_eq;
use review_markdown::{locate, render, text_anchor, utf16_len};
use review_model::DocAnchor;
use serde_json::Value;

fn fixtures() -> Vec<(String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no fixtures in {}", dir.display());
    files
        .into_iter()
        .map(|file| {
            let name = file.file_stem().unwrap().to_string_lossy().into_owned();
            (name, serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap())
        })
        .collect()
}

fn page_of(fixture: &Value) -> review_markdown::Page {
    render(fixture["markdown"].as_str().unwrap(), fixture["fileName"].as_str().unwrap())
}

#[test]
fn the_page_text_is_the_theia_pages() {
    for (name, fixture) in fixtures() {
        let page = page_of(&fixture);
        assert_eq!(page.text, fixture["text"].as_str().unwrap(), "{name}: page text");
        assert_eq!(page.title, fixture["title"].as_str().unwrap(), "{name}: title");
        assert_eq!(page.design, fixture["pseudocode"].as_bool().unwrap(), "{name}: design");
        let ids: Vec<&str> = page.diagrams.iter().map(|diagram| diagram.id.as_str()).collect();
        let codes: Vec<&str> = page.diagrams.iter().map(|diagram| diagram.code.as_str()).collect();
        let blocks = fixture["blocks"].as_array().unwrap();
        let expected_ids: Vec<&str> = blocks.iter().map(|block| block["id"].as_str().unwrap()).collect();
        let expected_codes: Vec<&str> = blocks.iter().map(|block| block["code"].as_str().unwrap()).collect();
        assert_eq!(ids, expected_ids, "{name}: diagram ids");
        assert_eq!(codes, expected_codes, "{name}: diagram code");
    }
}

#[test]
fn every_inline_is_where_the_page_text_has_it() {
    for (name, fixture) in fixtures() {
        let page = page_of(&fixture);
        let units: Vec<u16> = page.text.encode_utf16().collect();
        let mut last = 0;
        for inline in page.inlines() {
            let on_page = String::from_utf16_lossy(&units[inline.start..inline.start + utf16_len(&inline.text)]);
            assert_eq!(on_page, inline.text, "{name}: inline at {}", inline.start);
            assert!(inline.start >= last, "{name}: inlines out of order at {}", inline.start);
            last = inline.start;
        }
    }
}

#[test]
fn selections_anchor_as_the_theia_page_anchors_them() {
    for (name, fixture) in fixtures() {
        let page = page_of(&fixture);
        for selection in fixture["selections"].as_array().unwrap() {
            let start = selection["start"].as_u64().unwrap() as usize;
            let end = selection["end"].as_u64().unwrap() as usize;
            let anchor = text_anchor(&page.text, start, end);
            let expected: Option<DocAnchor> = serde_json::from_value(selection["anchor"].clone()).unwrap();
            assert_eq!(anchor, expected, "{name}: selection {}", selection["text"]);
            if let Some(anchor) = anchor {
                // An anchor marks the place it was made at.
                let hint = anchor.start_offset_hint.unwrap() as usize;
                assert_eq!(locate(&page.text, &anchor).map(|(at, _)| at), Some(hint), "{name}: {}", selection["text"]);
            }
        }
    }
}

#[test]
fn anchors_are_marked_where_the_theia_page_marks_them() {
    for (name, fixture) in fixtures() {
        let page = page_of(&fixture);
        for located in fixture["located"].as_array().unwrap() {
            let anchor: DocAnchor = serde_json::from_value(located["anchor"].clone()).unwrap();
            let at = located["at"].as_u64().map(|at| at as usize);
            assert_eq!(locate(&page.text, &anchor).map(|(at, _)| at), at, "{name}: {:?}", anchor.exact);
        }
    }
}
