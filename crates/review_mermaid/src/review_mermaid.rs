//! Mermaid diagrams for review pages: parsed and laid out here, drawn by the app (Floem), with the hit areas of the
//! nodes and edges a reviewer comments on.
//!
//! The parsers, layout and drawing are ported from Ferrite's `src/markdown/mermaid/` (MIT, see `NOTICE` and
//! `LICENSE-FERRITE`); its egui drawing records [`Primitive`]s on a [`Painter`] instead (`egui.rs`). Flowcharts
//! (`flowchart`, `graph`) and sequence diagrams are supported; other types come back as [`Error::Unsupported`], and the
//! page shows their source.

// Ported code keeps Ferrite's shape; Co-Review's lint settings are relaxed for it.
#[allow(clippy::unwrap_used, clippy::expect_used, dead_code, clippy::all)]
mod egui;
#[allow(clippy::unwrap_used, clippy::expect_used, dead_code, unused, clippy::all)]
mod flowchart;
#[allow(clippy::unwrap_used, clippy::expect_used, dead_code, unused, clippy::all)]
mod sequence;
#[allow(clippy::unwrap_used, clippy::expect_used, dead_code, unused, clippy::all)]
mod text;
#[allow(clippy::unwrap_used, clippy::expect_used, dead_code, unused, clippy::all)]
mod utils;

use std::panic::{AssertUnwindSafe, catch_unwind};

pub use egui::{Align, Align2, Color32, Hit, HitArea, HitTarget, Painter, Pos2, Primitive, Rect, Stroke, Vec2};
pub use sequence::participant_node_id;
pub use text::{EstimatedTextMeasurer, TextMeasurer, TextSize};

/// A diagram, drawn: what to paint, in the diagram's coordinates from (0, 0), and where its commentable parts are.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub size: Vec2,
    pub primitives: Vec<Primitive>,
    pub hits: Vec<Hit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A diagram type that isn't drawn yet (its keyword).
    Unsupported(String),
    /// The source doesn't parse, or drawing it failed.
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(kind) => write!(formatter, "{kind} diagrams are not drawn yet"),
            Error::Invalid(message) => write!(formatter, "{message}"),
        }
    }
}

/// The diagram's type keyword: its first line that isn't blank or a `%%` comment, first word.
pub fn diagram_type(code: &str) -> String {
    code.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_string()
}

/// Lays out and draws a diagram. `dark`: the dark palette; `available_width`: how wide the page is.
pub fn render(
    code: &str,
    dark: bool,
    font_size: f32,
    available_width: f32,
    text_measurer: &impl TextMeasurer,
) -> Result<Scene, Error> {
    // From the type line on: comments and blank lines before it would be read as the type line by some parsers.
    let start = code
        .lines()
        .position(|line| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with("%%")
        })
        .unwrap_or(0);
    let source = code.lines().skip(start).collect::<Vec<_>>().join("\n");
    let kind = diagram_type(&source);
    let painter = Painter::new();
    let drawn = catch_unwind(AssertUnwindSafe(|| match kind.to_lowercase().as_str() {
        "flowchart" | "graph" => {
            let chart = flowchart::parse_flowchart(&source).map_err(Error::Invalid)?;
            let layout = flowchart::layout_flowchart(&chart, available_width, font_size, text_measurer);
            let colors = if dark {
                flowchart::FlowchartColors::dark()
            } else {
                flowchart::FlowchartColors::light()
            };
            Ok(flowchart::render_flowchart(
                &painter,
                &chart,
                &layout,
                &colors,
                font_size,
                text_measurer,
            ))
        }
        "sequencediagram" => {
            let diagram = sequence::parse_sequence_diagram(&source).map_err(Error::Invalid)?;
            Ok(sequence::render_sequence_diagram(
                &painter,
                &diagram,
                dark,
                font_size,
                text_measurer,
            ))
        }
        _ => Err(Error::Unsupported(kind.clone())),
    }));
    let size = match drawn {
        Ok(result) => result?,
        Err(_) => return Err(Error::Invalid("the diagram could not be drawn".to_string())),
    };
    let (primitives, hits) = painter.finish();
    Ok(Scene { size, primitives, hits })
}

