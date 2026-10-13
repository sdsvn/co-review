//! Shared utility functions for flowchart rendering.
//!
//! Contains geometry helpers, drawing primitives, and lookup functions
//! used across the render module.

use crate::egui::{Pos2, Rect, Stroke, Vec2};

use super::types::{FlowSubgraph, Flowchart};

/// Draw an arrow head at the end of a line segment.
pub(crate) fn draw_arrow_head(
    painter: &crate::egui::Painter,
    from: Pos2,
    to: Pos2,
    head_type: &super::types::ArrowHead,
    color: crate::egui::Color32,
    stroke_width: f32,
) {
    use super::types::ArrowHead;

    let dir = (to - from).normalized();
    let perp = Vec2::new(-dir.y, dir.x);
    let arrow_size = 8.0 + stroke_width;

    match head_type {
        ArrowHead::Arrow => {
            let tip = to;
            let left = to - dir * arrow_size + perp * (arrow_size * 0.5);
            let right = to - dir * arrow_size - perp * (arrow_size * 0.5);
            painter.add(crate::egui::Shape::convex_polygon(
                vec![tip, left, right],
                color,
                Stroke::NONE,
            ));
        }
        ArrowHead::Circle => {
            painter.circle_filled(to - dir * 4.0, 4.0, color);
        }
        ArrowHead::Cross => {
            let center = to - dir * 4.0;
            let size = 4.0;
            painter.line_segment(
                [
                    center - perp * size - dir * size,
                    center + perp * size + dir * size,
                ],
                Stroke::new(stroke_width, color),
            );
            painter.line_segment(
                [
                    center + perp * size - dir * size,
                    center - perp * size + dir * size,
                ],
                Stroke::new(stroke_width, color),
            );
        }
        ArrowHead::None => {}
    }
}

/// The innermost subgraph a node is in (subgraphs are listed children first).
pub(crate) fn find_node_subgraph<'a>(node_id: &str, flowchart: &'a Flowchart) -> Option<&'a FlowSubgraph> {
    flowchart.subgraphs.iter().find(|subgraph| subgraph.node_ids.iter().any(|id| id == node_id))
}

/// Calculate intersection point of a line segment with a rectangle's border.
/// Returns the intersection point closest to `from` on the way to `to`.
pub(crate) fn line_rect_intersection(from: Pos2, to: Pos2, rect: Rect) -> Option<Pos2> {
    let dir = to - from;
    if dir.length_sq() < 0.001 {
        return None;
    }
    // Each border: where it is on one axis, and its extent on the other.
    let borders = [
        (rect.left(), from.x, dir.x, from.y, dir.y, rect.top(), rect.bottom()),
        (rect.right(), from.x, dir.x, from.y, dir.y, rect.top(), rect.bottom()),
        (rect.top(), from.y, dir.y, from.x, dir.x, rect.left(), rect.right()),
        (rect.bottom(), from.y, dir.y, from.x, dir.x, rect.left(), rect.right()),
    ];
    let mut best_t: Option<f32> = None;
    for (at, start, along, cross_start, cross_along, low, high) in borders {
        if along.abs() > 0.001 {
            let t = (at - start) / along;
            let cross = cross_start + t * cross_along;
            if t > 0.0 && t < 1.0 && cross >= low && cross <= high && best_t.is_none_or(|best| t < best) {
                best_t = Some(t);
            }
        }
    }
    best_t.map(|t| from + dir * t)
}

/// Padding around node rects when testing edge–obstacle collisions.
pub(crate) const NODE_OBSTACLE_PADDING: f32 = 8.0;

/// Margin outside the graph bounding box for back-edge loop routing.
pub(crate) const BACK_EDGE_LOOP_MARGIN: f32 = 24.0;

/// Extra horizontal spacing between parallel back-edge loops on the same side.
pub(crate) const BACK_EDGE_LANE_SPACING: f32 = 36.0;

/// Returns true if the open segment `(from, to)` crosses `rect` interior (not merely touching at endpoints).
pub(crate) fn segment_intersects_rect(from: Pos2, to: Pos2, rect: Rect) -> bool {
    let inner = rect.shrink(1.0);
    let Some(hit) = line_rect_intersection(from, to, inner) else {
        return false;
    };
    // Ignore grazing hits at endpoints (adjacent node padding overlap).
    let dir = to - from;
    if dir.length_sq() < 0.001 {
        return false;
    }
    let t = ((hit.x - from.x) * dir.x + (hit.y - from.y) * dir.y) / dir.length_sq();
    t > 0.02 && t < 0.98
}

/// Returns true if any segment in `path` hits an obstacle rect.
pub(crate) fn path_intersects_any(path: &[(Pos2, Pos2)], obstacles: &[Rect]) -> bool {
    path.iter().any(|&(a, b)| {
        obstacles
            .iter()
            .any(|&rect| segment_intersects_rect(a, b, rect))
    })
}

/// Bounding box enclosing all rects, or zero if empty.
pub(crate) fn union_rect_bounds(rects: &[Rect]) -> Rect {
    rects.iter().copied().reduce(Rect::union).unwrap_or(Rect::NOTHING)
}

/// Build padded obstacle rects for every node except the edge endpoints.
pub(crate) fn collect_node_obstacles(
    nodes: &std::collections::HashMap<String, super::types::NodeLayout>,
    offset: Vec2,
    exclude_from: &str,
    exclude_to: &str,
) -> Vec<Rect> {
    nodes
        .iter()
        .filter(|(id, _)| id.as_str() != exclude_from && id.as_str() != exclude_to)
        .map(|(_, layout)| Rect::from_min_size(layout.pos + offset, layout.size).expand(NODE_OBSTACLE_PADDING))
        .collect()
}

/// Content bounding size from positioned nodes and subgraphs.
pub(crate) fn layout_content_size(layout: &super::types::FlowchartLayout, margin: f32) -> Vec2 {
    let mut max_x = 0.0_f32;
    let mut max_y = 0.0_f32;
    for node in layout.nodes.values() {
        max_x = max_x.max(node.pos.x + node.size.x);
        max_y = max_y.max(node.pos.y + node.size.y);
    }
    for sg in layout.subgraphs.values() {
        max_x = max_x.max(sg.pos.x + sg.size.x);
        max_y = max_y.max(sg.pos.y + sg.size.y);
    }
    Vec2::new(max_x + margin, max_y + margin)
}

#[cfg(test)]
mod obstacle_tests {
    use super::*;

    #[test]
    fn segment_through_rect_center_is_intersection() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        assert!(segment_intersects_rect(
            Pos2::new(0.0, 20.0),
            Pos2::new(40.0, 20.0),
            rect
        ));
    }

    #[test]
    fn segment_bypassing_rect_is_clear() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(20.0, 20.0));
        assert!(!segment_intersects_rect(
            Pos2::new(0.0, 0.0),
            Pos2::new(40.0, 0.0),
            rect
        ));
    }

    #[test]
    fn orthogonal_detour_avoids_obstacle() {
        let obstacle = Rect::from_min_size(Pos2::new(40.0, 40.0), Vec2::new(30.0, 30.0)).expand(NODE_OBSTACLE_PADDING);
        let start = Pos2::new(55.0, 20.0);
        let end = Pos2::new(55.0, 90.0);
        // Route via left corridor — never crosses the obstacle column
        let path = [
            (start, Pos2::new(10.0, start.y)),
            (Pos2::new(10.0, start.y), Pos2::new(10.0, end.y)),
            (Pos2::new(10.0, end.y), end),
        ];
        assert!(!path_intersects_any(&path, &[obstacle]));
    }
}
