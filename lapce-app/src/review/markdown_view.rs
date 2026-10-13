//! Rendered Markdown with text selection, for review pages: the blocks of a `review_markdown::Page` as Floem views.
//!
//! Started from Lapce's hover renderer (`markdown.rs`: pulldown-cmark into text layouts); here every piece of text is
//! a [`TextBlock`] that knows where its text is in the page's text (`Inline::start`), so a selection, across blocks
//! too, is a range of the page's text and becomes a `text` anchor exactly as the Theia app makes it. Comment marks,
//! links, a design's badges and reasons are drawn on the same text layouts.

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    sync::Arc,
};

use floem::{
    IntoView, Renderer, View, ViewId,
    context::{ComputeLayoutCx, EventCx, LayoutCx, PaintCx, UpdateCx},
    event::{Event, EventListener, EventPropagation},
    kurbo::{Point, Rect},
    peniko::Color,
    reactive::{Memo, ReadSignal, RwSignal, SignalGet, SignalTrack, SignalUpdate, SignalWith, create_effect},
    style::{CursorStyle, Style},
    taffy::tree::NodeId,
    text::{Attrs, AttrsList, Cursor, FamilyOwned, LineHeightValue, Style as FontStyle, TextLayout, Weight},
    views::{
        Decorators, container, dyn_container, empty, h_stack, h_stack_from_iter, img, label, svg,
        v_stack_from_iter,
    },
};
use review_markdown::{Alignment, Badge, Block, Image, Inline, Meta, Page, StepNode, Style as SpanStyle};
use review_model::{DocAnchor, ThreadIntent};

use crate::{
    config::{LapceConfig, color::LapceColor},
    markdown::highlight_as_code,
};

/// Where a comment is placed: after the block (paragraph, heading, code, table, diagram, step) whose text starts at
/// this offset of the page's text; [`TOP`] above the page.
pub type LeafKey = usize;

/// Comments on the whole document and those whose place is gone.
pub const TOP: LeafKey = usize::MAX;

/// A highlighted range of the page's text: a comment's quote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub thread_id: String,
}

/// What the views of one page share: the selection, the comment marks, and what the page does when the reviewer acts.
#[derive(Clone)]
pub struct PageContext {
    pub config: ReadSignal<Arc<LapceConfig>>,
    /// The selection as `(anchor, focus)` offsets of the page's text (UTF-16).
    pub selection: RwSignal<Option<(usize, usize)>>,
    /// Where the selection's actions show, relative to the page's content.
    pub actions_at: RwSignal<Option<Point>>,
    /// The page's content view, for placing the selection's actions.
    pub content: Rc<RefCell<Option<ViewId>>>,
    pub marks: Memo<Vec<Mark>>,
    /// Steps with comments (`tree-node` ids), and diagram parts with comments (`block id`, `node or edge id`).
    pub marked_steps: Memo<Vec<String>>,
    pub marked_parts: Memo<Vec<(String, String)>>,
    /// The design tree's level (L1, L2, L3).
    pub level: RwSignal<u32>,
    pub texts: Rc<RefCell<Vec<Registered>>>,
    /// A link was clicked.
    pub on_link: Rc<dyn Fn(String)>,
    /// A comment's mark was clicked.
    pub on_mark: Rc<dyn Fn(String)>,
    /// Start a comment, a question or a suggested edit on an anchor.
    pub on_comment: Rc<dyn Fn(DocAnchor, Action)>,
    /// The comments placed after a block.
    pub slot: Rc<dyn Fn(LeafKey) -> floem::AnyView>,
    /// Images by their source, read when the page was.
    pub images: Rc<HashMap<String, Vec<u8>>>,
    /// The page's Mermaid blocks.
    pub diagrams: Rc<Vec<review_markdown::Diagram>>,
    /// The window's size in layout units, for a diagram's full-size view.
    pub window_size: Memo<floem::kurbo::Size>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Comment,
    Ask,
    ProposeEdit,
}

impl Action {
    pub fn intent(self) -> ThreadIntent {
        match self {
            Action::Ask => ThreadIntent::Question,
            Action::Comment | Action::ProposeEdit => ThreadIntent::Comment,
        }
    }
}

/// A text block on the page, for selections that cross blocks.
pub struct Registered {
    id: ViewId,
    start: usize,
    text: Rc<str>,
    layout: Weak<RefCell<TextLayout>>,
}

/// How a text block looks, besides its spans' styles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TextKind {
    Body,
    Heading(u8),
    Code,
    Muted,
    Strong,
}

