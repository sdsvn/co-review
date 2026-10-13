//! Shared drawing utilities for Mermaid diagram rendering.

use crate::egui::{Pos2, Stroke};

/// Draw a dashed line from start to end.
pub fn draw_dashed_line(
    painter: &crate::egui::Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    dash_len: f32,
    gap_len: f32,
) {
    let dir = (end - start).normalized();
    let total_len = (end - start).length();
    let mut pos = 0.0;

    while pos < total_len {
        let seg_start = start + dir * pos;
        let seg_end_pos = (pos + dash_len).min(total_len);
        let seg_end = start + dir * seg_end_pos;
        painter.line_segment([seg_start, seg_end], stroke);
        pos = seg_end_pos + gap_len;
    }
}
