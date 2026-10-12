//! A Markdown document as a page: blocks to draw, and the page's text as the Theia app's DOM has it.
//!
//! Built in two passes. The first parses the Markdown (pulldown-cmark, CommonMark with GFM tables and strikethrough;
//! raw HTML stays text, as with markdown-it's `html: false`) into blocks. The second lays the blocks out as the tokens
//! markdown-it renders them to and writes the page's text: the inline text of each block, and the newlines markdown-it
//! puts between block tags (`Renderer.renderToken`), which are text nodes in the DOM. Every inline then knows where
//! its text starts on the page.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::LazyLock;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use regex::Regex;
use review_design::{DocumentFormat, Mode, document_format, sections_of, split_frontmatter, step_id, steps};

use crate::text::utf16_len;

/// A document rendered as a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub title: String,
    pub format: DocumentFormat,
    /// Rendered as a design: sections, with step trees for `## Design` and `## Open Questions`.
    pub design: bool,
    pub blocks: Vec<Block>,
    /// The Mermaid blocks, in document order; [`Block::Diagram`] points into it.
    pub diagrams: Vec<Diagram>,
    /// The page's text, as text anchors see it.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagram {
    /// `%% id: <name>`, else `block-<n>`.
    pub id: String,
    /// The Mermaid source without its id line.
    pub code: String,
}

/// Text with styles, at a place in the page's text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    /// Where `text` starts in the page's text (UTF-16).
    pub start: usize,
    /// Styled byte ranges of `text`, in order, not overlapping.
    pub spans: Vec<Span>,
    /// Images in the text (they have no text of their own).
    pub images: Vec<Image>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
    /// A design step's marker (`?`, `⚠`, `✎`).
    pub badge: Option<Badge>,
    /// A design step's reason, after ` — `: shown muted.
    pub reason: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// `?`: an open question.
    Question,
    /// `⚠`: a risk.
    Risk,
    /// `✎`: a new name or artifact.
    Name,
}

impl Badge {
    pub fn glyph(self) -> &'static str {
        match self {
            Badge::Question => "?",
            Badge::Risk => "\u{26a0}",
            Badge::Name => "\u{270e}",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Badge::Question => "open question",
            Badge::Risk => "risk",
            Badge::Name => "naming / new artifact",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub url: String,
    pub alt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}

/// What a knowledge-base page says about itself in its frontmatter (`type`, `description`, `resource`).
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub kind: Inline,
    pub description: Option<Inline>,
    /// The code the page is about, as a link.
    pub resource: Option<Inline>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Meta(Meta),
    /// The format warnings banner: its title and one item per warning.
    Warnings { title: Inline, items: Vec<Inline> },
    Heading { level: u8, text: Inline },
    Paragraph(Inline),
    Code { language: String, text: Inline },
    Quote(Vec<Block>),
    List { start: Option<u64>, tight: bool, items: Vec<Vec<Block>> },
    Table { alignments: Vec<Alignment>, head: Vec<Inline>, rows: Vec<Vec<Inline>> },
    Rule,
    /// A Mermaid block: `head` is its id, as the diagram's header shows it.
    Diagram { index: usize, head: Inline },
    /// A design's `## ` section; `altitude`: it has the L1 · L2 · L3 switch (the Design step tree).
    Section { heading: Inline, altitude: bool, blocks: Vec<Block> },
    /// A step tree; `design`: the Design section's (folded by level).
    Tree { design: bool, steps: Vec<StepNode> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepNode {
    /// The step's anchor id (`review_design::step_id`).
    pub id: String,
    /// The step's text as written (its anchor's `source`).
    pub source: String,
    pub depth: u32,
    pub text: Inline,
    pub children: Vec<StepNode>,
}

/// The title of a document: frontmatter `title`, else its first `# ` heading (without `Feature:`), else `fallback`.
pub fn title(markdown: &str, fallback: &str) -> String {
    static HEADING: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?m)^#\s+([^\r\n]+?)\s*$").ok());
    static FEATURE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?i)^Feature:\s*").ok());
    let frontmatter = split_frontmatter(markdown);
    if let Some(title) = frontmatter.data.get("title").filter(|title| !title.is_empty()) {
        return title.clone();
    }
    let heading = HEADING
        .as_ref()
        .and_then(|heading| heading.captures(&frontmatter.body))
        .and_then(|captures| captures.get(1))
        .map(|heading| heading.as_str().to_string());
    match (heading, FEATURE.as_ref()) {
        (Some(heading), Some(feature)) => feature.replace(&heading, "").into_owned(),
        (Some(heading), None) => heading,
        (None, _) => fallback.to_string(),
    }
}