/// The page's blocks.
pub fn blocks_view(ctx: &PageContext, blocks: &[Block]) -> impl View + use<> {
    let views: Vec<floem::AnyView> = blocks.iter().map(|block| block_view(ctx, block)).collect();
    v_stack_from_iter(views).style(|s| s.width_full().flex_col())
}

fn block_view(ctx: &PageContext, block: &Block) -> floem::AnyView {
    let config = ctx.config;
    match block {
        Block::Meta(meta) => meta_view(ctx, meta).into_any(),
        Block::Warnings { title, items } => {
            let mut rows: Vec<floem::AnyView> = vec![text_block(ctx, title, TextKind::Strong).into_any()];
            for item in items {
                rows.push(
                    h_stack((label(|| "\u{2022}").style(|s| s.margin_right(6.0)), text_block(ctx, item, TextKind::Body)))
                        .style(|s| s.width_full().items_start())
                        .into_any(),
                );
            }
            v_stack_from_iter(rows)
                .style(move |s| {
                    let config = config.get();
                    s.width_full()
                        .padding(10.0)
                        .margin_bottom(12.0)
                        .border(1.0)
                        .border_radius(6.0)
                        .border_color(config.color(LapceColor::LAPCE_WARN))
                        .background(config.color(LapceColor::LAPCE_WARN).multiply_alpha(0.08))
                })
                .into_any()
        }
        Block::Heading { level, text } => with_slot(
            ctx,
            text.start,
            text_block(ctx, text, TextKind::Heading(*level)).style({
                let level = *level;
                move |s| s.margin_top(if level <= 2 { 16.0 } else { 10.0 }).margin_bottom(6.0)
            }),
        ),
        Block::Paragraph(inline) => {
            let images: Vec<floem::AnyView> = inline.images.iter().map(|image| image_view(ctx, image)).collect();
            let text = (!inline.text.trim().is_empty()).then(|| text_block(ctx, inline, TextKind::Body).into_any());
            let mut parts: Vec<floem::AnyView> = text.into_iter().collect();
            parts.extend(images);
            with_slot(ctx, inline.start, v_stack_from_iter(parts).style(|s| s.width_full().margin_vert(4.0)))
        }
        Block::Code { language, text } => {
            let code = text_block_code(ctx, text, language.clone());
            with_slot(
                ctx,
                text.start,
                container(code).style(move |s| {
                    let config = config.get();
                    s.width_full()
                        .padding(8.0)
                        .margin_vert(6.0)
                        .border_radius(4.0)
                        .background(config.color(LapceColor::PANEL_BACKGROUND))
                }),
            )
        }
        Block::Quote(children) => h_stack((
            empty().style(move |s| {
                s.width(3.0)
                    .height_full()
                    .margin_right(10.0)
                    .background(config.get().color(LapceColor::MARKDOWN_BLOCKQUOTE))
            }),
            blocks_view(ctx, children).style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
        ))
        .style(|s| s.width_full().margin_vert(4.0))
        .into_any(),
        Block::List { start, items, .. } => {
            let rows: Vec<floem::AnyView> = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let marker = match start {
                        Some(first) => format!("{}.", first + index as u64),
                        None => "\u{2022}".to_string(),
                    };
                    h_stack((
                        label(move || marker.clone()).style(move |s| {
                            s.min_width(22.0)
                                .padding_right(6.0)
                                .margin_top(4.0)
                                .justify_end()
                                .color(config.get().color(LapceColor::EDITOR_DIM))
                        }),
                        blocks_view(ctx, item).style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
                    ))
                    .style(|s| s.width_full().items_start())
                    .into_any()
                })
                .collect();
            v_stack_from_iter(rows).style(|s| s.width_full().margin_vert(2.0)).into_any()
        }
        Block::Table { alignments, head, rows } => {
            let start = head.first().map_or(TOP, |cell| cell.start);
            let mut lines: Vec<floem::AnyView> = vec![table_row(ctx, head, alignments, true)];
            lines.extend(rows.iter().map(|row| table_row(ctx, row, alignments, false)));
            with_slot(
                ctx,
                start,
                v_stack_from_iter(lines).style(move |s| {
                    s.width_full()
                        .margin_vert(6.0)
                        .border(1.0)
                        .border_color(config.get().color(LapceColor::LAPCE_BORDER))
                }),
            )
        }
        Block::Rule => empty()
            .style(move |s| {
                s.width_full()
                    .height(1.0)
                    .margin_vert(10.0)
                    .background(config.get().color(LapceColor::LAPCE_BORDER))
            })
            .into_any(),
        Block::Diagram { index, head } => {
            let diagram = super::mermaid::diagram_view(ctx, *index, head);
            with_slot(ctx, head.start, diagram)
        }
        Block::Section { heading, altitude, blocks } => {
            let title = text_block(ctx, heading, TextKind::Heading(2));
            let switch = altitude.then(|| level_switch(ctx).into_any());
            let mut row: Vec<floem::AnyView> = vec![title.into_any()];
            row.extend(switch);
            let mut parts: Vec<floem::AnyView> = vec![
                h_stack_from_iter(row)
                    .style(|s| s.width_full().items_center().margin_top(16.0).margin_bottom(6.0))
                    .into_any(),
            ];
            parts.push(blocks_view(ctx, blocks).into_any());
            v_stack_from_iter(parts).style(|s| s.width_full()).into_any()
        }
        Block::Tree { design, steps } => super::design_tree::tree_view(ctx, steps, *design).into_any(),
    }
}

