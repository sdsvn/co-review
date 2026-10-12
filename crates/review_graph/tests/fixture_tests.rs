//! Fixture tests: replay the TypeScript blastRadius and overview functions over the committed
//! graph.json fixture and assert equality with the expected outputs produced by tests/gen/gen.mjs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use review_graph::{blast_radius, graph_overview, GraphifyGraph};
use review_model::ReviewCoverage;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load_graph() -> GraphifyGraph {
    let text = std::fs::read_to_string(fixtures_dir().join("graph.json"))
        .expect("graph.json fixture must exist");
    serde_json::from_str(&text).expect("graph.json must parse")
}

#[test]
fn blast_radius_matches_typescript() {
    let graph = load_graph();

    // Same changed lines as gen.mjs: line 447 and line 10 in review_model/src/review_model.rs
    let mut changed_lines = BTreeMap::new();
    changed_lines.insert("review_model/src/review_model.rs".to_string(), vec![447, 10]);

    let result = blast_radius(&graph, &changed_lines).expect("should produce blast radius");

    let expected_text = std::fs::read_to_string(fixtures_dir().join("blast_radius_expected.json"))
        .expect("blast_radius_expected.json must exist");
    let expected: serde_json::Value =
        serde_json::from_str(&expected_text).expect("expected JSON must parse");
    let actual: serde_json::Value =
        serde_json::to_value(&result).expect("blast radius should serialize");

    // Compare changed node ids.
    let expected_changed: Vec<&str> = expected["changed"]
        .as_array()
        .expect("changed array")
        .iter()
        .map(|n| n["id"].as_str().expect("id"))
        .collect();
    let actual_changed: Vec<&str> = actual["changed"]
        .as_array()
        .expect("changed array")
        .iter()
        .map(|n| n["id"].as_str().expect("id"))
        .collect();

    // Use sets since BTreeMap ordering may differ from TS Map insertion order.
    let expected_set: std::collections::HashSet<&str> = expected_changed.iter().copied().collect();
    let actual_set: std::collections::HashSet<&str> = actual_changed.iter().copied().collect();
    assert_eq!(
        expected_set, actual_set,
        "changed node ids must match TypeScript output"
    );

    // Compare affected counts.
    let expected_affected = expected["affected"].as_array().expect("affected array").len();
    let actual_affected = actual["affected"].as_array().expect("affected array").len();
    assert_eq!(
        expected_affected, actual_affected,
        "affected count must match TypeScript output"
    );

    // Compare file lists (as sets).
    let expected_files: std::collections::HashSet<&str> = expected["files"]
        .as_array()
        .expect("files array")
        .iter()
        .map(|f| f.as_str().expect("file path"))
        .collect();
    let actual_files: std::collections::HashSet<&str> = actual["files"]
        .as_array()
        .expect("files array")
        .iter()
        .map(|f| f.as_str().expect("file path"))
        .collect();
    assert_eq!(
        expected_files, actual_files,
        "affected files must match TypeScript output"
    );
}

#[test]
fn overview_matches_typescript() {
    let graph = load_graph();
    let coverage = ReviewCoverage {
        total: 5,
        viewed: 2,
        areas: vec![],
    };

    let actual = graph_overview(&graph, &coverage, "test-repo");
    let expected = std::fs::read_to_string(fixtures_dir().join("overview_expected.txt"))
        .expect("overview_expected.txt must exist");

    pretty_assertions::assert_eq!(expected, actual);
}

#[test]
fn overview_with_areas_matches_typescript() {
    let graph = load_graph();
    let coverage = ReviewCoverage {
        total: 12,
        viewed: 5,
        areas: vec![
            review_model::ReviewCoverageArea {
                path: "src/api".to_string(),
                total: 4,
                viewed: 2,
                next: Some("src/api/handler.rs".to_string()),
            },
            review_model::ReviewCoverageArea {
                path: "src/db".to_string(),
                total: 3,
                viewed: 1,
                next: None,
            },
        ],
    };

    let actual = graph_overview(&graph, &coverage, "test-repo");
    let expected =
        std::fs::read_to_string(fixtures_dir().join("overview_with_areas_expected.txt"))
            .expect("overview_with_areas_expected.txt must exist");

    pretty_assertions::assert_eq!(expected, actual);
}

#[test]
fn edge_case_overview_matches_typescript() {
    let text = std::fs::read_to_string(fixtures_dir().join("edge_cases_graph.json"))
        .expect("edge_cases_graph.json must exist");
    let graph: GraphifyGraph =
        serde_json::from_str(&text).expect("edge_cases_graph.json must parse");
    let coverage = ReviewCoverage {
        total: 15,
        viewed: 1,
        areas: vec![],
    };

    let actual = graph_overview(&graph, &coverage, "test-repo");
    let expected =
        std::fs::read_to_string(fixtures_dir().join("edge_cases_overview_expected.txt"))
            .expect("edge_cases_overview_expected.txt must exist");

    pretty_assertions::assert_eq!(expected, actual);
}

#[test]
fn dependencies_set_matches_typescript() {
    let expected_text = std::fs::read_to_string(fixtures_dir().join("dependencies.json"))
        .expect("dependencies.json must exist");
    let expected: Vec<String> =
        serde_json::from_str(&expected_text).expect("dependencies JSON must parse");

    let mut actual: Vec<&str> = review_graph::DEPENDENCIES.to_vec();
    actual.sort();

    assert_eq!(
        expected,
        actual.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "DEPENDENCIES set must match TypeScript"
    );
}
