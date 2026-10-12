//! Mermaid diagrams on review pages: laid out by `review_mermaid` (ported from Ferrite) and drawn here with Floem,
//! with comments on the diagram (`mermaid-block`), on a node (`mermaid-node`) and on an edge (`mermaid-edge`).
//! Diagram types that aren't drawn yet show their source with a note.

use std::{any::Any, cell::RefCell, sync::Arc};

use floem::{
    Renderer, View, ViewId,
    context::{EventCx, LayoutCx, PaintCx, UpdateCx},
    event::{Event, EventPropagation},
    kurbo::{BezPath, Circle, Point, Rect, Stroke},
    peniko::Color,
    reactive::{ReadSignal, SignalGet, SignalTrack, SignalWith, create_effect},
    style::Style,
    taffy::tree::NodeId,
    text::{Attrs, AttrsList, FamilyOwned, TextLayout},
    views::{Decorators, empty, h_stack, label, v_stack},
};
use review_markdown::Inline;
use review_mermaid::{Align, Color32, HitArea, HitTarget, Primitive, Scene, TextMeasurer, TextSize};
use review_model::{DocAnchor, DocAnchorType};

use super::{
    design_tree::anchor_base,
    markdown_view::{Action, PageContext, TextKind, comment_icon, text_block},
};
use crate::config::{LapceConfig, color::LapceColor};