/// A block followed by the comments placed after it.
pub fn with_slot(ctx: &PageContext, key: LeafKey, view: impl IntoView + 'static) -> floem::AnyView {
    v_stack_from_iter([view.into_any(), (ctx.slot)(key)])
        .style(|s| s.width_full())
        .into_any()
}

/// L1 · L2 · L3: how deep the design tree is unfolded.
fn level_switch(ctx: &PageContext) -> impl View + use<> {
    let (level, config) = (ctx.level, ctx.config);
    let button = move |depth: u32| {
        label(move || format!("L{depth}"))
            .on_click_stop(move |_| level.set(depth))
            .style(move |s| {
                let config = config.get();
                let active = level.get() == depth;
                s.padding_horiz(6.0)
                    .padding_vert(1.0)
                    .margin_left(4.0)
                    .font_size(11.0)
                    .border(1.0)
                    .border_radius(4.0)
                    .border_color(config.color(LapceColor::LAPCE_BORDER))
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .apply_if(active, |s| {
                        s.background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
                            .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
                    })
            })
    };
    h_stack((button(1), button(2), button(3))).style(|s| s.margin_left(10.0))
}

fn meta_view(ctx: &PageContext, meta: &Meta) -> impl View + use<> {
    let config = ctx.config;
    let mut parts: Vec<floem::AnyView> = vec![
        container(text_block(ctx, &meta.kind, TextKind::Strong))
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(6.0)
                    .padding_vert(1.0)
                    .margin_right(8.0)
                    .border_radius(4.0)
                    .background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND).multiply_alpha(0.25))
            })
            .into_any(),
    ];
    if let Some(description) = &meta.description {
        parts.push(
            container(text_block(ctx, description, TextKind::Muted))
                .style(|s| s.margin_right(8.0).flex_grow(1.0_f32).min_width(0.0))
                .into_any(),
        );
    }
    if let Some(resource) = &meta.resource {
        parts.push(text_block(ctx, resource, TextKind::Code).into_any());
    }
    h_stack_from_iter(parts).style(move |s| {
        s.width_full()
            .items_center()
            .padding_bottom(8.0)
            .margin_bottom(8.0)
            .border_bottom(1.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
    })
}

fn table_row(ctx: &PageContext, cells: &[Inline], alignments: &[Alignment], head: bool) -> floem::AnyView {
    let config = ctx.config;
    let views: Vec<floem::AnyView> = cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let alignment = alignments.get(index).copied().unwrap_or(Alignment::None);
            container(text_block(ctx, cell, if head { TextKind::Strong } else { TextKind::Body }))
                .style(move |s| {
                    let s = s
                        .flex_grow(1.0_f32)
                        .flex_basis(0.0_f32)
                        .min_width(0.0)
                        .padding_horiz(8.0)
                        .padding_vert(4.0)
                        .border_right(1.0)
                        .border_color(config.get().color(LapceColor::LAPCE_BORDER));
                    match alignment {
                        Alignment::Right => s.justify_end(),
                        Alignment::Center => s.justify_center(),
                        Alignment::None | Alignment::Left => s,
                    }
                })
                .into_any()
        })
        .collect();
    h_stack_from_iter(views)
        .style(move |s| {
            let config = config.get();
            s.width_full()
                .border_bottom(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .apply_if(head, |s| s.background(config.color(LapceColor::PANEL_BACKGROUND)))
        })
        .into_any()
}

