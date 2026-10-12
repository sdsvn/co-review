//! The Markdown matches what the TypeScript app writes, byte for byte, for the cases recorded by
//! `tests/gen/gen.mjs`.

use std::path::PathBuf;

use pretty_assertions::assert_eq;
use review_bundle::patch::PatchMeta;
use review_graph::{BlastRadius, GraphState};
use review_model::{Review, ReviewCoverage};
use review_pages::{
    OverviewInput, OverviewParts, PatchFileSummary, PatchSummary, Radius, overview_page,
    repository_overview,
};
use serde::Deserialize;
use serde_json::Value;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()))
}

#[derive(Deserialize)]
struct OverviewCase {
    name: String,
    review: Review,
    root: String,
    parts: Value,
    expected: String,
}

#[derive(Deserialize)]
struct RecordedPatchFile {
    path: String,
    added: u32,
    removed: u32,
}

#[derive(Deserialize)]
struct RecordedPatch {
    name: String,
    files: Vec<RecordedPatchFile>,
}

fn parts_of(value: &Value) -> OverviewParts {
    let patches: Vec<RecordedPatch> =
        serde_json::from_value(value["patches"].clone()).expect("patches");
    OverviewParts {
        pull_request: value
            .get("pr")
            .map(|pr| serde_json::from_value::<PatchMeta>(pr.clone()).expect("pr")),
        patches: patches
            .into_iter()
            .map(|patch| PatchSummary {
                name: patch.name,
                files: patch
                    .files
                    .into_iter()
                    .map(|file| PatchFileSummary {
                        path: file.path,
                        added: file.added,
                        removed: file.removed,
                    })
                    .collect(),
            })
            .collect(),
        repository: value
            .get("repository")
            .map(|repository| repository.as_str().expect("repository").to_string()),
        radius: value.get("radius").map(|radius| match radius {
            Value::String(_) => Radius::NoGraph,
            radius => Radius::Graph(
                serde_json::from_value::<BlastRadius>(radius.clone()).expect("radius"),
            ),
        }),
        building: value
            .get("building")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

#[test]
fn overview_pages_match_typescript() {
    let cases: Vec<OverviewCase> =
        serde_json::from_str(&fixture("overview_cases.json")).expect("overview cases");
    assert!(cases.len() >= 19);
    for case in &cases {
        let actual = overview_page(&case.review, &case.root, &parts_of(&case.parts));
        assert_eq!(actual, case.expected, "case: {}", case.name);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryCase {
    name: String,
    directory: String,
    readme: Option<String>,
    graph_text: Option<String>,
    outlines: Vec<(String, String)>,
    coverage: ReviewCoverage,
    graph: Option<String>,
    expected: String,
}

#[test]
fn repository_overviews_match_typescript() {
    let cases: Vec<RepositoryCase> =
        serde_json::from_str(&fixture("repository_cases.json")).expect("repository cases");
    assert!(cases.len() >= 6);
    let base = tempfile::tempdir().expect("temp dir");
    for case in &cases {
        let root = base.path().join(&case.directory);
        std::fs::create_dir_all(&root).expect("root");
        if let Some(readme) = &case.readme {
            std::fs::write(root.join(readme), "# Readme\n").expect("readme");
        }
        if let Some(graph) = &case.graph_text {
            std::fs::create_dir_all(root.join("graphify-out")).expect("graphify-out");
            std::fs::write(root.join("graphify-out/graph.json"), graph).expect("graph");
        }
        let graph = case.graph.as_deref().map(|state| match state {
            "built" => GraphState::Built,
            "missing" => GraphState::Missing,
            _ => GraphState::Failed,
        });
        let actual = repository_overview(&OverviewInput {
            root: &root,
            coverage: &case.coverage,
            outlines: &case.outlines,
            graph,
        })
        .expect("overview");
        assert_eq!(actual, case.expected, "case: {}", case.name);
    }
}

#[test]
fn unreadable_graph_fails_the_overview() {
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(root.path().join("graphify-out")).expect("graphify-out");
    std::fs::write(root.path().join("graphify-out/graph.json"), "{ not json").expect("graph");
    let coverage = ReviewCoverage {
        total: 0,
        viewed: 0,
        areas: Vec::new(),
    };
    let result = repository_overview(&OverviewInput {
        root: root.path(),
        coverage: &coverage,
        outlines: &[],
        graph: Some(GraphState::Built),
    });
    assert!(result.is_err());
}
