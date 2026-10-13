//! Mermaid diagrams on review pages: laid out by `review_mermaid` (ported from Ferrite) and drawn here with Floem,
//! with comments on the diagram (`mermaid-block`), on a node (`mermaid-node`) and on an edge (`mermaid-edge`).
//! Diagram types that aren't drawn yet show their source with a note.
//!
//! A click on a diagram away from its nodes and edges, or its full-size button, opens it in a window-sized view
//! ([`ZoomStage`]): the wheel, a trackpad pinch, the buttons or + / - / 0 zoom toward the pointer, dragging pans,
//! double-click zooms in (shift: out), Esc closes. Clicking a node or edge there starts a comment on it, as on the page.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use floem::{
    Renderer, View, ViewId,
    action::{add_overlay, remove_overlay},
    context::{EventCx, LayoutCx, PaintCx, UpdateCx},
    event::{Event, EventPropagation},
    keyboard::{Key, NamedKey},
    kurbo::{BezPath, Circle, Point, Rect, Shape, Size, Stroke},
    peniko::Color,
    reactive::{Memo, ReadSignal, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect, create_rw_signal},
    style::{CursorStyle, Style},
    taffy::tree::NodeId,
    text::{Attrs, AttrsList, FamilyOwned, TextLayout},
    views::{Decorators, empty, h_stack, label, svg, v_stack},
};
use review_markdown::Inline;
use review_mermaid::{Align, Color32, HitArea, HitTarget, Pos2, Primitive, Scene, TextMeasurer, TextSize};
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
    // The full-size view, while it is open.
    let zoomed: Rc<Cell<Option<ViewId>>> = Rc::new(Cell::new(None));
    let canvas = DiagramCanvas::new(ctx, block_id.clone(), code, zoomed.clone());
    let canvas_id = canvas.id;
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
            icon_button(config, SCREEN_FULL, "Open full size: zoom and pan", move || {
                canvas_id.update_state(OpenFullSize)
            }),
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
    v_stack((header, canvas))
        .on_cleanup(move || {
            if let Some(overlay) = zoomed.take() {
                remove_overlay(overlay);
            }
        })
        .style(move |s| {
            s.width_full()
                .margin_vert(8.0)
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.get().color(LapceColor::LAPCE_BORDER))
        })
}

const SCREEN_FULL: &str = include_str!("../../../icons/codicons/screen-full.svg");
const CLOSE: &str = include_str!("../../../icons/codicons/close.svg");

/// An icon that does something, with a tooltip (the diagram header's and the full-size view's buttons).
fn icon_button(
    config: ReadSignal<Arc<LapceConfig>>,
    icon: &'static str,
    tooltip: &'static str,
    on_click: impl Fn() + 'static,
) -> impl View {
    crate::app::tooltip_label(
        config,
        svg(icon).style(move |s| {
            let config = config.get();
            let size = config.ui.icon_size() as f32;
            s.size(size, size).color(config.color(LapceColor::LAPCE_ICON_ACTIVE))
        }),
        move || tooltip,
    )
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.margin_left(6.0)
            .padding(2.0)
            .border_radius(4.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
    })
}

/// Asks a diagram's canvas to open its full-size view.
struct OpenFullSize;

/// The anchor of a comment on a diagram's node or edge.
fn part_anchor(block_id: &str, code: &str, target: &HitTarget) -> DocAnchor {
    match target {
        HitTarget::Node { id, label } => DocAnchor {
            anchor_type: DocAnchorType::MermaidNode,
            block_id: Some(block_id.to_string()),
            node_id: Some(id.clone()),
            source: Some(if code.trim_start().starts_with("sequenceDiagram") {
                label.clone()
            } else {
                format!("{id}[{label}]")
            }),
            ..anchor_base()
        },
        HitTarget::Edge { id, .. } => DocAnchor {
            anchor_type: DocAnchorType::MermaidEdge,
            block_id: Some(block_id.to_string()),
            edge_id: Some(id.clone()),
            ..anchor_base()
        },
    }
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

/// Where a scene is drawn: the diagram's point `p` is at `origin + p * scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    origin: Point,
    scale: f64,
}