fn image_view(ctx: &PageContext, image: &Image) -> floem::AnyView {
    let config = ctx.config;
    match ctx.images.get(&image.url) {
        Some(bytes) if image.url.to_lowercase().ends_with(".svg") => {
            let svg_text = String::from_utf8_lossy(bytes).into_owned();
            svg(move || svg_text.clone()).style(|s| s.max_width_full().height(200.0)).into_any()
        }
        Some(bytes) => {
            let bytes = bytes.clone();
            img(move || bytes.clone()).style(|s| s.max_width_full()).into_any()
        }
        None => {
            let alt = format!("[{}] ({})", image.alt, image.url);
            label(move || alt.clone())
                .style(move |s| s.color(config.get().color(LapceColor::EDITOR_DIM)))
                .into_any()
        }
    }
}

// -----------------------------------------------------------------------------------------------------------------
// Selectable text
// -----------------------------------------------------------------------------------------------------------------

/// A block of page text: drawn with its styles, the selection and the comment marks, and selectable.
pub fn text_block(ctx: &PageContext, inline: &Inline, kind: TextKind) -> TextBlock {
    let inline = Rc::new(inline.clone());
    let config = ctx.config;
    let build = {
        let inline = inline.clone();
        move || layout_of(&inline, kind, None, &config.get())
    };
    TextBlock::new(ctx, inline, build)
}

/// A code block's text, highlighted as its language.
fn text_block_code(ctx: &PageContext, inline: &Inline, language: String) -> TextBlock {
    let inline = Rc::new(inline.clone());
    let config = ctx.config;
    let build = {
        let inline = inline.clone();
        move || layout_of(&inline, TextKind::Code, Some(&language), &config.get())
    };
    TextBlock::new(ctx, inline, build)
}

/// The text layout of an inline: its spans' styles on the kind's base style.
fn layout_of(inline: &Inline, kind: TextKind, language: Option<&str>, config: &LapceConfig) -> TextLayout {
    let ui_family: Vec<FamilyOwned> = FamilyOwned::parse_list(&config.ui.font_family).collect();
    let code_family: Vec<FamilyOwned> = FamilyOwned::parse_list(&config.editor.font_family).collect();
    let base_size = config.ui.font_size() as f32 + 1.0;
    let foreground = config.color(LapceColor::EDITOR_FOREGROUND);
    let mut base = Attrs::new()
        .color(foreground)
        .family(&ui_family)
        .font_size(base_size)
        .line_height(LineHeightValue::Normal(1.6));
    match kind {
        TextKind::Body => {}
        TextKind::Heading(level) => {
            let scale = match level {
                1 => 1.8,
                2 => 1.4,
                3 => 1.17,
                _ => 1.0,
            };
            base = base.font_size(base_size * scale).weight(Weight::BOLD).line_height(LineHeightValue::Normal(1.3));
        }
        TextKind::Code => {
            base = base
                .family(&code_family)
                .font_size(config.editor.font_size() as f32)
                .line_height(LineHeightValue::Normal(1.5));
        }
        TextKind::Muted => base = base.color(config.color(LapceColor::EDITOR_DIM)),
        TextKind::Strong => base = base.weight(Weight::BOLD),
    }
    let mut attrs = AttrsList::new(base.clone());
    for span in &inline.spans {
        attrs.add_span(span.range.clone(), span_attrs(base.clone(), &span.style, &code_family, config));
    }
    if let Some(language) = language.filter(|language| !language.is_empty()) {
        highlight_as_code(
            &mut attrs,
            base.clone(),
            lapce_core::language::LapceLanguage::from_name(language),
            &inline.text,
            0,
            config,
        );
    }
    let mut layout = TextLayout::new();
    layout.set_text(&inline.text, attrs, None);
    layout
}

fn span_attrs<'a>(base: Attrs<'a>, style: &SpanStyle, code_family: &'a [FamilyOwned], config: &LapceConfig) -> Attrs<'a> {
    let mut attrs = base;
    if style.code {
        attrs = attrs.family(code_family);
    }
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(FontStyle::Italic);
    }
    if style.reason {
        attrs = attrs.color(config.color(LapceColor::EDITOR_DIM));
    }
    if style.strike {
        attrs = attrs.color(config.color(LapceColor::EDITOR_DIM));
    }
    if style.link.is_some() {
        attrs = attrs.color(config.color(LapceColor::EDITOR_LINK));
    }
    if let Some(badge) = style.badge {
        attrs = attrs.weight(Weight::BOLD).color(badge_color(badge, config));
    }
    attrs
}