/// Renders `markdown` (the file `file_name`) as a page.
pub fn render(markdown: &str, file_name: &str) -> Page {
    let format = document_format(markdown, file_name);
    let frontmatter = split_frontmatter(markdown);
    let page_title = title(markdown, file_name);
    let mut writer = Writer::default();
    let mut blocks = Vec::new();
    let mut diagrams = Vec::new();

    // What the page adds above the document: the page's header, then the format warnings.
    if let Some(kind) = frontmatter.data.get("type").filter(|kind| !kind.is_empty())
        && !frontmatter.data.contains_key("co-review")
    {
        let mut meta = Meta {
            kind: Inline::plain(kind),
            description: frontmatter
                .data
                .get("description")
                .filter(|description| !description.is_empty())
                .map(|description| Inline::plain(description)),
            resource: frontmatter
                .data
                .get("resource")
                .filter(|resource| !resource.is_empty())
                .map(|resource| Inline::styled(resource, Style { link: Some(resource.clone()), ..Style::default() })),
        };
        writer.inline(&mut meta.kind);
        if let Some(description) = &mut meta.description {
            writer.inline(description);
        }
        if let Some(resource) = &mut meta.resource {
            writer.inline(resource);
        }
        blocks.push(Block::Meta(meta));
    }
    if !format.warnings.is_empty() {
        let mut heading = if format.declared {
            code_spans("Declares `co-review: design`, but:")
        } else {
            Inline::plain("Format:")
        };
        writer.inline(&mut heading);
        let mut items: Vec<Inline> = format.warnings.iter().map(|warning| code_spans(warning)).collect();
        for item in &mut items {
            writer.inline(item);
        }
        blocks.push(Block::Warnings { title: heading, items });
    }

    let design = format.mode == Mode::Design;
    if design {
        let mut heading = Inline::plain(&page_title);
        writer.inline(&mut heading);
        blocks.push(Block::Heading { level: 1, text: heading });
        let sections = sections_of(&frontmatter.body);
        blocks.extend(prose(&sections.preamble, &mut diagrams, true, &mut writer));
        for section in sections.sections {
            let tree_kind = match section.key.as_str() {
                "design" => Some(true),
                "open questions" => Some(false),
                _ => None,
            };
            let tree = tree_kind.map(|design| (design, steps(&section.body))).filter(|(_, steps)| !steps.is_empty());
            let mut heading = Inline::plain(&section.heading);
            writer.inline(&mut heading);
            let altitude = tree.as_ref().is_some_and(|(design, _)| *design);
            if altitude {
                writer.gap(" L1L2L3");
            }
            let section_blocks = match tree {
                Some((design, tree)) => {
                    let mut nodes: Vec<StepNode> = tree.iter().map(step_node).collect();
                    for node in &mut nodes {
                        place_step(node, &mut writer);
                    }
                    vec![Block::Tree { design, steps: nodes }]
                }
                None => prose(&section.body, &mut diagrams, true, &mut writer),
            };
            blocks.push(Block::Section { heading, altitude, blocks: section_blocks });
        }
    } else {
        blocks.extend(prose(&frontmatter.body, &mut diagrams, false, &mut writer));
    }

    Page {
        title: page_title,
        format,
        design,
        blocks,
        diagrams,
        text: writer.text,
    }
}

/// The page's text being written, and how long it is in UTF-16 code units.
#[derive(Default)]
struct Writer {
    text: String,
    units: usize,
}

impl Writer {
    fn gap(&mut self, text: &str) {
        self.text.push_str(text);
        self.units += utf16_len(text);
    }

    fn inline(&mut self, inline: &mut Inline) {
        inline.start = self.units;
        self.gap(&inline.text.clone());
    }
}

impl Inline {
    fn plain(text: &str) -> Self {
        Inline {
            text: text.to_string(),
            ..Inline::default()
        }
    }

    fn styled(text: &str, style: Style) -> Self {
        let mut inline = Inline::default();
        inline.push(text, &style);
        inline
    }

