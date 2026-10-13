//! Markdown review pages, headless: a document (a design, a bundle page, a Markdown file in a code review, a review's
//! overview page) as blocks to draw, and the page's text as text anchors see it.
//!
//! Ports the rendering half of the Theia app's document page (`browser/document/document-render.ts`, the parts of
//! `document-review-widget.tsx` that add text, and `text-anchor.ts`). The page's text is the text of the DOM the
//! Theia app renders, character for character, so anchors made here and there agree: it was recorded
//! from the TypeScript and `tests/conformance.rs` replays it.
//!
//! Offsets into the page's text are UTF-16 code units, as in JavaScript.

mod anchor;
mod links;
mod openspec;
mod page;
mod text;

pub use anchor::{locate, text_anchor};
pub use links::{LinkTarget, parse_link, resolve_link};
pub use openspec::{OpenSpecCard, openspec_cards};
pub use page::{Alignment, Badge, Block, Diagram, Image, Inline, Meta, Page, Span, StepNode, Style, render};
pub use text::{utf16_len, utf16_to_byte, byte_to_utf16};