pub fn badge_color(badge: Badge, config: &LapceConfig) -> Color {
    match badge {
        Badge::Question => config.color(LapceColor::EDITOR_LINK),
        Badge::Risk => config.color(LapceColor::LAPCE_WARN),
        Badge::Name => config.color(LapceColor::SOURCE_CONTROL_ADDED),
    }
}

pub struct TextBlock {
    id: ViewId,
    ctx: PageContext,
    inline: Rc<Inline>,
    /// The layout as drawn (wrapped to the width available), shared with the page for selections across blocks.
    layout: Rc<RefCell<TextLayout>>,
    /// The layout without wrapping.
    natural: TextLayout,
    available_width: Option<f32>,
    text_node: Option<NodeId>,
    /// Pressed at this point (and page offset); a drag selects.
    pressed: Option<(Point, usize)>,
    dragging: bool,
}

impl TextBlock {
    fn new(ctx: &PageContext, inline: Rc<Inline>, build: impl Fn() -> TextLayout + 'static) -> Self {
        let id = ViewId::new();
        let natural = build();
        create_effect(move |first: Option<()>| {
            let layout = build();
            if first.is_some() {
                id.update_state(layout);
            }
        });
        // Marks and the selection repaint the block.
        {
            let (selection, marks) = (ctx.selection, ctx.marks);
            create_effect(move |_| {
                selection.track();
                marks.track();
                id.request_paint();
            });
        }
        let layout = Rc::new(RefCell::new(natural.clone()));
        ctx.texts.borrow_mut().push(Registered {
            id,
            start: inline.start,
            text: Rc::from(inline.text.as_str()),
            layout: Rc::downgrade(&layout),
        });
        TextBlock {
            id,
            ctx: ctx.clone(),
            inline,
            layout,
            natural,
            available_width: None,
            text_node: None,
            pressed: None,
            dragging: false,
        }
    }

    fn start(&self) -> usize {
        self.inline.start
    }

    /// The page offset at a point of this block.
    fn offset_at(&self, point: Point) -> usize {
        offset_in(&self.layout.borrow(), &self.inline.text, self.start(), point)
    }

    /// The page offset at a window point: in the text block under it, or the nearest one.
    fn offset_at_window(&self, window: Point) -> Option<usize> {
        let texts = self.ctx.texts.borrow();
        let mut best: Option<(f64, usize)> = None;
        for text in texts.iter() {
            let Some(layout) = text.layout.upgrade() else {
                continue;
            };
            let rect = text.id.layout_rect();
            if rect.width() <= 0.0 && rect.height() <= 0.0 {
                continue;
            }
            let distance = if window.y < rect.y0 {
                rect.y0 - window.y
            } else if window.y > rect.y1 {
                window.y - rect.y1
            } else {
                0.0
            };
            let horizontal = if window.x < rect.x0 {
                rect.x0 - window.x
            } else if window.x > rect.x1 {
                window.x - rect.x1
            } else {
                0.0
            };
            let score = distance * 1000.0 + horizontal;
            if best.is_none_or(|(best, _)| score < best) {
                let local = Point::new(window.x - rect.x0, window.y - rect.y0);
                let offset = if window.y < rect.y0 {
                    text.start
                } else if window.y > rect.y1 {
                    text.start + review_markdown::utf16_len(&text.text)
                } else {
                    offset_in(&layout.borrow(), &text.text, text.start, local)
                };
                best = Some((score, offset));
            }
        }
        best.map(|(_, offset)| offset)
    }
}

impl Drop for TextBlock {
    fn drop(&mut self) {
        let id = self.id;
        if let Ok(mut texts) = self.ctx.texts.try_borrow_mut() {
            texts.retain(|text| text.id != id);
        }
    }
}

/// The page offset at `point` of a laid-out text starting at page offset `start`.
fn offset_in(layout: &TextLayout, text: &str, start: usize, point: Point) -> usize {
    let hit = layout.hit_point(point);
    let line_start = layout.lines_range().get(hit.line).map_or(0, |range| range.start);
    let byte = (line_start + hit.index).min(text.len());
    start + review_markdown::byte_to_utf16(text, byte)
}

/// The cursor of byte `index` in a layout's text.
fn cursor_of(layout: &TextLayout, index: usize) -> Cursor {
    let lines = layout.lines_range();
    let line = lines
        .iter()
        .position(|range| index <= range.end)
        .unwrap_or(lines.len().saturating_sub(1));
    let line_start = lines.get(line).map_or(0, |range| range.start);
    Cursor::new(line, index.saturating_sub(line_start))
}