impl Placement {
    fn point(&self, pos: Pos2) -> Point {
        Point::new(
            self.origin.x + f64::from(pos.x) * self.scale,
            self.origin.y + f64::from(pos.y) * self.scale,
        )
    }

    fn to_diagram(self, point: Point) -> Pos2 {
        Pos2::new(
            ((point.x - self.origin.x) / self.scale) as f32,
            ((point.y - self.origin.y) / self.scale) as f32,
        )
    }
}

/// Paints a scene where a [`Placement`] puts it, its labels laid out at the size they are shown.
struct ScenePainter {
    config: ReadSignal<Arc<LapceConfig>>,
    texts: RefCell<Vec<CachedText>>,
    /// The scale the cached labels are laid out for.
    texts_scale: Cell<f64>,
}

impl ScenePainter {
    fn new(config: ReadSignal<Arc<LapceConfig>>) -> Self {
        ScenePainter {
            config,
            texts: RefCell::new(Vec::new()),
            texts_scale: Cell::new(1.0),
        }
    }

    fn clear(&self) {
        self.texts.borrow_mut().clear();
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

    /// The scene, the parts of it with comments (`marks`) and the part under the pointer.
    fn paint(&self, cx: &mut PaintCx, scene: &Scene, at: Placement, marks: &[String], hovered: Option<&HitTarget>) {
        if self.texts_scale.get() != at.scale {
            self.texts_scale.set(at.scale);
            self.clear();
        }
        for primitive in &scene.primitives {
            self.paint_primitive(cx, primitive, at);
        }
        let config = self.config.get_untracked();
        let warn = config.color(LapceColor::LAPCE_WARN);
        for hit in &scene.hits {
            let id = match &hit.target {
                HitTarget::Node { id, .. } | HitTarget::Edge { id, .. } => id,
            };
            if marks.contains(id) {
                paint_area(cx, &hit.area, at, warn, 2.5);
            }
            if hovered == Some(&hit.target) {
                paint_area(cx, &hit.area, at, config.color(LapceColor::EDITOR_LINK), 2.0);
            }
        }
    }

    fn paint_primitive(&self, cx: &mut PaintCx, primitive: &Primitive, at: Placement) {
        let point = |pos: Pos2| at.point(pos);
        match primitive {
            Primitive::Line { from, to, stroke } => {
                let line = floem::kurbo::Line::new(point(*from), point(*to));
                fill_and_stroke(cx, &line, Color32::TRANSPARENT, *stroke, at.scale);
            }
            Primitive::Polygon { points, fill, stroke } => {
                let mut path = polyline(points, at);
                path.close_path();
                fill_and_stroke(cx, &path, *fill, *stroke, at.scale);
            }
            Primitive::Rect { rect, radius, fill, stroke } => {
                let shape = Rect::from_points(point(rect.min), point(rect.max))
                    .to_rounded_rect(f64::from(*radius) * at.scale);
                fill_and_stroke(cx, &shape, *fill, *stroke, at.scale);
            }
            Primitive::Circle { center, radius, fill, stroke } => {
                let circle = Circle::new(point(*center), f64::from(*radius) * at.scale);
                fill_and_stroke(cx, &circle, *fill, *stroke, at.scale);
            }
            Primitive::Text { pos, align, text, size, color: text_color, max_width } => {
                let scale = at.scale as f32;
                let max_width = max_width.map(|width| width * scale);
                let layout = self.text_layout(text, *size * scale, color(*text_color), max_width);
                let measured = layout.size();
                let width = max_width.map_or(measured.width, f64::from);
                let pos = point(*pos);
                let x = match align.0[0] {
                    Align::Min => pos.x,
                    Align::Center => pos.x - width / 2.0,
                    Align::Max => pos.x - width,
                };
                let y = match align.0[1] {
                    Align::Min => pos.y,
                    Align::Center => pos.y - measured.height / 2.0,
                    Align::Max => pos.y - measured.height,
                };
                cx.draw_text(&layout, Point::new(x, y));
            }
        }
    }
}

/// The diagram parts of `block_id` with comments.
fn marked(parts: Memo<Vec<(String, String)>>, block_id: &str) -> Vec<String> {
    parts.with_untracked(|parts| {
        parts
            .iter()
            .filter(|(block, _)| block == block_id)
            .map(|(_, part)| part.clone())
            .collect()
    })
}

struct DiagramCanvas {
    id: ViewId,
    ctx: PageContext,
    block_id: String,
    code: String,
    drawing: Option<Drawing>,
    /// Laid out for this width and theme.
    drawn_for: Option<(u32, bool)>,
    hovered: Option<HitTarget>,
    /// Where the primary button went down on the diagram (a click opens it full size).
    pressed: Option<Point>,
    /// Where the diagram is drawn in the canvas (centred).
    offset: Point,
    node: Option<NodeId>,
    config: ReadSignal<Arc<LapceConfig>>,
    painter: ScenePainter,
    /// The full-size view (an overlay), while it is open.
    zoomed: Rc<Cell<Option<ViewId>>>,
}

impl DiagramCanvas {
    fn new(ctx: &PageContext, block_id: String, code: String, zoomed: Rc<Cell<Option<ViewId>>>) -> Self {
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
            pressed: None,
            offset: Point::ZERO,
            node: None,
            config,
            painter: ScenePainter::new(config),
            zoomed,
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
        self.painter.clear();
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

    fn target_at(&self, point: Point) -> Option<HitTarget> {
        let Some(Drawing::Scene(scene)) = &self.drawing else {
            return None;
        };
        let at = Placement {
            origin: self.offset,
            scale: 1.0,
        };
        scene.hit_test(at.to_diagram(point)).cloned()
    }

    fn comment_on(&self, target: &HitTarget) {
        (self.ctx.on_comment)(part_anchor(&self.block_id, &self.code, target), Action::Comment);
    }

    /// Opens the diagram in a window-sized view, to zoom and pan.
    fn open_full_size(&self) {
        let Some(Drawing::Scene(scene)) = &self.drawing else {
            tracing::debug!("diagram {} is not drawn: no full-size view", self.block_id);
            return;
        };
        if let Some(open) = self.zoomed.take() {
            remove_overlay(open);
        }
        let zoomed = self.zoomed.clone();
        let close: Rc<dyn Fn()> = {
            let (zoomed, block_id) = (zoomed.clone(), self.block_id.clone());
            Rc::new(move || {
                if let Some(overlay) = zoomed.take() {
                    tracing::info!("diagram {block_id}: full-size view closed");
                    remove_overlay(overlay);
                }
            })
        };
        let on_part: Rc<dyn Fn(&HitTarget)> = {
            let (on_comment, block_id, code) = (self.ctx.on_comment.clone(), self.block_id.clone(), self.code.clone());
            Rc::new(move |target: &HitTarget| on_comment(part_anchor(&block_id, &code, target), Action::Comment))
        };
        let scene = Rc::new(scene.as_ref().clone());
        let (config, window_size, marked_parts, block_id) =
            (self.config, self.ctx.window_size, self.ctx.marked_parts, self.block_id.clone());
        tracing::info!(
            "diagram {block_id}: opened full size ({:.0}x{:.0})",
            scene.size.x,
            scene.size.y
        );
        let overlay = add_overlay(Point::ZERO, move |_| {
            full_size_view(config, window_size, scene, block_id, marked_parts, on_part, close)
        });
        zoomed.set(Some(overlay));
    }
}

fn color(color: Color32) -> Color {
    Color::from_rgba8(color.r(), color.g(), color.b(), color.a())
}

fn fill_and_stroke(cx: &mut PaintCx, shape: &impl Shape, fill: Color32, stroke: review_mermaid::Stroke, scale: f64) {
    if fill.a() > 0 {
        cx.fill(shape, color(fill), 0.0);
    }
    if !stroke.is_empty() {
        cx.stroke(shape, color(stroke.color), &Stroke::new(f64::from(stroke.width) * scale));
    }
}

/// The diagram points as a path where `at` places them.
fn polyline(points: &[Pos2], at: Placement) -> BezPath {
    let mut path = BezPath::new();
    for (index, pos) in points.iter().enumerate() {
        if index == 0 {
            path.move_to(at.point(*pos));
        } else {
            path.line_to(at.point(*pos));
        }
    }
    path
}

impl View for DiagramCanvas {
    fn id(&self) -> ViewId {
        self.id
    }