impl Scene {
    /// What is at `point`: a node before an edge, the one drawn last first.
    pub fn hit_test(&self, point: Pos2) -> Option<&HitTarget> {
        let node = self.hits.iter().rev().find(|hit| match &hit.area {
            HitArea::Rect(rect) => rect.contains(point),
            HitArea::Path { .. } => false,
        });
        let edge = || {
            self.hits.iter().rev().find(|hit| match &hit.area {
                HitArea::Path { points, tolerance } => points
                    .windows(2)
                    .any(|segment| distance_to_segment(point, segment[0], segment[1]) <= *tolerance),
                HitArea::Rect(_) => false,
            })
        };
        node.or_else(edge).map(|hit| &hit.target)
    }

    /// The area of a hit target, by node or edge id.
    pub fn area_of(&self, node_id: Option<&str>, edge_id: Option<&str>) -> Option<&HitArea> {
        self.hits
            .iter()
            .find(|hit| match &hit.target {
                HitTarget::Node { id, .. } => Some(id.as_str()) == node_id,
                HitTarget::Edge { id, .. } => Some(id.as_str()) == edge_id,
            })
            .map(|hit| &hit.area)
    }
}

impl Scene {
    /// The scene as SVG (estimated text placement), to look at a diagram outside the app.
    pub fn to_svg(&self) -> String {
        let paint = |color: Color32| {
            format!(
                "rgba({},{},{},{:.3})",
                color.r(),
                color.g(),
                color.b(),
                f32::from(color.a()) / 255.0
            )
        };
        let escape = |text: &str| text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{:.0}\" height=\"{:.0}\" font-family=\"sans-serif\">",
            self.size.x, self.size.y
        );
        for primitive in &self.primitives {
            let element = match primitive {
                Primitive::Line { from, to, stroke } => format!(
                    "<line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    from.x, from.y, to.x, to.y, paint(stroke.color), stroke.width
                ),
                Primitive::Polygon { points, fill, stroke } => format!(
                    "<polygon points=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    points.iter().map(|point| format!("{},{}", point.x, point.y)).collect::<Vec<_>>().join(" "),
                    paint(*fill),
                    paint(stroke.color),
                    stroke.width
                ),
                Primitive::Rect { rect, radius, fill, stroke } => format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    rect.min.x,
                    rect.min.y,
                    rect.width(),
                    rect.height(),
                    radius,
                    paint(*fill),
                    paint(stroke.color),
                    stroke.width
                ),
                Primitive::Circle { center, radius, fill, stroke } => format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    center.x,
                    center.y,
                    radius,
                    paint(*fill),
                    paint(stroke.color),
                    stroke.width
                ),
                Primitive::Text { pos, align, text, size, color, .. } => {
                    let anchor = match align.0[0] {
                        Align::Min => "start",
                        Align::Center => "middle",
                        Align::Max => "end",
                    };
                    let baseline = match align.0[1] {
                        Align::Min => "hanging",
                        Align::Center => "central",
                        Align::Max => "text-after-edge",
                    };
                    format!(
                        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"{anchor}\" dominant-baseline=\"{baseline}\" fill=\"{}\">{}</text>",
                        pos.x,
                        pos.y,
                        size,
                        paint(*color),
                        escape(text)
                    )
                }
            };
            svg.push_str(&element);
        }
        svg.push_str("</svg>");
        svg
    }
}