/// Fills the rects of the text from byte `from` to `to`.
fn fill_range(cx: &mut PaintCx, layout: &TextLayout, from: usize, to: usize, color: Color) {
    if to <= from {
        return;
    }
    let (start, end) = (cursor_of(layout, from), cursor_of(layout, to));
    for run in layout.layout_runs() {
        if let Some((x, width)) = run.highlight(start, end) {
            if width <= 0.0 {
                continue;
            }
            let top = f64::from(run.line_top);
            let rect = Rect::new(f64::from(x), top, f64::from(x + width), top + f64::from(run.line_height));
            cx.fill(&rect.to_rounded_rect(2.0), color, 0.0);
        }
    }
}

impl View for TextBlock {
    fn id(&self) -> ViewId {
        self.id
    }

    fn view_style(&self) -> Option<Style> {
        Some(Style::new().cursor(CursorStyle::Text).min_width(0.0))
    }

    fn debug_name(&self) -> std::borrow::Cow<'static, str> {
        format!("Page text at {}", self.inline.start).into()
    }

    fn update(&mut self, _cx: &mut UpdateCx, state: Box<dyn Any>) {
        if let Ok(layout) = state.downcast::<TextLayout>() {
            self.natural = *layout;
            *self.layout.borrow_mut() = self.natural.clone();
            self.available_width = None;
            self.id.request_layout();
        }
    }

    fn layout(&mut self, cx: &mut LayoutCx) -> NodeId {
        cx.layout_node(self.id(), true, |_cx| {
            let natural = self.natural.size();
            let height = self.layout.borrow().size().height.max(natural.height) as f32;
            let width = natural.width as f32;
            let node = *self.text_node.get_or_insert_with(|| self.id.new_taffy_node());
            let style = Style::new().width(width).height(height).to_taffy_style();
            self.id.set_taffy_style(node, style);
            vec![node]
        })
    }

    fn compute_layout(&mut self, _cx: &mut ComputeLayoutCx) -> Option<Rect> {
        let width = self.id.get_layout().map_or(0.0, |layout| layout.size.width);
        let natural = self.natural.size().width as f32;
        if natural > width && width > 0.0 {
            if self.available_width != Some(width) {
                let mut wrapped = self.natural.clone();
                wrapped.set_size(width, f32::MAX);
                *self.layout.borrow_mut() = wrapped;
                self.available_width = Some(width);
                self.id.request_layout();
            }
        } else if self.available_width.is_some() {
            *self.layout.borrow_mut() = self.natural.clone();
            self.available_width = None;
            self.id.request_layout();
        }
        None
    }

    fn event_before_children(&mut self, cx: &mut EventCx, event: &Event) -> EventPropagation {
        match event {
            Event::PointerDown(pointer) if pointer.button.is_primary() => {
                let offset = self.offset_at(pointer.pos);
                if pointer.modifiers.shift()
                    && let Some((anchor, _)) = self.ctx.selection.get_untracked()
                {
                    self.ctx.selection.set(Some((anchor, offset)));
                    self.dragging = true;
                } else {
                    self.ctx.selection.set(None);
                    self.ctx.actions_at.set(None);
                    self.dragging = false;
                }
                self.pressed = Some((pointer.pos, offset));
                cx.update_active(self.id);
                EventPropagation::Stop
            }
            Event::PointerMove(pointer) => {
                let Some((pressed_at, anchor)) = self.pressed else {
                    return EventPropagation::Continue;
                };
                if !self.dragging && pressed_at.distance(pointer.pos) < 3.0 {
                    return EventPropagation::Stop;
                }
                self.dragging = true;
                let origin = self.id.layout_rect().origin();
                let window = Point::new(pointer.pos.x + origin.x, pointer.pos.y + origin.y);
                let focus = self.offset_at_window(window).unwrap_or(anchor);
                let anchor = self.ctx.selection.get_untracked().map_or(anchor, |(anchor, _)| anchor);
                self.ctx.selection.set(Some((anchor, focus)));
                EventPropagation::Stop
            }
            Event::PointerUp(pointer) => {
                let Some((_, offset)) = self.pressed.take() else {
                    return EventPropagation::Continue;
                };
                self.id.clear_active();
                let selected = self.ctx.selection.get_untracked().filter(|(anchor, focus)| anchor != focus);
                if self.dragging && selected.is_some() {
                    let origin = self.id.layout_rect().origin();
                    let content = self.ctx.content.borrow().map(|content| content.layout_rect().origin());
                    let at = content.map_or(Point::ZERO, |content| {
                        Point::new(origin.x + pointer.pos.x - content.x, origin.y + pointer.pos.y - content.y)
                    });
                    self.ctx.actions_at.set(Some(at));
                } else {
                    self.ctx.selection.set(None);
                    // A click: a link or a comment's mark.
                    let units = offset - self.start();
                    let byte = review_markdown::utf16_to_byte(&self.inline.text, units);
                    if let Some(link) = self.inline.link_at(byte) {
                        (self.ctx.on_link)(link.to_string());
                    } else if let Some(mark) = self
                        .ctx
                        .marks
                        .with_untracked(|marks| marks.iter().find(|mark| mark.start <= offset && offset < mark.end).cloned())
                    {
                        (self.ctx.on_mark)(mark.thread_id);
                    }
                }
                self.dragging = false;
                EventPropagation::Stop
            }
            _ => EventPropagation::Continue,
        }
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let layout = self.layout.borrow();
        let config = self.ctx.config.get_untracked();
        let start = self.start();
        let text = &self.inline.text;
        let end = start + review_markdown::utf16_len(text);
        let to_bytes = |from: usize, to: usize| {
            (
                review_markdown::utf16_to_byte(text, from.saturating_sub(start)),
                review_markdown::utf16_to_byte(text, to.saturating_sub(start)),
            )
        };
        // Code and badge backgrounds.
        for span in &self.inline.spans {
            if span.style.code || span.style.badge.is_some() {
                let color = match span.style.badge {
                    Some(badge) => badge_color(badge, &config).multiply_alpha(0.15),
                    None => config.color(LapceColor::PANEL_BACKGROUND),
                };
                fill_range(cx, &layout, span.range.start, span.range.end, color);
            }
        }
        // Comment marks.
        let mark_color = config.color(LapceColor::LAPCE_WARN).multiply_alpha(0.22);
        self.ctx.marks.with_untracked(|marks| {
            for mark in marks.iter().filter(|mark| mark.start < end && mark.end > start) {
                let (from, to) = to_bytes(mark.start.max(start), mark.end.min(end));
                fill_range(cx, &layout, from, to, mark_color);
            }
        });
        // The selection.
        if let Some((anchor, focus)) = self.ctx.selection.get_untracked() {
            let (from, to) = (anchor.min(focus), anchor.max(focus));
            if from < end && to > start {
                let (from, to) = to_bytes(from.max(start), to.min(end));
                fill_range(cx, &layout, from, to, config.color(LapceColor::EDITOR_SELECTION));
            }
        }
        cx.draw_text(&layout, Point::ZERO);
    }
}