    fn view_style(&self) -> Option<Style> {
        // A node or an edge takes a comment; anywhere else opens the diagram full size.
        Some(Style::new().width_full().min_width(0.0).cursor(CursorStyle::Pointer))
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        format!("Mermaid diagram {}", self.block_id).into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, state: Box<dyn Any>) {
        if state.is::<OpenFullSize>() {
            self.open_full_size();
            return;
        }
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
            Event::PointerDown(pointer) if pointer.button.is_primary() => {
                self.pressed = Some(pointer.pos);
                EventPropagation::Continue
            }
            Event::PointerUp(pointer) if pointer.button.is_primary() => {
                let pressed = self.pressed.take();
                match self.target_at(pointer.pos) {
                    Some(target) => {
                        self.comment_on(&target);
                        EventPropagation::Stop
                    }
                    // A click away from the nodes and edges (not the end of a selection or a drag).
                    None if pressed.is_some_and(|down| down.distance(pointer.pos) <= 4.0)
                        && matches!(self.drawing, Some(Drawing::Scene(_))) =>
                    {
                        self.open_full_size();
                        EventPropagation::Stop
                    }
                    None => EventPropagation::Continue,
                }
            }
            _ => EventPropagation::Continue,
        }
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let config = self.config.get_untracked();
        match &self.drawing {
            Some(Drawing::Scene(scene)) => {
                let at = Placement {
                    origin: self.offset,
                    scale: 1.0,
                };
                let marks = marked(self.ctx.marked_parts, &self.block_id);
                self.painter.paint(cx, scene, at, &marks, self.hovered.as_ref());
            }
            Some(Drawing::Source { note, layout }) => {
                let note_layout = self.painter.text_layout(note, 12.0, config.color(LapceColor::EDITOR_DIM), None);
                cx.draw_text(&note_layout, Point::new(8.0, 8.0));
                cx.draw_text(layout, Point::new(8.0, 32.0));
            }
            None => {}
        }
    }
}

fn paint_area(cx: &mut PaintCx, area: &HitArea, at: Placement, color: Color, width: f64) {
    match area {
        HitArea::Rect(rect) => {
            let shape = Rect::from_points(at.point(rect.min), at.point(rect.max))
                .inflate(2.0, 2.0)
                .to_rounded_rect(6.0);
            cx.stroke(&shape, color, &Stroke::new(width));
        }
        HitArea::Path { points, .. } => {
            cx.stroke(&polyline(points, at), color.multiply_alpha(0.7), &Stroke::new(width + 2.0));
        }
    }
}

// ---- The full-size view: zoom and pan ----

const MIN_SCALE: f64 = 0.1;
const MAX_SCALE: f64 = 12.0;
/// Room around the diagram when it is fitted to the window.
const FIT_MARGIN: f64 = 48.0;
/// How far the pointer moves before a press is a drag (not a click).
const DRAG_SLOP: f64 = 4.0;

/// Where the full-size view shows the diagram: scaled by `scale`, its top-left at `origin` in the stage.
#[derive(Clone, Copy, Debug)]
struct Zoom {
    scale: f64,
    origin: Point,
    stage: Size,
    diagram: Size,
}

impl Zoom {
    fn placement(&self) -> Placement {
        Placement {
            origin: self.origin,
            scale: self.scale,
        }
    }