    fn push(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        if *style != Style::default() {
            match self.spans.last_mut() {
                Some(last) if last.range.end == start && last.style == *style => last.range.end = self.text.len(),
                _ => self.spans.push(Span {
                    range: start..self.text.len(),
                    style: style.clone(),
                }),
            }
        }
    }

    /// The style at byte `index` of the text.
    pub fn style_at(&self, index: usize) -> Option<&Style> {
        self.spans
            .iter()
            .find(|span| span.range.contains(&index))
            .map(|span| &span.style)
    }

    /// The link at byte `index` of the text.
    pub fn link_at(&self, index: usize) -> Option<&str> {
        self.style_at(index).and_then(|style| style.link.as_deref())
    }

    /// The end of the text on the page (UTF-16).
    pub fn end(&self) -> usize {
        self.start + utf16_len(&self.text)
    }
}

/// `` `code` `` in a warning as code (the banner's rendering of warnings).
fn code_spans(text: &str) -> Inline {
    static CODE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"`([^`]+)`").ok());
    let mut inline = Inline::default();
    let Some(code) = CODE.as_ref() else {
        inline.push(text, &Style::default());
        return inline;
    };
    let mut last = 0;
    for captures in code.captures_iter(text) {
        let (Some(whole), Some(inner)) = (captures.get(0), captures.get(1)) else {
            continue;
        };
        inline.push(text.get(last..whole.start()).unwrap_or(""), &Style::default());
        inline.push(inner.as_str(), &Style { code: true, ..Style::default() });
        last = whole.end();
    }
    inline.push(text.get(last..).unwrap_or(""), &Style::default());
    inline
}

// Private-use characters marking a step's styles while its text is transformed as `stepInline` transforms HTML.
const BADGE_QUESTION: char = '\u{e001}';
const BADGE_RISK: char = '\u{e002}';
const BADGE_NAME: char = '\u{e003}';
const BADGE_END: char = '\u{e00f}';
const CODE_START: char = '\u{e010}';
const CODE_END: char = '\u{e011}';
const STRONG_START: char = '\u{e020}';
const STRONG_END: char = '\u{e021}';
const EM_START: char = '\u{e030}';
const EM_END: char = '\u{e031}';

/// A step's text with its markers, code, bold, italic and reason (`stepInline`).
fn step_inline(text: &str) -> Inline {
    static CODE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"`([^`]+)`").ok());
    static STRONG: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").ok());
    static EM: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\*([^*]+)\*").ok());
    let mut marked = text.to_string();
    for (glyph, start) in [("?", BADGE_QUESTION), ("\u{26a0}", BADGE_RISK), ("\u{270e}", BADGE_NAME)] {
        marked = marked.replace(&format!("`{glyph}`"), &format!("{start}{glyph}{BADGE_END}"));
    }
    for (pattern, start, end) in [(&CODE, CODE_START, CODE_END), (&STRONG, STRONG_START, STRONG_END), (&EM, EM_START, EM_END)] {
        if let Some(pattern) = pattern.as_ref() {
            marked = pattern.replace_all(&marked, format!("{start}${{1}}{end}")).into_owned();
        }
    }
    let reason = marked.find(" \u{2014} ").map(|at| at + 1);
    let mut inline = Inline::default();
    let mut style = Style::default();
    for (index, character) in marked.char_indices() {
        if Some(index) == reason {
            style.reason = true;
        }
        match character {
            BADGE_QUESTION => style.badge = Some(Badge::Question),
            BADGE_RISK => style.badge = Some(Badge::Risk),
            BADGE_NAME => style.badge = Some(Badge::Name),
            BADGE_END => style.badge = None,
            CODE_START | CODE_END => style.code = character == CODE_START,
            STRONG_START | STRONG_END => style.bold = character == STRONG_START,
            EM_START | EM_END => style.italic = character == EM_START,
            _ => inline.push(character.encode_utf8(&mut [0; 4]), &style),
        }
    }
    inline
}

fn step_node(step: &review_design::Step) -> StepNode {
    StepNode {
        id: step_id(&step.text),
        source: step.text.clone(),
        depth: step.depth,
        text: step_inline(&step.text),
        children: step.children.iter().map(step_node).collect(),
    }
}

fn place_step(node: &mut StepNode, writer: &mut Writer) {
    writer.inline(&mut node.text);
    for child in &mut node.children {
        place_step(child, writer);
    }
}

