//! Host-supplied reader style and font-independent metrics.

/// A device-independent RGBA color for display-list decorations.
///
/// The renderer maps this value to the color model of its draw target.  The
/// layout crate therefore does not choose a framebuffer pixel format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Color {
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 255,
        }
    }

    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

/// Paint used to fill a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FillStyle {
    pub color: Color,
}

impl FillStyle {
    pub const fn new(color: Color) -> Self {
        Self { color }
    }
}

/// Stroke used for borders and rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BorderStyle {
    pub color: Color,
    pub width: u32,
}

impl BorderStyle {
    pub const fn new(color: Color, width: u32) -> Self {
        Self { color, width }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insets {
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
    pub left: u32,
}

impl Insets {
    pub const fn all(value: u32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextStyle {
    pub font_size: u32,
    pub line_height: u32,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
    /// Grayscale ink value: 0 is black and 255 is white.
    pub ink: u8,
}

impl TextStyle {
    pub const fn new(font_size: u32, line_height: u32) -> Self {
        Self {
            font_size,
            line_height,
            bold: false,
            italic: false,
            code: false,
            strikethrough: false,
            ink: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderStyle {
    pub body: TextStyle,
    pub heading: TextStyle,
    pub code: TextStyle,
    pub page_padding: Insets,
    pub paragraph_spacing: u32,
    /// Space before and after a heading, in page-space units.
    pub heading_spacing_before: u32,
    pub heading_spacing_after: u32,
    pub block_quote_indent: u32,
    pub block_quote_border: BorderStyle,
    pub block_quote_padding: u32,
    pub list_indent: u32,
    pub list_item_spacing: u32,
    pub inline_code_background: FillStyle,
    pub code_background: FillStyle,
    /// A slightly darker fill marks display lines created by wrapping a
    /// source line, while source-newline lines retain `code_background`.
    pub code_continuation_background: FillStyle,
    pub thematic_break: BorderStyle,
    /// Horizontal inset applied to each table cell around its text.
    pub table_cell_padding: u32,
    /// Vertical inset applied to each displayed line in a table cell.
    ///
    /// Table borders frame each displayed line. This inset keeps glyph bounds
    /// away from those borders, including when a cell wraps.
    pub table_cell_vertical_padding: u32,
    /// Smallest table font size used by compact and grouped fallback modes.
    pub table_min_font_size: u32,
    /// Smallest table line height used by compact and grouped fallback modes.
    pub table_min_line_height: u32,
    /// Monochrome cell border used for table framing and row separators.
    pub table_border: BorderStyle,
    /// A heavier rule distinguishes the header without relying on colour.
    pub table_header_border: BorderStyle,
    /// Optional light fill behind headers; bold text and the heavier rule are
    /// the primary header distinction for monochrome displays.
    pub table_header_fill: FillStyle,
}

impl Default for ReaderStyle {
    fn default() -> Self {
        Self {
            body: TextStyle::new(16, 22),
            heading: TextStyle {
                bold: true,
                ..TextStyle::new(22, 28)
            },
            code: TextStyle {
                font_size: 14,
                line_height: 20,
                code: true,
                ..TextStyle::new(14, 20)
            },
            page_padding: Insets {
                top: 16,
                right: 16,
                bottom: 16,
                left: 16,
            },
            paragraph_spacing: 12,
            heading_spacing_before: 20,
            heading_spacing_after: 8,
            block_quote_indent: 24,
            block_quote_border: BorderStyle::new(Color::rgb(150, 150, 150), 2),
            block_quote_padding: 8,
            list_indent: 24,
            list_item_spacing: 4,
            inline_code_background: FillStyle::new(Color::rgb(242, 242, 242)),
            code_background: FillStyle::new(Color::rgb(248, 248, 248)),
            code_continuation_background: FillStyle::new(Color::rgb(238, 238, 238)),
            thematic_break: BorderStyle::new(Color::rgb(128, 128, 128), 1),
            table_cell_padding: 3,
            table_cell_vertical_padding: 3,
            table_min_font_size: 12,
            table_min_line_height: 16,
            table_border: BorderStyle::new(Color::BLACK, 1),
            table_header_border: BorderStyle::new(Color::BLACK, 2),
            table_header_fill: FillStyle::new(Color::rgb(238, 238, 238)),
        }
    }
}