    /// The whole diagram, centred in the stage.
    fn fit(&mut self) {
        if self.stage.is_zero_area() || self.diagram.is_zero_area() {
            return;
        }
        let scale = ((self.stage.width - FIT_MARGIN) / self.diagram.width)
            .min((self.stage.height - FIT_MARGIN) / self.diagram.height);
        self.scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        self.origin = Point::new(
            (self.stage.width - self.diagram.width * self.scale) / 2.0,
            (self.stage.height - self.diagram.height * self.scale) / 2.0,
        );
    }

    /// Zooms by `factor`, keeping the stage point `at` where it is (the stage's centre by default).
    fn zoom(&mut self, factor: f64, at: Option<Point>) {
        let at = at.unwrap_or_else(|| Point::new(self.stage.width / 2.0, self.stage.height / 2.0));
        let next = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let ratio = next / self.scale;
        self.origin = Point::new(at.x - (at.x - self.origin.x) * ratio, at.y - (at.y - self.origin.y) * ratio);
        self.scale = next;
    }
}

/// What the toolbar's buttons ask of the stage.
#[derive(Clone, Copy, Debug)]
enum ZoomCommand {
    By(f64),
    Fit,
}

/// The window-sized view of a diagram: a bar (its block id, the zoom level, zoom out / in, fit, close) over the stage.
fn full_size_view(
    config: ReadSignal<Arc<LapceConfig>>,
    window_size: Memo<Size>,
    scene: Rc<Scene>,
    block_id: String,
    marked_parts: Memo<Vec<(String, String)>>,
    on_part: Rc<dyn Fn(&HitTarget)>,
    close: Rc<dyn Fn()>,
) -> impl View + use<> {
    let level = create_rw_signal(1.0_f64);
    let stage = ZoomStage::new(config, scene, block_id.clone(), marked_parts, level, on_part, close.clone());
    let stage_id = stage.id;
    let text_button = move |text: &'static str, tooltip: &'static str, command: ZoomCommand| {
        crate::app::tooltip_label(config, label(move || text), move || tooltip)
            .on_click_stop(move |_| stage_id.update_state(command))
            .style(move |s| {
                let config = config.get();
                s.margin_left(6.0)
                    .padding_horiz(6.0)
                    .padding_vert(1.0)
                    .border_radius(4.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
            })
    };
    let bar = h_stack((
        label(|| "diagram").style(move |s| {
            s.margin_right(6.0)
                .font_size(11.0)
                .selectable(false)
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        label(move || block_id.clone()).style(move |s| {
            let config = config.get();
            s.font_family(config.editor.font_family.clone()).selectable(false)
        }),
        empty().style(|s| s.flex_grow(1.0_f32)),
        label(move || format!("{:.0}%", level.get() * 100.0)).style(move |s| {
            s.min_width(44.0)
                .justify_end()
                .selectable(false)
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        text_button("\u{2212}", "Zoom out (\u{2212})", ZoomCommand::By(0.8)),
        text_button("+", "Zoom in (+)", ZoomCommand::By(1.25)),
        text_button("Fit", "Fit to window (0)", ZoomCommand::Fit),
        icon_button(config, CLOSE, "Close (Esc)", move || close()),
    ))
    .style(move |s| {
        s.width_full()
            .items_center()
            .padding_horiz(10.0)
            .padding_vert(4.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    });
    v_stack((bar, stage)).style(move |s| {
        let size = window_size.get();
        let config = config.get();
        s.width(size.width as f32)
            .height(size.height as f32)
            .flex_col()
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
            .color(config.color(LapceColor::EDITOR_FOREGROUND))
            .font_size(config.ui.font_size() as f32)
    })
}

/// A press on the stage: where it started, where the diagram was, and whether it has become a drag.
#[derive(Clone, Copy, Debug)]
struct Press {
    at: Point,
    origin: Point,
    dragging: bool,
}

/// The diagram in the full-size view, zoomed and panned.
struct ZoomStage {
    id: ViewId,
    scene: Rc<Scene>,
    block_id: String,
    marked_parts: Memo<Vec<(String, String)>>,
    painter: ScenePainter,
    zoom: Zoom,
    /// The zoom level, for the bar.
    level: RwSignal<f64>,
    press: Option<Press>,
    hovered: Option<HitTarget>,
    pointer: Option<Point>,
    /// Starts a comment on a node or an edge.
    on_part: Rc<dyn Fn(&HitTarget)>,
    close: Rc<dyn Fn()>,
}

impl ZoomStage {
    fn new(
        config: ReadSignal<Arc<LapceConfig>>,
        scene: Rc<Scene>,
        block_id: String,
        marked_parts: Memo<Vec<(String, String)>>,
        level: RwSignal<f64>,
        on_part: Rc<dyn Fn(&HitTarget)>,
        close: Rc<dyn Fn()>,
    ) -> Self {
        let id = ViewId::new();
        create_effect(move |_| {
            marked_parts.track();
            id.request_paint();
        });
        // Keys reach the view with the focus.
        id.request_focus();
        let diagram = Size::new(f64::from(scene.size.x), f64::from(scene.size.y));
        ZoomStage {
            id,
            scene,
            block_id,
            marked_parts,
            painter: ScenePainter::new(config),
            zoom: Zoom {
                scale: 1.0,
                origin: Point::ZERO,
                stage: Size::ZERO,
                diagram,
            },
            level,
            press: None,
            hovered: None,
            pointer: None,
            on_part,
            close,
        }
    }

    fn target_at(&self, point: Point) -> Option<HitTarget> {
        self.scene.hit_test(self.zoom.placement().to_diagram(point)).cloned()
    }

    /// Shows the zoom as it now is.
    fn zoomed(&mut self, why: &str) {
        tracing::debug!(
            "diagram {}: {why}, at {:.0}% (origin {:.0}, {:.0})",
            self.block_id,
            self.zoom.scale * 100.0,
            self.zoom.origin.x,
            self.zoom.origin.y
        );
        self.level.set(self.zoom.scale);
        self.id.request_paint();
    }

    fn zoom_by(&mut self, factor: f64, at: Option<Point>, why: &str) {
        self.zoom.zoom(factor, at);
        self.zoomed(why);
    }

    fn fit(&mut self) {
        self.zoom.fit();
        self.zoomed("fitted to the window");
    }
}

impl View for ZoomStage {
    fn id(&self) -> ViewId {
        self.id
    }

    fn view_style(&self) -> Option<Style> {
        let cursor = match (&self.press, &self.hovered) {
            (Some(Press { dragging: true, .. }), _) => CursorStyle::Grabbing,
            (_, Some(_)) => CursorStyle::Pointer,
            _ => CursorStyle::Grab,
        };
        Some(
            Style::new()
                .width_full()
                .flex_grow(1.0_f32)
                .flex_basis(0.0_f32)
                .min_height(0.0_f32)
                .cursor(cursor),
        )
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        format!("Mermaid diagram {} (full size)", self.block_id).into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, state: Box<dyn Any>) {
        if let Ok(command) = state.downcast::<ZoomCommand>() {
            match *command {
                ZoomCommand::By(factor) => self.zoom_by(factor, None, "zoomed with the bar"),
                ZoomCommand::Fit => self.fit(),
            }
        }
    }

    fn compute_layout(&mut self, _cx: &mut floem::context::ComputeLayoutCx) -> Option<Rect> {
        let size = self
            .id
            .get_layout()
            .map_or(Size::ZERO, |layout| Size::new(f64::from(layout.size.width), f64::from(layout.size.height)));
        // Fitted when it opens and when the window is resized.
        if size != self.zoom.stage && !size.is_zero_area() {
            self.zoom.stage = size;
            self.fit();
        }
        None
    }

    fn event_before_children(&mut self, cx: &mut EventCx, event: &Event) -> EventPropagation {
        match event {
            Event::PointerDown(pointer) if pointer.button.is_primary() => {
                self.id.request_focus();
                if pointer.count == 2 && self.target_at(pointer.pos).is_none() {
                    let (factor, why) = if pointer.modifiers.shift() {
                        (0.5, "double-click zoomed out")
                    } else {
                        (2.0, "double-click zoomed in")
                    };
                    self.press = None;
                    self.zoom_by(factor, Some(pointer.pos), why);
                    return EventPropagation::Stop;
                }
                self.press = Some(Press {
                    at: pointer.pos,
                    origin: self.zoom.origin,
                    dragging: false,
                });
                cx.update_active(self.id);
                EventPropagation::Stop
            }
            Event::PointerMove(pointer) => {
                self.pointer = Some(pointer.pos);
                if let Some(press) = self.press.as_mut() {
                    if !press.dragging && press.at.distance(pointer.pos) > DRAG_SLOP {
                        press.dragging = true;
                        self.id.request_style();
                    }
                    if press.dragging {
                        self.zoom.origin = press.origin + (pointer.pos - press.at);
                        self.id.request_paint();
                        return EventPropagation::Stop;
                    }
                }
                let hovered = self.target_at(pointer.pos);
                if hovered != self.hovered {
                    self.hovered = hovered;
                    self.id.request_style();
                    self.id.request_paint();
                }
                EventPropagation::Stop
            }
            Event::PointerUp(pointer) if pointer.button.is_primary() => {
                match self.press.take() {
                    Some(Press { dragging: true, .. }) => {
                        self.id.request_style();
                        self.zoomed("panned");
                    }
                    Some(_) => {
                        if let Some(target) = self.target_at(pointer.pos) {
                            tracing::info!("diagram {}: comment on {target:?} from the full-size view", self.block_id);
                            (self.on_part)(&target);
                            (self.close)();
                        }
                    }
                    None => {}
                }
                EventPropagation::Stop
            }
            Event::PointerWheel(wheel) => {
                // Trackpad pinches arrive as ctrl+wheel with small deltas (as in a browser); mouse wheels with large ones.
                let rate = if wheel.modifiers.control() { 0.01 } else { 0.0015 };
                self.pointer = Some(wheel.pos);
                self.zoom_by((-wheel.delta.y * rate).exp(), Some(wheel.pos), "wheel zoomed");
                EventPropagation::Stop
            }
            Event::PinchGesture(pinch) => {
                let factor = (1.0 + pinch.delta).max(0.1);
                self.zoom_by(factor, self.pointer, "pinch zoomed");
                EventPropagation::Stop
            }
            Event::KeyDown(key) => {
                let modifiers = key.modifiers;
                if modifiers.meta() || modifiers.control() || modifiers.alt() {
                    return EventPropagation::Continue;
                }
                match &key.key.logical_key {
                    Key::Named(NamedKey::Escape) => (self.close)(),
                    Key::Character(text) => match text.as_str() {
                        "+" | "=" => self.zoom_by(1.25, self.pointer, "zoomed in with +"),
                        "-" => self.zoom_by(0.8, self.pointer, "zoomed out with -"),
                        "0" => self.fit(),
                        _ => {}
                    },
                    _ => {}
                }
                // The view is modal: keys don't reach the page behind it.
                EventPropagation::Stop
            }
            _ => EventPropagation::Continue,
        }
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let stage = self.zoom.stage.to_rect();
        cx.save();
        cx.clip(&stage);
        let marks = marked(self.marked_parts, &self.block_id);
        self.painter.paint(cx, &self.scene, self.zoom.placement(), &marks, self.hovered.as_ref());
        cx.restore();
    }
}

impl Drop for ZoomStage {
    fn drop(&mut self) {
        tracing::debug!("diagram {}: full-size view dropped", self.block_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom() -> Zoom {
        Zoom {
            scale: 1.0,
            origin: Point::ZERO,
            stage: Size::new(1000.0, 600.0),
            diagram: Size::new(200.0, 100.0),
        }
    }

    #[test]
    fn fitting_centres_the_whole_diagram() {
        let mut zoom = zoom();
        zoom.fit();
        // (1000 - 48) / 200 = 4.76 is the tighter fit (the height allows 5.52).
        assert!((zoom.scale - 4.76).abs() < 1e-9);
        assert!((zoom.origin.x - 24.0).abs() < 1e-9);
        assert!((zoom.origin.y - (600.0 - 476.0) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer_in_place() {
        let mut zoom = zoom();
        let pointer = Point::new(300.0, 200.0);
        let before = zoom.placement().to_diagram(pointer);
        zoom.zoom(2.0, Some(pointer));
        assert_eq!(zoom.scale, 2.0);
        let after = zoom.placement().point(before);
        assert!(after.distance(pointer) < 1e-6, "{after:?}");
    }

    #[test]
    fn zoom_is_kept_between_its_limits() {
        let mut zoom = zoom();
        zoom.zoom(1000.0, None);
        assert_eq!(zoom.scale, MAX_SCALE);
        zoom.zoom(0.000_1, None);
        assert_eq!(zoom.scale, MIN_SCALE);
    }
}