/// Markdown as blocks, its Mermaid blocks added to `diagrams`, its text written. `design`: a design's section, where
/// blocks without an id are numbered again from those before (`render` in `document-render.ts`), and blank text
/// renders nothing.
fn prose(markdown: &str, diagrams: &mut Vec<Diagram>, design: bool, writer: &mut Writer) -> Vec<Block> {
    if design && markdown.trim().is_empty() {
        return Vec::new();
    }
    let offset = diagrams.len();
    let mut blocks = parse(markdown, diagrams);
    if design {
        for (index, diagram) in diagrams.iter_mut().enumerate().skip(offset) {
            if diagram.id.starts_with("block-") {
                diagram.id = format!("block-{}", index + 1);
            }
        }
        for block in &mut blocks {
            rename_heads(block, diagrams);
        }
    }
    let mut tokens = Vec::new();
    for block in &blocks {
        tokens_of(block, false, &mut tokens);
    }
    let starts = write_tokens(&tokens, writer);
    let mut starts = starts.into_iter();
    for block in &mut blocks {
        assign_starts(block, &mut starts);
    }
    blocks
}

fn rename_heads(block: &mut Block, diagrams: &[Diagram]) {
    match block {
        Block::Diagram { index, head } => {
            if let Some(diagram) = diagrams.get(*index) {
                head.text.clone_from(&diagram.id);
            }
        }
        Block::Quote(children) => children.iter_mut().for_each(|child| rename_heads(child, diagrams)),
        Block::List { items, .. } => items
            .iter_mut()
            .flatten()
            .for_each(|child| rename_heads(child, diagrams)),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------------------------
// First pass: Markdown to blocks
// ---------------------------------------------------------------------------------------------------------------

enum Frame {
    Quote(Vec<Block>),
    List { start: Option<u64>, items: Vec<Vec<Block>>, paragraphs: bool },
    /// A list item: its blocks, and the text written directly in it (a tight list's).
    Item { blocks: Vec<Block>, text: Option<InlineBuilder> },
    Paragraph(InlineBuilder),
    Heading(u8, InlineBuilder),
    Code { language: String, text: String },
    Html(String),
    Table { alignments: Vec<Alignment>, head: Vec<Inline>, rows: Vec<Vec<Inline>> },
    Row(Vec<Inline>),
    Cell(InlineBuilder),
    /// Footnotes, metadata: not rendered.
    Skipped,
}

#[derive(Default)]
struct InlineBuilder {
    inline: Inline,
    styles: Vec<Style>,
    /// Inside an image: its text is the image's alt text.
    image: Option<Image>,
}

impl InlineBuilder {
    fn style(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    fn text(&mut self, text: &str) {
        if let Some(image) = &mut self.image {
            image.alt.push_str(text);
            return;
        }
        let style = self.style();
        if style.link.is_some() {
            self.inline.push(text, &style);
            return;
        }
        // Bare URLs are links (markdown-it's `linkify`).
        static URL: LazyLock<Option<Regex>> = LazyLock::new(|| {
            Regex::new(r#"(?:https?://|www\.)[^\s<>"']*[^\s<>"'.,;:!?)\]}]"#).ok()
        });
        let mut last = 0;
        if let Some(url) = URL.as_ref() {
            for found in url.find_iter(text) {
                self.inline.push(text.get(last..found.start()).unwrap_or(""), &style);
                let target = if found.as_str().starts_with("www.") {
                    format!("http://{}", found.as_str())
                } else {
                    found.as_str().to_string()
                };
                self.inline.push(found.as_str(), &Style { link: Some(target), ..style.clone() });
                last = found.end();
            }
        }
        self.inline.push(text.get(last..).unwrap_or(""), &style);
    }

    fn open(&mut self, change: impl FnOnce(&mut Style)) {
        let mut style = self.style();
        change(&mut style);
        self.styles.push(style);
    }

    fn finish(self) -> Inline {
        self.inline
    }
}

fn parse(markdown: &str, diagrams: &mut Vec<Diagram>) -> Vec<Block> {
    let parser = Parser::new_ext(markdown, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH);
    let mut root: Vec<Block> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                // Text written directly in a list item ends where a block starts in it.
                if !matches!(
                    tag,
                    Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
                ) && let Some(Frame::Item { blocks, text }) = stack.last_mut()
                    && let Some(text) = text.take()
                {
                    blocks.push(Block::Paragraph(text.finish()));
                }
                let frame = match tag {
                    Tag::Paragraph => {
                        if let Some(Frame::List { paragraphs, .. }) = stack.iter_mut().rev().nth(1) {
                            *paragraphs = true;
                        }
                        Frame::Paragraph(InlineBuilder::default())
                    }
                    Tag::Heading { level, .. } => Frame::Heading(heading_level(level), InlineBuilder::default()),
                    Tag::BlockQuote(_) => Frame::Quote(Vec::new()),
                    Tag::CodeBlock(kind) => Frame::Code {
                        language: match kind {
                            CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                            CodeBlockKind::Indented => String::new(),
                        },
                        text: String::new(),
                    },
                    Tag::HtmlBlock => Frame::Html(String::new()),
                    Tag::List(start) => Frame::List { start, items: Vec::new(), paragraphs: false },
                    Tag::Item => Frame::Item { blocks: Vec::new(), text: None },
                    Tag::Table(alignments) => Frame::Table {
                        alignments: alignments.into_iter().map(alignment).collect(),
                        head: Vec::new(),
                        rows: Vec::new(),
                    },
                    Tag::TableHead | Tag::TableRow => Frame::Row(Vec::new()),
                    Tag::TableCell => Frame::Cell(InlineBuilder::default()),
                    Tag::Emphasis => {
                        with_inline(&mut stack, |inline| inline.open(|style| style.italic = true));
                        continue;
                    }
                    Tag::Strong => {
                        with_inline(&mut stack, |inline| inline.open(|style| style.bold = true));
                        continue;
                    }
                    Tag::Strikethrough => {
                        with_inline(&mut stack, |inline| inline.open(|style| style.strike = true));
                        continue;
                    }
                    Tag::Link { dest_url, .. } => {
                        with_inline(&mut stack, |inline| {
                            inline.open(|style| style.link = Some(dest_url.to_string()));
                        });
                        continue;
                    }
                    Tag::Image { dest_url, .. } => {
                        with_inline(&mut stack, |inline| {
                            inline.image = Some(Image {
                                url: dest_url.to_string(),
                                alt: String::new(),
                            });
                        });
                        continue;
                    }
                    Tag::FootnoteDefinition(_) | Tag::MetadataBlock(_) => Frame::Skipped,
                };
                stack.push(frame);
            }
            Event::End(end) => {
                match end {
                    TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                        with_inline(&mut stack, |inline| {
                            inline.styles.pop();
                        });
                        continue;
                    }
                    TagEnd::Image => {
                        with_inline(&mut stack, |inline| {
                            if let Some(image) = inline.image.take() {
                                inline.inline.images.push(image);
                            }
                        });
                        continue;
                    }
                    _ => {}
                }
                let Some(frame) = stack.pop() else {
                    continue;
                };
                let block = match frame {
                    Frame::Quote(blocks) => Some(Block::Quote(blocks)),
                    Frame::List { start, items, paragraphs } => Some(Block::List {
                        start,
                        tight: !paragraphs,
                        items,
                    }),
                    Frame::Item { mut blocks, text } => {
                        if let Some(text) = text {
                            blocks.push(Block::Paragraph(text.finish()));
                        }
                        if let Some(Frame::List { items, .. }) = stack.last_mut() {
                            items.push(blocks);
                        }
                        None
                    }
                    Frame::Paragraph(inline) => Some(Block::Paragraph(inline.finish())),
                    Frame::Heading(level, inline) => Some(Block::Heading { level, text: inline.finish() }),
                    Frame::Code { language, text } if language == "mermaid" => {
                        let diagram = diagram(&text, diagrams.len());
                        let head = Inline::plain(&diagram.id);
                        diagrams.push(diagram);
                        Some(Block::Diagram { index: diagrams.len() - 1, head })
                    }
                    Frame::Code { language, text } => Some(Block::Code {
                        language,
                        text: Inline::styled(text.strip_suffix('\n').unwrap_or(&text), Style { code: true, ..Style::default() }),
                    }),
                    Frame::Html(text) => Some(Block::Paragraph(Inline::plain(text.trim_end_matches('\n')))),
                    Frame::Table { alignments, head, rows } => Some(Block::Table { alignments, head, rows }),
                    Frame::Row(cells) => {
                        match (end, stack.last_mut()) {
                            (TagEnd::TableHead, Some(Frame::Table { head, .. })) => *head = cells,
                            (_, Some(Frame::Table { rows, .. })) => rows.push(cells),
                            _ => {}
                        }
                        None
                    }
                    Frame::Cell(inline) => {
                        if let Some(Frame::Row(cells)) = stack.last_mut() {
                            cells.push(inline.finish());
                        }
                        None
                    }
                    Frame::Skipped => None,
                };
                if let Some(block) = block {
                    match stack.last_mut() {
                        None => root.push(block),
                        Some(Frame::Quote(blocks) | Frame::Item { blocks, .. }) => blocks.push(block),
                        Some(_) => {}
                    }
                }
            }
            Event::Text(text) => match stack.last_mut() {
                Some(Frame::Code { text: code, .. }) => code.push_str(&text),
                Some(Frame::Html(html)) => html.push_str(&text),
                _ => with_inline(&mut stack, |inline| inline.text(&text)),
            },
            Event::Code(code) => with_inline(&mut stack, |inline| {
                inline.open(|style| style.code = true);
                inline.text(&code);
                inline.styles.pop();
            }),
            Event::Html(html) => match stack.last_mut() {
                Some(Frame::Html(text)) => text.push_str(&html),
                _ => with_inline(&mut stack, |inline| inline.text(&html)),
            },
            Event::InlineHtml(html) => with_inline(&mut stack, |inline| inline.text(&html)),
            Event::SoftBreak | Event::HardBreak => with_inline(&mut stack, |inline| inline.text("\n")),
            Event::Rule => {
                let block = Block::Rule;
                match stack.last_mut() {
                    None => root.push(block),
                    Some(Frame::Quote(blocks) | Frame::Item { blocks, .. }) => blocks.push(block),
                    Some(_) => {}
                }
            }
            Event::InlineMath(text) | Event::DisplayMath(text) | Event::FootnoteReference(text) => {
                with_inline(&mut stack, |inline| inline.text(&text));
            }
            Event::TaskListMarker(_) => {}
        }
    }
    root
}