/// A page's comment actions for the selection: shown where the drag ended.
pub fn selection_actions(ctx: &PageContext, page: Memo<Option<Rc<Page>>>, has_agent: Memo<bool>) -> impl View + use<> {
    let (selection, at, config) = (ctx.selection, ctx.actions_at, ctx.config);
    let on_comment = ctx.on_comment.clone();
    let act = move |action: Action| {
        let Some((anchor, focus)) = selection.get_untracked() else {
            return;
        };
        let anchor = page.with_untracked(|page| {
            page.as_ref()
                .and_then(|page| review_markdown::text_anchor(&page.text, anchor.min(focus), anchor.max(focus)))
        });
        selection.set(None);
        at.set(None);
        if let Some(anchor) = anchor {
            on_comment(anchor, action);
        }
    };
    let copy = move || {
        let Some((anchor, focus)) = selection.get_untracked() else {
            return;
        };
        let text = page.with_untracked(|page| {
            page.as_ref().map(|page| {
                let units: Vec<u16> = page.text.encode_utf16().collect();
                String::from_utf16_lossy(units.get(anchor.min(focus)..anchor.max(focus)).unwrap_or_default())
            })
        });
        if let Some(text) = text
            && let Err(error) = floem::Clipboard::set_contents(text)
        {
            tracing::error!("copying the selection: {error:?}");
        }
        selection.set(None);
        at.set(None);
    };
    let button = move |text: &'static str, primary: bool, run: Rc<dyn Fn()>| {
        label(move || text)
            .on_event_stop(EventListener::PointerDown, move |_| run())
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(8.0)
                    .padding_vert(3.0)
                    .margin_right(4.0)
                    .border_radius(4.0)
                    .cursor(CursorStyle::Pointer)
                    .selectable(false)
                    .apply_if(primary, |s| {
                        s.background(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND))
                            .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
                    })
                    .hover(|s| s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND)))
            })
    };
    let multi_line = move || {
        let Some((anchor, focus)) = selection.get() else {
            return false;
        };
        page.with(|page| {
            page.as_ref().is_some_and(|page| {
                let units: Vec<u16> = page.text.encode_utf16().collect();
                units
                    .get(anchor.min(focus)..anchor.max(focus))
                    .is_some_and(|selected| selected.contains(&u16::from(b'\n')))
            })
        })
    };
    let act = Rc::new(act);
    dyn_container(
        move || (has_agent.get(), multi_line()),
        move |(agent, multi_line)| {
            let mut buttons: Vec<floem::AnyView> = Vec::new();
            if agent {
                let act = act.clone();
                buttons.push(button("Ask Agent", multi_line, Rc::new(move || act(Action::Ask))).into_any());
            }
            let comment = act.clone();
            buttons.push(
                button("Comment", !agent || !multi_line, Rc::new(move || comment(Action::Comment))).into_any(),
            );
            let edit = act.clone();
            buttons.push(button("Propose Edit", false, Rc::new(move || edit(Action::ProposeEdit))).into_any());
            buttons.push(button("Copy", false, Rc::new(copy)).into_any());
            h_stack_from_iter(buttons).into_any()
        },
    )
    .on_event_stop(EventListener::PointerDown, |_| {})
    .style(move |s| {
        let config = config.get();
        let place = at.get();
        let place = place.filter(|_| selection.with(|selection| selection.is_some()));
        match place {
            Some(point) => s
                .absolute()
                .inset_left((point.x - 80.0).max(4.0) as f32)
                .inset_top((point.y + 14.0) as f32)
                .padding(4.0)
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_BACKGROUND))
                .box_shadow_blur(6.0)
                .box_shadow_color(config.color(LapceColor::LAPCE_DROPDOWN_SHADOW))
                .z_index(10),
            None => s.hide(),
        }
    })
}