/// The diagram at `index` of the page: its header (its block id, and Comment) and the diagram.
pub fn diagram_view(ctx: &PageContext, index: usize, head: &Inline) -> impl View + use<> {
    let config = ctx.config;
    let block_id = head.text.clone();
    let code = ctx.diagrams.get(index).map(|diagram| diagram.code.clone()).unwrap_or_default();
    let header = {
        let on_comment = ctx.on_comment.clone();
        let anchor = DocAnchor {
            anchor_type: DocAnchorType::MermaidBlock,
            block_id: Some(block_id.clone()),
            ..anchor_base()
        };
        h_stack((
            label(|| "diagram").style(move |s| {
                s.margin_right(6.0)
                    .font_size(11.0)
                    .selectable(false)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
            text_block(ctx, head, TextKind::Code),
            empty().style(|s| s.flex_grow(1.0_f32)),
            comment_icon(config, "Comment on this diagram", move || on_comment(anchor.clone(), Action::Comment)),
        ))
        .style(move |s| {
            s.width_full()
                .items_center()
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .border_bottom(1.0)
                .border_color(config.get().color(LapceColor::LAPCE_BORDER))
        })
    };
    let canvas = DiagramCanvas::new(ctx, block_id, code);
    v_stack((header, canvas)).style(move |s| {
        s.width_full()
            .margin_vert(8.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

/// Text measured with the app's fonts, for the layout.
struct FloemMeasurer {
    family: Vec<FamilyOwned>,
}

impl FloemMeasurer {
    fn layout(&self, text: &str, font_size: f32, width: Option<f32>) -> TextLayout {
        let attrs = Attrs::new().family(&self.family).font_size(font_size);
        let mut layout = TextLayout::new();
        if let Some(width) = width {
            layout.set_size(width, f32::MAX);
        }
        layout.set_text(text, AttrsList::new(attrs), None);
        layout
    }
}

impl TextMeasurer for FloemMeasurer {
    fn measure(&self, text: &str, font_size: f32) -> TextSize {
        let size = self.layout(text, font_size, None).size();
        TextSize::new(size.width as f32, size.height as f32)
    }

    fn row_height(&self, font_size: f32) -> f32 {
        self.layout("Ag", font_size, None).size().height as f32
    }

    fn measure_wrapped(&self, text: &str, font_size: f32, max_width: f32) -> TextSize {
        if max_width <= 0.0 {
            return self.measure(text, font_size);
        }
        let size = self.layout(text, font_size, Some(max_width)).size();
        TextSize::new(size.width as f32, size.height as f32)
    }
}

/// What the canvas shows: the drawn diagram, or why it isn't drawn.
enum Drawing {
    Scene(Box<Scene>),
    Source { note: String, layout: TextLayout },
}

/// A laid-out label: its text, size and wrap width.
type CachedText = (String, f32, Option<f32>, TextLayout);

struct DiagramCanvas {
    id: ViewId,
    ctx: PageContext,
    block_id: String,
    code: String,
    drawing: Option<Drawing>,
    /// Laid out for this width and theme.
    drawn_for: Option<(u32, bool)>,
    hovered: Option<HitTarget>,
    /// Where the diagram is drawn in the canvas (centred).
    offset: Point,
    node: Option<NodeId>,
    config: ReadSignal<Arc<LapceConfig>>,
    texts: RefCell<Vec<CachedText>>,
}

impl DiagramCanvas {
    fn new(ctx: &PageContext, block_id: String, code: String) -> Self {
        let id = ViewId::new();
        let config = ctx.config;
        // A theme change draws the diagram again; comments change its marks.
        create_effect(move |_| {
            config.with(|_| ());
            id.update_state(());
        });
        let marked = ctx.marked_parts;
        create_effect(move |_| {
            marked.track();
            id.request_paint();
        });
        DiagramCanvas {
            id,
            ctx: ctx.clone(),
            block_id,
            code,
            drawing: None,
            drawn_for: None,
            hovered: None,
            offset: Point::ZERO,
            node: None,
            config,
            texts: RefCell::new(Vec::new()),
        }
    }

    fn dark(config: &LapceConfig) -> bool {
        let background = config.color(LapceColor::EDITOR_BACKGROUND).to_rgba8();
        let luminance = 0.2126 * f32::from(background.r) + 0.7152 * f32::from(background.g) + 0.0722 * f32::from(background.b);
        luminance < 128.0
    }

    /// Lays the diagram out for `width`, unless it already is.
    fn draw_for(&mut self, width: f32) {
        let config = self.config.get_untracked();
        let dark = Self::dark(&config);
        let key = (width.max(200.0).round() as u32, dark);
        if self.drawn_for == Some(key) {
            return;
        }
        self.drawn_for = Some(key);
        let measurer = FloemMeasurer {
            family: FamilyOwned::parse_list(&config.ui.font_family).collect(),
        };
        let font_size = config.ui.font_size() as f32;
        self.texts.borrow_mut().clear();
        self.drawing = Some(match review_mermaid::render(&self.code, dark, font_size, width.max(200.0), &measurer) {
            Ok(scene) => {
                tracing::debug!(
                    "diagram {} drawn at {width}px: {:.0}x{:.0}, {} parts to comment on",
                    self.block_id,
                    scene.size.x,
                    scene.size.y,
                    scene.hits.len()
                );
                Drawing::Scene(Box::new(scene))
            }
            Err(error) => {
                tracing::debug!("diagram {} shown as source: {error}", self.block_id);
                let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&config.editor.font_family).collect();
                let attrs = Attrs::new()
                    .family(&family)
                    .font_size(config.editor.font_size() as f32)
                    .color(config.color(LapceColor::EDITOR_FOREGROUND));
                let mut layout = TextLayout::new();
                layout.set_text(&self.code, AttrsList::new(attrs), None);
                Drawing::Source {
                    note: match error {
                        review_mermaid::Error::Unsupported(kind) => {
                            format!("{kind} diagrams aren't drawn yet; this is the diagram's source.")
                        }
                        review_mermaid::Error::Invalid(message) => format!("The diagram could not be drawn: {message}"),
                    },
                    layout,
                }
            }
        });
    }

    fn size(&self) -> (f64, f64) {
        match &self.drawing {
            Some(Drawing::Scene(scene)) => (f64::from(scene.size.x), f64::from(scene.size.y)),
            Some(Drawing::Source { layout, .. }) => {
                let size = layout.size();
                (size.width + 16.0, size.height + 40.0)
            }
            None => (0.0, 40.0),
        }
    }

    fn text_layout(&self, text: &str, size: f32, color: Color, max_width: Option<f32>) -> TextLayout {
        let mut texts = self.texts.borrow_mut();
        if let Some((_, _, _, layout)) = texts
            .iter()
            .find(|(cached, cached_size, width, _)| cached == text && *cached_size == size && *width == max_width)
        {
            return layout.clone();
        }
        let config = self.config.get_untracked();
        let family: Vec<FamilyOwned> = FamilyOwned::parse_list(&config.ui.font_family).collect();
        let attrs = Attrs::new().family(&family).font_size(size).color(color);
        let mut layout = TextLayout::new();
        if let Some(width) = max_width {
            layout.set_size(width, f32::MAX);
        }
        layout.set_text(text, AttrsList::new(attrs), max_width.map(|_| floem::text::Align::Center));
        texts.push((text.to_string(), size, max_width, layout.clone()));
        layout
    }

    fn target_at(&self, point: Point) -> Option<HitTarget> {
        let Some(Drawing::Scene(scene)) = &self.drawing else {
            return None;
        };
        let local = review_mermaid::Pos2::new((point.x - self.offset.x) as f32, (point.y - self.offset.y) as f32);
        scene.hit_test(local).cloned()
    }

    fn comment_on(&self, target: &HitTarget) {
        let anchor = match target {
            HitTarget::Node { id, label } => DocAnchor {
                anchor_type: DocAnchorType::MermaidNode,
                block_id: Some(self.block_id.clone()),
                node_id: Some(id.clone()),
                source: Some(if self.code.trim_start().starts_with("sequenceDiagram") {
                    label.clone()
                } else {
                    format!("{id}[{label}]")
                }),
                ..anchor_base()
            },
            HitTarget::Edge { id, .. } => DocAnchor {
                anchor_type: DocAnchorType::MermaidEdge,
                block_id: Some(self.block_id.clone()),
                edge_id: Some(id.clone()),
                ..anchor_base()
            },
        };
        (self.ctx.on_comment)(anchor, Action::Comment);
    }
}

fn color(color: Color32) -> Color {
    Color::from_rgba8(color.r(), color.g(), color.b(), color.a())
}

impl View for DiagramCanvas {
    fn id(&self) -> ViewId {
        self.id
    }

    fn view_style(&self) -> Option<Style> {
        Some(Style::new().width_full().min_width(0.0))
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        format!("Mermaid diagram {}", self.block_id).into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, _state: Box<dyn Any>) {
        self.drawn_for = None;
        self.id.request_layout();
    }

    fn layout(&mut self, cx: &mut LayoutCx) -> NodeId {
        cx.layout_node(self.id(), true, |_cx| {
            // Laid out at the width it had last time (the page's width), at least once at a default width.
            let width = self.id.get_layout().map_or(800.0, |layout| layout.size.width).max(200.0);
            self.draw_for(width - 16.0);
            let (_, height) = self.size();
            let node = *self.node.get_or_insert_with(|| self.id.new_taffy_node());
            let style = Style::new().width_full().height((height + 16.0) as f32).to_taffy_style();
            self.id.set_taffy_style(node, style);
            vec![node]
        })
    }

    fn compute_layout(&mut self, _cx: &mut floem::context::ComputeLayoutCx) -> Option<Rect> {
        let width = self.id.get_layout().map_or(0.0, |layout| layout.size.width);
        let key = (width - 16.0).max(200.0).round() as u32;
        if width > 0.0 && self.drawn_for.map(|(drawn, _)| drawn) != Some(key) {
            self.draw_for(width - 16.0);
            self.id.request_layout();
        }
        let (diagram_width, _) = self.size();
        self.offset = Point::new(((f64::from(width) - diagram_width) / 2.0).max(8.0), 8.0);
        None
    }

    fn event_before_children(&mut self, _cx: &mut EventCx, event: &Event) -> EventPropagation {
        match event {
            Event::PointerMove(pointer) => {
                let hovered = self.target_at(pointer.pos);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    self.id.request_style();
                    self.id.request_paint();
                }
                EventPropagation::Continue
            }
            Event::PointerLeave => {
                if self.hovered.take().is_some() {
                    self.id.request_paint();
                }
                EventPropagation::Continue
            }
            Event::PointerUp(pointer) if pointer.button.is_primary() => match self.target_at(pointer.pos) {
                Some(target) => {
                    self.comment_on(&target);
                    EventPropagation::Stop
                }
                None => EventPropagation::Continue,
            },
            _ => EventPropagation::Continue,
        }
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let config = self.config.get_untracked();
        match &self.drawing {
            Some(Drawing::Scene(scene)) => {
                cx.save();
                cx.offset((self.offset.x, self.offset.y));
                for primitive in &scene.primitives {
                    self.paint_primitive(cx, primitive);
                }
                // Parts with comments, and the part under the pointer.
                let marks = self.ctx.marked_parts.with_untracked(|parts| {
                    parts
                        .iter()
                        .filter(|(block, _)| *block == self.block_id)
                        .map(|(_, part)| part.clone())
                        .collect::<Vec<_>>()
                });
                let warn = config.color(LapceColor::LAPCE_WARN);
                for hit in &scene.hits {
                    let id = match &hit.target {
                        HitTarget::Node { id, .. } | HitTarget::Edge { id, .. } => id,
                    };
                    let hovered = self.hovered.as_ref() == Some(&hit.target);
                    if marks.contains(id) {
                        paint_area(cx, &hit.area, warn, 2.5);
                    }
                    if hovered {
                        paint_area(cx, &hit.area, config.color(LapceColor::EDITOR_LINK), 2.0);
                    }
                }
                cx.restore();
            }
            Some(Drawing::Source { note, layout }) => {
                let note_layout = self.text_layout(note, 12.0, config.color(LapceColor::EDITOR_DIM), None);
                cx.draw_text(&note_layout, Point::new(8.0, 8.0));
                cx.draw_text(layout, Point::new(8.0, 32.0));
            }
            None => {}
        }
    }
}

impl DiagramCanvas {
    fn paint_primitive(&self, cx: &mut PaintCx, primitive: &Primitive) {
        let point = |pos: review_mermaid::Pos2| Point::new(f64::from(pos.x), f64::from(pos.y));
        match primitive {
            Primitive::Line { from, to, stroke } => {
                if !stroke.is_empty() {
                    let line = floem::kurbo::Line::new(point(*from), point(*to));
                    cx.stroke(&line, color(stroke.color), &Stroke::new(f64::from(stroke.width)));
                }
            }
            Primitive::Polygon { points, fill, stroke } => {
                let mut path = BezPath::new();
                for (index, pos) in points.iter().enumerate() {
                    if index == 0 {
                        path.move_to(point(*pos));
                    } else {
                        path.line_to(point(*pos));
                    }
                }
                path.close_path();
                if fill.a() > 0 {
                    cx.fill(&path, color(*fill), 0.0);
                }
                if !stroke.is_empty() {
                    cx.stroke(&path, color(stroke.color), &Stroke::new(f64::from(stroke.width)));
                }
            }
            Primitive::Rect { rect, radius, fill, stroke } => {
                let shape = Rect::new(
                    f64::from(rect.min.x),
                    f64::from(rect.min.y),
                    f64::from(rect.max.x),
                    f64::from(rect.max.y),
                )
                .to_rounded_rect(f64::from(*radius));
                if fill.a() > 0 {
                    cx.fill(&shape, color(*fill), 0.0);
                }
                if !stroke.is_empty() {
                    cx.stroke(&shape, color(stroke.color), &Stroke::new(f64::from(stroke.width)));
                }
            }
            Primitive::Circle { center, radius, fill, stroke } => {
                let circle = Circle::new(point(*center), f64::from(*radius));
                if fill.a() > 0 {
                    cx.fill(&circle, color(*fill), 0.0);
                }
                if !stroke.is_empty() {
                    cx.stroke(&circle, color(stroke.color), &Stroke::new(f64::from(stroke.width)));
                }
            }
            Primitive::Text { pos, align, text, size, color: text_color, max_width } => {
                let layout = self.text_layout(text, *size, color(*text_color), *max_width);
                let measured = layout.size();
                let width = max_width.map_or(measured.width, f64::from);
                let x = match align.0[0] {
                    Align::Min => f64::from(pos.x),
                    Align::Center => f64::from(pos.x) - width / 2.0,
                    Align::Max => f64::from(pos.x) - width,
                };
                let y = match align.0[1] {
                    Align::Min => f64::from(pos.y),
                    Align::Center => f64::from(pos.y) - measured.height / 2.0,
                    Align::Max => f64::from(pos.y) - measured.height,
                };
                cx.draw_text(&layout, Point::new(x, y));
            }
        }
    }
}

fn paint_area(cx: &mut PaintCx, area: &HitArea, color: Color, width: f64) {
    match area {
        HitArea::Rect(rect) => {
            let shape = Rect::new(
                f64::from(rect.min.x) - 2.0,
                f64::from(rect.min.y) - 2.0,
                f64::from(rect.max.x) + 2.0,
                f64::from(rect.max.y) + 2.0,
            )
            .to_rounded_rect(6.0);
            cx.stroke(&shape, color, &Stroke::new(width));
        }
        HitArea::Path { points, .. } => {
            let mut path = BezPath::new();
            for (index, pos) in points.iter().enumerate() {
                let point = Point::new(f64::from(pos.x), f64::from(pos.y));
                if index == 0 {
                    path.move_to(point);
                } else {
                    path.line_to(point);
                }
            }
            cx.stroke(&path, color.multiply_alpha(0.7), &Stroke::new(width + 2.0));
        }
    }
}