fn distance_to_segment(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let along = end - start;
    let length = along.x * along.x + along.y * along.y;
    if length <= f32::EPSILON {
        return point.distance(start);
    }
    let t = (((point - start).x * along.x + (point - start).y * along.y) / length).clamp(0.0, 1.0);
    point.distance(start + along * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(code: &str) -> Scene {
        render(code, false, 14.0, 800.0, &EstimatedTextMeasurer::new()).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn a_flowchart_has_its_nodes_and_edges_as_hit_targets() {
        let scene = scene("flowchart TD\n  req[Ingest request] --> check{Valid?}\n  check -->|yes| store[(Store)]\n");
        let nodes: Vec<&str> = scene
            .hits
            .iter()
            .filter_map(|hit| match &hit.target {
                HitTarget::Node { id, .. } => Some(id.as_str()),
                HitTarget::Edge { .. } => None,
            })
            .collect();
        assert_eq!(nodes, ["req", "check", "store"]);
        let edges: Vec<&str> = scene
            .hits
            .iter()
            .filter_map(|hit| match &hit.target {
                HitTarget::Edge { id, .. } => Some(id.as_str()),
                HitTarget::Node { .. } => None,
            })
            .collect();
        assert_eq!(edges, ["req\u{2192}check", "check\u{2192}store"]);
        assert!(scene.size.x > 0.0 && scene.size.y > 0.0);
        // The middle of a node hits it.
        let Some(HitArea::Rect(rect)) = scene.area_of(Some("check"), None) else {
            panic!("no area for check");
        };
        assert_eq!(
            scene.hit_test(rect.center()),
            Some(&HitTarget::Node { id: "check".into(), label: "Valid?".into() })
        );
        // A point on an edge's path hits the edge.
        let Some(HitArea::Path { points, .. }) = scene.area_of(None, Some("req\u{2192}check")) else {
            panic!("no path for the edge");
        };
        let middle = points[0] + (points[1] - points[0]) * 0.5;
        assert!(matches!(scene.hit_test(middle), Some(HitTarget::Edge { index: 0, .. })));
    }

    #[test]
    fn a_sequence_diagram_has_participants_and_messages() {
        let scene = scene(
            "sequenceDiagram\n  participant api as Place order\n  participant db as Database\n  api->>db: order\n  db-->>api: ok\n",
        );
        let targets: Vec<String> = scene
            .hits
            .iter()
            .map(|hit| match &hit.target {
                HitTarget::Node { id, .. } => format!("node {id}"),
                HitTarget::Edge { id, index } => format!("edge {id} {index}"),
            })
            .collect();
        assert_eq!(
            targets,
            ["node place-order", "node database", "edge api\u{2192}db 0", "edge db\u{2192}api 1"]
        );
    }

    /// `REVIEW_MERMAID_SVG=<dir>` writes the test diagrams as SVG there, to look at them.
    #[test]
    fn scenes_draw_as_svg() {
        let flowchart = scene("flowchart TD\n  req[Accept webhook] --> save[(Save delivery)]\n  save --> job{Job runner}\n  job -->|ok| done[Mark delivered]\n  job -->|5xx| retry[Retry with backoff]\n  retry --> job\n");
        let sequence = scene("sequenceDiagram\n  participant api as Place order\n  participant db as Database\n  api->>db: order\n  db-->>api: ok\n");
        for (name, scene) in [("flowchart", &flowchart), ("sequence", &sequence)] {
            let svg = scene.to_svg();
            assert!(svg.starts_with("<svg") && svg.contains("<text"));
            if let Some(dir) = std::env::var_os("REVIEW_MERMAID_SVG") {
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}.svg")), svg).ok();
            }
        }
    }

    #[test]
    fn other_types_are_unsupported_and_bad_sources_invalid() {
        let measurer = EstimatedTextMeasurer::new();
        assert_eq!(
            render("%% c\npie title Pets\n  \"Dogs\" : 3\n", false, 14.0, 800.0, &measurer),
            Err(Error::Unsupported("pie".into()))
        );
        assert!(matches!(
            render("sequenceDiagram\n  loop forever\n  A->>B: x\n", false, 14.0, 800.0, &measurer),
            Err(Error::Invalid(_))
        ));
        assert_eq!(diagram_type("%% id: x\n\nflowchart LR\n"), "flowchart");
    }
}
