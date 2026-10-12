pub fn hello() {}
pub struct Point { x: f64, y: f64 }
impl Point {
    pub fn new(x: f64, y: f64) -> Self { Self { x, y } }
    pub fn distance(&self) -> f64 { (self.x * self.x + self.y * self.y).sqrt() }
}
pub enum Color { Red, Green, Blue }
pub trait Drawable { fn draw(&self); }