/// Runs `apply` on the inline text being written: a paragraph's, heading's or cell's, or a list item's own text.
fn with_inline(stack: &mut [Frame], apply: impl FnOnce(&mut InlineBuilder)) {
    match stack.last_mut() {
        Some(Frame::Paragraph(inline) | Frame::Heading(_, inline) | Frame::Cell(inline)) => apply(inline),
        Some(Frame::Item { text, .. }) => apply(text.get_or_insert_with(InlineBuilder::default)),
        _ => {}
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alignment(alignment: pulldown_cmark::Alignment) -> Alignment {
    match alignment {
        pulldown_cmark::Alignment::None => Alignment::None,
        pulldown_cmark::Alignment::Left => Alignment::Left,
        pulldown_cmark::Alignment::Center => Alignment::Center,
        pulldown_cmark::Alignment::Right => Alignment::Right,
    }
}

/// A Mermaid block: `%% id: <name>` (anywhere in it) names it, and its first id line is left out of the source.
fn diagram(code: &str, index: usize) -> Diagram {
    static ID: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"%%\s*id:\s*([\w-]+)").ok());
    static ID_LINE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"%%\s*id:[^\n]*\n").ok());
    let id = ID
        .as_ref()
        .and_then(|id| id.captures(code))
        .and_then(|captures| captures.get(1))
        .map_or_else(|| format!("block-{}", index + 1), |id| id.as_str().to_string());
    let code = match ID_LINE.as_ref() {
        Some(line) => line.replace(code, "").into_owned(),
        None => code.to_string(),
    };
    Diagram {
        id,
        code: code.trim().to_string(),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Second pass: blocks to markdown-it's tokens, and the page's text
// ---------------------------------------------------------------------------------------------------------------

enum Token {
    Open { tag: &'static str, hidden: bool },
    Close { tag: &'static str, hidden: bool },
    Inline(String),
    /// A code block: its text, then the newline of `</code></pre>\n`.
    Fence(String),
    Rule,
}

impl Token {
    fn hidden(&self) -> bool {
        matches!(self, Token::Open { hidden: true, .. } | Token::Close { hidden: true, .. })
    }
}

fn tokens_of(block: &Block, tight: bool, tokens: &mut Vec<Token>) {
    let wrap = |tag: &'static str, hidden: bool, tokens: &mut Vec<Token>, inner: &dyn Fn(&mut Vec<Token>)| {
        tokens.push(Token::Open { tag, hidden });
        inner(tokens);
        tokens.push(Token::Close { tag, hidden });
    };
    match block {
        Block::Paragraph(inline) => wrap("p", tight, tokens, &|tokens| tokens.push(Token::Inline(inline.text.clone()))),
        // The placeholder paragraph a diagram's host replaces; its text is the diagram's header.
        Block::Diagram { head, .. } => wrap("p", tight, tokens, &|tokens| tokens.push(Token::Inline(head.text.clone()))),
        Block::Heading { level, text } => {
            let tag = ["h1", "h2", "h3", "h4", "h5", "h6"]
                .get(usize::from(level.saturating_sub(1)))
                .copied()
                .unwrap_or("h6");
            wrap(tag, false, tokens, &|tokens| tokens.push(Token::Inline(text.text.clone())));
        }
        Block::Code { text, .. } => tokens.push(Token::Fence(text.text.clone())),
        Block::Rule => tokens.push(Token::Rule),
        Block::Quote(children) => wrap("blockquote", false, tokens, &|tokens| {
            for child in children {
                tokens_of(child, false, tokens);
            }
        }),
        Block::List { start, tight, items } => {
            let tag = if start.is_some() { "ol" } else { "ul" };
            wrap(tag, false, tokens, &|tokens| {
                for item in items {
                    tokens.push(Token::Open { tag: "li", hidden: false });
                    for child in item {
                        tokens_of(child, *tight, tokens);
                    }
                    tokens.push(Token::Close { tag: "li", hidden: false });
                }
            });
        }
        Block::Table { head, rows, .. } => wrap("table", false, tokens, &|tokens| {
            let row = |tokens: &mut Vec<Token>, cells: &[Inline], cell: &'static str| {
                tokens.push(Token::Open { tag: "tr", hidden: false });
                for inline in cells {
                    tokens.push(Token::Open { tag: cell, hidden: false });
                    tokens.push(Token::Inline(inline.text.clone()));
                    tokens.push(Token::Close { tag: cell, hidden: false });
                }
                tokens.push(Token::Close { tag: "tr", hidden: false });
            };
            tokens.push(Token::Open { tag: "thead", hidden: false });
            row(tokens, head, "th");
            tokens.push(Token::Close { tag: "thead", hidden: false });
            if !rows.is_empty() {
                tokens.push(Token::Open { tag: "tbody", hidden: false });
                for cells in rows {
                    row(tokens, cells, "td");
                }
                tokens.push(Token::Close { tag: "tbody", hidden: false });
            }
        }),
        Block::Meta(_) | Block::Warnings { .. } | Block::Section { .. } | Block::Tree { .. } => {}
    }
}

/// Writes the tokens' text (`Renderer.renderToken`'s newlines) and returns where each inline and code text starts.
fn write_tokens(tokens: &[Token], writer: &mut Writer) -> Vec<usize> {
    let mut starts = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let after_hidden = index > 0 && tokens.get(index - 1).is_some_and(Token::hidden);
        match token {
            Token::Open { hidden: true, .. } | Token::Close { hidden: true, .. } => {}
            Token::Open { tag, .. } => {
                if after_hidden {
                    writer.gap("\n");
                }
                let newline = match tokens.get(index + 1) {
                    Some(Token::Inline(_)) => false,
                    Some(next) if next.hidden() => false,
                    Some(Token::Close { tag: next, .. }) => next != tag,
                    _ => true,
                };
                if newline {
                    writer.gap("\n");
                }
            }
            Token::Close { .. } => writer.gap("\n"),
            Token::Inline(text) => {
                starts.push(writer.units);
                writer.gap(text);
            }
            Token::Fence(text) => {
                starts.push(writer.units);
                writer.gap(text);
                writer.gap("\n");
                // The code's own last newline, which the block's text leaves out.
                if !text.is_empty() {
                    writer.gap("\n");
                }
            }
            Token::Rule => {
                if after_hidden {
                    writer.gap("\n");
                }
                writer.gap("\n");
            }
        }
    }
    starts
}

