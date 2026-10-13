//! The part of egui's API Ferrite's Mermaid code draws with, recording what it draws instead of painting it: the
//! ported code keeps its shape, and the app draws the recorded [`Primitive`]s with its own toolkit (Floem).
//!
//! Not from Ferrite: written for Co-Review (EPL-2.0).

use std::cell::{Cell, RefCell};
use std::ops::{Add, Div, Mul, Sub};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

impl Pos2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Pos2) -> f32 {
        (self - other).length()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn normalized(self) -> Vec2 {
        let length = self.length();
        if length > 0.0 {
            self / length
        } else {
            self
        }
    }

    pub fn length_sq(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// The larger of each coordinate.
    pub fn max(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x.max(other.x), self.y.max(other.y))
    }
}

impl Sub for Pos2 {
    type Output = Vec2;
    fn sub(self, other: Pos2) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl Add<Vec2> for Pos2 {
    type Output = Pos2;
    fn add(self, other: Vec2) -> Pos2 {
        Pos2::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub<Vec2> for Pos2 {
    type Output = Pos2;
    fn sub(self, other: Vec2) -> Pos2 {
        Pos2::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, factor: f32) -> Vec2 {
        Vec2::new(self.x * factor, self.y * factor)
    }
}

impl Div<f32> for Vec2 {
    type Output = Vec2;
    fn div(self, divisor: f32) -> Vec2 {
        Vec2::new(self.x / divisor, self.y / divisor)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub min: Pos2,
    pub max: Pos2,
}

impl Rect {
    /// Contains nothing; the union with anything is that thing.
    pub const NOTHING: Rect = Rect {
        min: Pos2::new(f32::INFINITY, f32::INFINITY),
        max: Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
    };

    pub fn union(self, other: Rect) -> Rect {
        Rect {
            min: Pos2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Pos2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    pub fn from_min_size(min: Pos2, size: Vec2) -> Self {
        Self { min, max: min + size }
    }

    pub fn from_min_max(min: Pos2, max: Pos2) -> Self {
        Self { min, max }
    }

    pub fn from_center_size(center: Pos2, size: Vec2) -> Self {
        Self {
            min: center - size / 2.0,
            max: center + size / 2.0,
        }
    }

    pub fn center(&self) -> Pos2 {
        Pos2::new((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }

    pub fn left(&self) -> f32 {
        self.min.x
    }

    pub fn right(&self) -> f32 {
        self.max.x
    }

    pub fn top(&self) -> f32 {
        self.min.y
    }

    pub fn bottom(&self) -> f32 {
        self.max.y
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn left_top(&self) -> Pos2 {
        self.min
    }

    pub fn right_bottom(&self) -> Pos2 {
        self.max
    }

    pub fn left_bottom(&self) -> Pos2 {
        Pos2::new(self.min.x, self.max.y)
    }

    pub fn left_center(&self) -> Pos2 {
        Pos2::new(self.min.x, self.center().y)
    }

    pub fn contains(&self, point: Pos2) -> bool {
        self.min.x <= point.x && point.x <= self.max.x && self.min.y <= point.y && point.y <= self.max.y
    }

    pub fn expand(&self, amount: f32) -> Rect {
        Rect {
            min: self.min - Vec2::new(amount, amount),
            max: self.max + Vec2::new(amount, amount),
        }
    }

    pub fn shrink(&self, amount: f32) -> Rect {
        self.expand(-amount)
    }
}

/// An sRGB colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Color32(pub [u8; 4]);

impl Color32 {
    pub const TRANSPARENT: Color32 = Color32([0, 0, 0, 0]);
    pub const WHITE: Color32 = Color32([255, 255, 255, 255]);

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Color32([r, g, b, 255])
    }

    pub const fn from_rgba_unmultiplied(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color32([r, g, b, a])
    }

    pub fn r(&self) -> u8 {
        self.0[0]
    }

    pub fn g(&self) -> u8 {
        self.0[1]
    }

    pub fn b(&self) -> u8 {
        self.0[2]
    }

    pub fn a(&self) -> u8 {
        self.0[3]
    }

    /// Fades the colour (egui multiplies all premultiplied channels, which for straight alpha is the alpha).
    pub fn gamma_multiply(self, factor: f32) -> Color32 {
        let [r, g, b, a] = self.0;
        Color32([r, g, b, (f32::from(a) * factor.clamp(0.0, 1.0)).round() as u8])
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub color: Color32,
}

impl Stroke {
    pub const NONE: Stroke = Stroke {
        width: 0.0,
        color: Color32::TRANSPARENT,
    };

    pub fn new(width: f32, color: impl Into<Color32>) -> Self {
        Self {
            width,
            color: color.into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.color.a() == 0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CornerRadius(pub f32);

impl CornerRadius {
    pub const ZERO: CornerRadius = CornerRadius(0.0);

    pub fn same(radius: u8) -> Self {
        CornerRadius(f32::from(radius))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeKind {
    Inside,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontId {
    pub size: f32,
}

impl FontId {
    pub fn proportional(size: f32) -> Self {
        Self { size }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Min,
    Center,
    Max,
}

/// Where a text's anchor point is on the text: `(horizontal, vertical)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Align2(pub [Align; 2]);

impl Align2 {
    pub const LEFT_TOP: Align2 = Align2([Align::Min, Align::Min]);
    pub const LEFT_CENTER: Align2 = Align2([Align::Min, Align::Center]);
    pub const CENTER_CENTER: Align2 = Align2([Align::Center, Align::Center]);
    pub const CENTER_BOTTOM: Align2 = Align2([Align::Center, Align::Max]);
}

/// A shape egui's `Painter::add` takes.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    ConvexPolygon {
        points: Vec<Pos2>,
        fill: Color32,
        stroke: Stroke,
    },
}

impl Shape {
    pub fn convex_polygon(points: Vec<Pos2>, fill: impl Into<Color32>, stroke: impl Into<Stroke>) -> Shape {
        Shape::ConvexPolygon {
            points,
            fill: fill.into(),
            stroke: stroke.into(),
        }
    }
}

/// Something drawn, in the diagram's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    Line {
        from: Pos2,
        to: Pos2,
        stroke: Stroke,
    },
    Polygon {
        points: Vec<Pos2>,
        fill: Color32,
        stroke: Stroke,
    },
    Rect {
        rect: Rect,
        radius: f32,
        fill: Color32,
        stroke: Stroke,
    },
    Circle {
        center: Pos2,
        radius: f32,
        fill: Color32,
        stroke: Stroke,
    },
    /// Text whose `align` point is at `pos`.
    Text {
        pos: Pos2,
        align: Align2,
        text: String,
        size: f32,
        color: Color32,
        /// Wrapped at this width (centred lines), else one line.
        max_width: Option<f32>,
    },
}

/// What the reviewer can comment on in a diagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    /// A node: its id and label.
    Node { id: String, label: String },
    /// An edge: its id (`from→to`) and its index in the diagram's edges.
    Edge { id: String, index: usize },
}

/// Where a [`HitTarget`] is.
#[derive(Debug, Clone, PartialEq)]
pub enum HitArea {
    Rect(Rect),
    /// Within `tolerance` of the line through `points`.
    Path { points: Vec<Pos2>, tolerance: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub target: HitTarget,
    pub area: HitArea,
}

/// Records what is drawn on it, in order, and where the commentable parts are.
#[derive(Debug, Default)]
pub struct Painter {
    primitives: RefCell<Vec<Primitive>>,
    hits: RefCell<Vec<Hit>>,
    edges: Cell<usize>,
}

impl Painter {
    fn push(&self, primitive: Primitive) {
        self.primitives.borrow_mut().push(primitive);
    }

    pub fn line_segment(&self, [from, to]: [Pos2; 2], stroke: impl Into<Stroke>) {
        self.push(Primitive::Line {
            from,
            to,
            stroke: stroke.into(),
        });
    }

    pub fn add(&self, shape: Shape) {
        match shape {
            Shape::ConvexPolygon { points, fill, stroke } => self.push(Primitive::Polygon { points, fill, stroke }),
        }
    }

    pub fn rect(
        &self,
        rect: Rect,
        radius: impl Into<CornerRadius>,
        fill: impl Into<Color32>,
        stroke: impl Into<Stroke>,
        _kind: StrokeKind,
    ) {
        self.push(Primitive::Rect {
            rect,
            radius: radius.into().0,
            fill: fill.into(),
            stroke: stroke.into(),
        });
    }

    pub fn rect_filled(&self, rect: Rect, radius: impl Into<CornerRadius>, fill: impl Into<Color32>) {
        self.rect(rect, radius, fill, Stroke::NONE, StrokeKind::Inside);
    }

    pub fn circle(&self, center: Pos2, radius: f32, fill: impl Into<Color32>, stroke: impl Into<Stroke>) {
        self.push(Primitive::Circle {
            center,
            radius,
            fill: fill.into(),
            stroke: stroke.into(),
        });
    }

    pub fn circle_filled(&self, center: Pos2, radius: f32, fill: impl Into<Color32>) {
        self.circle(center, radius, fill, Stroke::NONE);
    }

    pub fn circle_stroke(&self, center: Pos2, radius: f32, stroke: impl Into<Stroke>) {
        self.circle(center, radius, Color32::TRANSPARENT, stroke);
    }

    pub fn text(&self, pos: Pos2, align: Align2, text: impl ToString, font: FontId, color: impl Into<Color32>) {
        self.text_inner(pos, align, text.to_string(), font, color.into(), None);
    }

    /// Text wrapped at `max_width`.
    pub fn text_wrapped(
        &self,
        pos: Pos2,
        align: Align2,
        text: impl ToString,
        font: FontId,
        color: impl Into<Color32>,
        max_width: f32,
    ) {
        self.text_inner(pos, align, text.to_string(), font, color.into(), Some(max_width));
    }

    fn text_inner(&self, pos: Pos2, align: Align2, text: String, font: FontId, color: Color32, max_width: Option<f32>) {
        self.push(Primitive::Text {
            pos,
            align,
            text,
            size: font.size,
            color,
            max_width,
        });
    }

    /// Marks where a commentable part of the diagram is (Co-Review's addition to the drawing code).
    pub fn hit(&self, target: HitTarget, area: HitArea) {
        self.hits.borrow_mut().push(Hit { target, area });
    }

    /// Numbers the edges (messages) as they are drawn.
    pub fn next_edge_index(&self) -> usize {
        let index = self.edges.get();
        self.edges.set(index + 1);
        index
    }

    pub fn finish(self) -> (Vec<Primitive>, Vec<Hit>) {
        (self.primitives.into_inner(), self.hits.into_inner())
    }
}
