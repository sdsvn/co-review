//! Text measurement for the layout: the app supplies a measurer; tests estimate.

/// Result of measuring text dimensions.
#[derive(Debug, Clone, Copy)]
pub struct TextSize {
    pub width: f32,
    pub height: f32,
}

impl TextSize {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

pub trait TextMeasurer {
    /// Measure the dimensions of text with the given font size.
    fn measure(&self, text: &str, font_size: f32) -> TextSize;

    /// Measure text with wrapping at max_width. Returns size of wrapped text.
    fn measure_wrapped(&self, text: &str, font_size: f32, max_width: f32) -> TextSize {
        let single_line = self.measure(text, font_size);
        if single_line.width <= max_width || max_width <= 0.0 {
            return single_line;
        }

        // Estimate wrapped height based on number of lines needed
        let lines_needed = (single_line.width / max_width).ceil();
        TextSize::new(max_width, single_line.height * lines_needed)
    }

    /// Truncate text to fit within max_width, adding ellipsis if needed.
    fn truncate_with_ellipsis(&self, text: &str, font_size: f32, max_width: f32) -> String {
        let size = self.measure(text, font_size);
        if size.width <= max_width || max_width <= 0.0 {
            return text.to_string();
        }

        let ellipsis = "…";
        let ellipsis_width = self.measure(ellipsis, font_size).width;
        let available_width = max_width - ellipsis_width;

        if available_width <= 0.0 {
            return ellipsis.to_string();
        }

        // Binary search for the right truncation point
        let chars: Vec<char> = text.chars().collect();
        let mut low = 0;
        let mut high = chars.len();

        while low < high {
            let mid = (low + high + 1) / 2;
            let truncated: String = chars[..mid].iter().collect();
            let width = self.measure(&truncated, font_size).width;

            if width <= available_width {
                low = mid;
            } else {
                high = mid - 1;
            }
        }

        if low == 0 {
            ellipsis.to_string()
        } else {
            let truncated: String = chars[..low].iter().collect();
            format!("{}{}", truncated, ellipsis)
        }
    }
}

/// Text measured by its length in bytes: each byte 0.55 of the font size wide.
#[derive(Debug, Clone, Copy, Default)]
pub struct EstimatedTextMeasurer;

impl TextMeasurer for EstimatedTextMeasurer {
    fn measure(&self, text: &str, font_size: f32) -> TextSize {
        TextSize::new(text.len() as f32 * font_size * 0.55, font_size)
    }
}