fn assign_starts(block: &mut Block, starts: &mut impl Iterator<Item = usize>) {
    let set = |inline: &mut Inline, starts: &mut dyn Iterator<Item = usize>| {
        inline.start = starts.next().unwrap_or(inline.start);
    };
    match block {
        Block::Paragraph(inline) | Block::Heading { text: inline, .. } | Block::Code { text: inline, .. } => set(inline, starts),
        Block::Diagram { head, .. } => set(head, starts),
        Block::Quote(children) => children.iter_mut().for_each(|child| assign_starts(child, starts)),
        Block::List { items, .. } => items
            .iter_mut()
            .flatten()
            .for_each(|child| assign_starts(child, starts)),
        Block::Table { head, rows, .. } => {
            for inline in head.iter_mut().chain(rows.iter_mut().flatten()) {
                set(inline, starts);
            }
        }
        Block::Meta(_) | Block::Warnings { .. } | Block::Section { .. } | Block::Tree { .. } | Block::Rule => {}
    }
}

/// Every inline of the blocks, in page order, with the step it belongs to.
pub(crate) fn inlines(blocks: &[Block]) -> Vec<&Inline> {
    fn steps<'a>(nodes: &'a [StepNode], out: &mut Vec<&'a Inline>) {
        for node in nodes {
            out.push(&node.text);
            steps(&node.children, out);
        }
    }
    fn walk<'a>(blocks: &'a [Block], out: &mut Vec<&'a Inline>) {
        for block in blocks {
            match block {
                Block::Meta(meta) => {
                    out.push(&meta.kind);
                    out.extend(meta.description.iter());
                    out.extend(meta.resource.iter());
                }
                Block::Warnings { title, items } => {
                    out.push(title);
                    out.extend(items.iter());
                }
                Block::Paragraph(inline) | Block::Heading { text: inline, .. } | Block::Code { text: inline, .. } => {
                    out.push(inline);
                }
                Block::Diagram { head, .. } => out.push(head),
                Block::Quote(children) => walk(children, out),
                Block::List { items, .. } => items.iter().for_each(|item| walk(item, out)),
                Block::Table { head, rows, .. } => out.extend(head.iter().chain(rows.iter().flatten())),
                Block::Section { heading, blocks, .. } => {
                    out.push(heading);
                    walk(blocks, out);
                }
                Block::Tree { steps: nodes, .. } => steps(nodes, out),
                Block::Rule => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

impl Page {
    /// Every inline of the page, in page order.
    pub fn inlines(&self) -> Vec<&Inline> {
        inlines(&self.blocks)
    }

    /// The diagram with block id `id`.
    pub fn diagram(&self, id: &str) -> Option<&Diagram> {
        self.diagrams.iter().find(|diagram| diagram.id == id)
    }

    /// The design steps by id, for placing `tree-node` comments.
    pub fn steps(&self) -> HashMap<String, &StepNode> {
        fn walk<'a>(nodes: &'a [StepNode], out: &mut HashMap<String, &'a StepNode>) {
            for node in nodes {
                out.entry(node.id.clone()).or_insert(node);
                walk(&node.children, out);
            }
        }
        let mut out = HashMap::new();
        for block in &self.blocks {
            if let Block::Section { blocks, .. } = block {
                for block in blocks {
                    if let Block::Tree { steps, .. } = block {
                        walk(steps, &mut out);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_text_keeps_its_words_and_marks_its_styles() {
        let inline = step_inline("Retry `?` the **charge** in `jobs` — it *may* fail");
        assert_eq!(inline.text, "Retry ? the charge in jobs — it may fail");
        let styles: Vec<(&str, &Style)> = inline
            .spans
            .iter()
            .map(|span| (inline.text.get(span.range.clone()).unwrap_or(""), &span.style))
            .collect();
        assert_eq!(styles[0].0, "?");
        assert_eq!(styles[0].1.badge, Some(Badge::Question));
        assert_eq!(styles[1], ("charge", &Style { bold: true, ..Style::default() }));
        assert_eq!(styles[2], ("jobs", &Style { code: true, ..Style::default() }));
        assert_eq!(styles[3].0, "— it ");
        assert!(styles[3].1.reason);
        assert_eq!(styles[4], ("may", &Style { italic: true, reason: true, ..Style::default() }));
    }

    #[test]
    fn inline_starts_point_at_their_text() {
        let page = render("# A\n\nsome *text* here\n\n- x\n- y\n", "a.md");
        for inline in page.inlines() {
            let units: Vec<u16> = page.text.encode_utf16().collect();
            let on_page = String::from_utf16_lossy(units.get(inline.start..inline.end()).unwrap_or_default());
            assert_eq!(on_page, inline.text);
        }
    }
}