/// The comment action of a step or a diagram: the review icon, with a tooltip.
pub fn comment_icon(
    config: ReadSignal<Arc<LapceConfig>>,
    tooltip: &'static str,
    on_click: impl Fn() + 'static,
) -> impl View {
    crate::app::tooltip_label(
        config,
        svg(move || config.get().ui_svg(crate::config::icon::LapceIcons::REVIEW)).style(move |s| {
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

/// Steps in a page's tree: each with its text and comment marks.
pub fn step_text(ctx: &PageContext, step: &StepNode) -> TextBlock {
    text_block(ctx, &step.text, TextKind::Body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str, width: Option<f32>) -> TextLayout {
        let mut layout = TextLayout::new();
        if let Some(width) = width {
            layout.set_size(width, f32::MAX);
        }
        layout.set_text(text, AttrsList::new(Attrs::new().font_size(14.0)), None);
        layout
    }

    #[test]
    fn a_point_maps_to_its_offset_on_the_page() {
        // The block's text starts at page offset 100; "é" is one UTF-16 unit, two bytes.
        let text = "café one\nsecond line";
        let layout = layout(text, None);
        assert_eq!(offset_in(&layout, text, 100, Point::new(0.0, 2.0)), 100);
        // Past the end of the first line: its end.
        assert_eq!(offset_in(&layout, text, 100, Point::new(10_000.0, 2.0)), 108);
        // On the second line, at its start.
        let second_line_y = layout.size().height * 0.75;
        assert_eq!(offset_in(&layout, text, 100, Point::new(0.0, second_line_y)), 109);
    }

    #[test]
    fn byte_offsets_become_cursors_on_their_line() {
        let text = "one\ntwo three";
        let layout = layout(text, None);
        assert_eq!(cursor_of(&layout, 0), Cursor::new(0, 0));
        assert_eq!(cursor_of(&layout, 3), Cursor::new(0, 3));
        assert_eq!(cursor_of(&layout, 4), Cursor::new(1, 0));
        assert_eq!(cursor_of(&layout, 13), Cursor::new(1, 9));
    }

    #[test]
    fn wrapped_text_maps_points_on_later_rows() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";
        let wrapped = layout(text, Some(80.0));
        let rows = wrapped.layout_runs().count();
        assert!(rows > 1, "the text wraps");
        // The start of the last row is past the first row's text.
        let last_row_y = wrapped.size().height - 2.0;
        assert!(offset_in(&wrapped, text, 0, Point::new(0.0, last_row_y)) > 20);
    }
}
