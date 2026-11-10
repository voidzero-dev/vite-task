//! Turning ratatui buffers into the bytes that draw them.
//!
//! The tree is drawn in the primary screen at the cursor, so it can only move
//! the cursor relative to where it left it. The task view owns the alternate
//! screen and positions rows absolutely. Both redraw only the rows that changed.

use std::io::Write as _;

use ratatui_core::{
    buffer::Buffer,
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr as _;

/// Begins a synchronized update, so the terminal shows a frame all at once.
pub const SYNC_BEGIN: &[u8] = b"\x1b[?2026h";
pub const SYNC_END: &[u8] = b"\x1b[?2026l";
pub const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
pub const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
pub const ENTER_ALT_SCREEN: &[u8] = b"\x1b[?1049h";
pub const LEAVE_ALT_SCREEN: &[u8] = b"\x1b[?1049l";
/// Button presses, drags and the wheel, reported in the SGR encoding.
pub const ENABLE_MOUSE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
pub const DISABLE_MOUSE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";

/// Writes the SGR sequence that switches from the default style to `style`.
pub fn write_style(out: &mut Vec<u8>, style: Style) {
    out.extend_from_slice(b"\x1b[0");
    let modifier = style.add_modifier;
    for (flag, code) in [
        (Modifier::BOLD, 1),
        (Modifier::DIM, 2),
        (Modifier::ITALIC, 3),
        (Modifier::UNDERLINED, 4),
        (Modifier::SLOW_BLINK, 5),
        (Modifier::REVERSED, 7),
        (Modifier::HIDDEN, 8),
        (Modifier::CROSSED_OUT, 9),
    ] {
        if modifier.contains(flag) {
            let _ = write!(out, ";{code}");
        }
    }
    if let Some(fg) = style.fg {
        write_color(out, fg, false);
    }
    if let Some(bg) = style.bg {
        write_color(out, bg, true);
    }
    out.push(b'm');
}

fn write_color(out: &mut Vec<u8>, color: Color, background: bool) {
    let offset = if background { 10 } else { 0 };
    let base = match color {
        Color::Reset => return,
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::Gray => 37,
        Color::DarkGray => 90,
        Color::LightRed => 91,
        Color::LightGreen => 92,
        Color::LightYellow => 93,
        Color::LightBlue => 94,
        Color::LightMagenta => 95,
        Color::LightCyan => 96,
        Color::White => 97,
        Color::Indexed(index) => {
            let _ = write!(out, ";{};5;{index}", 38 + offset);
            return;
        }
        Color::Rgb(r, g, b) => {
            let _ = write!(out, ";{};2;{r};{g};{b}", 38 + offset);
            return;
        }
    };
    let _ = write!(out, ";{}", base + offset);
}

/// The style of a buffer cell, with unset colors left unset.
fn cell_style(cell: &ratatui_core::buffer::Cell) -> Style {
    let mut style = Style::new().add_modifier(cell.modifier);
    if cell.fg != Color::Reset {
        style = style.fg(cell.fg);
    }
    if cell.bg != Color::Reset {
        style = style.bg(cell.bg);
    }
    style
}

/// Encodes one row of `buffer`, dropping trailing blank cells. The bytes end
/// with the default style. Without `styled`, colors and attributes are left out.
pub fn encode_row(buffer: &Buffer, y: u16, styled: bool) -> Vec<u8> {
    let area = buffer.area;
    // Find the last cell worth writing: anything but a plain space.
    let mut end = area.width;
    while end > 0 {
        let cell = &buffer[(area.x + end - 1, y)];
        if cell.symbol() != " "
            || cell.bg != Color::Reset
            || cell.modifier.contains(Modifier::REVERSED)
        {
            break;
        }
        end -= 1;
    }
    let mut out = Vec::new();
    let mut current = Style::new();
    let mut x = 0;
    while x < end {
        let cell = &buffer[(area.x + x, y)];
        let style = if styled { cell_style(cell) } else { Style::new() };
        if style != current {
            write_style(&mut out, style);
            current = style;
        }
        let symbol = cell.symbol();
        out.extend_from_slice(symbol.as_bytes());
        let width = u16::try_from(symbol.width()).unwrap_or(1).max(1);
        x += width;
    }
    if current != Style::new() {
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}

/// Encodes every row of `buffer`.
pub fn encode_rows(buffer: &Buffer, styled: bool) -> Vec<Vec<u8>> {
    (buffer.area.y..buffer.area.bottom()).map(|y| encode_row(buffer, y, styled)).collect()
}

/// Removes SGR sequences (colors and attributes) from text.
pub fn strip_styles(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if text[index] == 0x1b && text.get(index + 1) == Some(&b'[') {
            let params = text[index + 2..]
                .iter()
                .take_while(|byte| matches!(byte, b'0'..=b'9' | b';' | b':'));
            let end = index + 2 + params.count();
            if text.get(end) == Some(&b'm') {
                index = end + 1;
                continue;
            }
        }
        out.push(text[index]);
        index += 1;
    }
    out
}

/// Draws a block of rows in the primary screen, starting at the line the
/// cursor was on when the first frame was drawn.
///
/// After each frame, the cursor stays on the block's last row. A block never
/// gets taller than the screen, so moving up always reaches its first row.
#[derive(Debug, Default)]
pub struct InlineRenderer {
    rows: Vec<Vec<u8>>,
}

impl InlineRenderer {
    pub fn draw(&mut self, rows: Vec<Vec<u8>>, out: &mut Vec<u8>) {
        if rows == self.rows {
            return;
        }
        let old_height = self.rows.len();
        if old_height == 0 {
            // The screen below the cursor may hold old output; clear it first.
            out.extend_from_slice(b"\r\x1b[J");
            for (index, row) in rows.iter().enumerate() {
                if index > 0 {
                    out.extend_from_slice(b"\r\n");
                }
                out.extend_from_slice(row);
            }
            self.rows = rows;
            return;
        }
        if old_height > 1 {
            let _ = write!(out, "\x1b[{}A", old_height - 1);
        }
        out.push(b'\r');
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                out.extend_from_slice(b"\r\n");
            }
            if index >= old_height || self.rows[index] != *row {
                out.extend_from_slice(b"\x1b[2K");
                out.extend_from_slice(row);
            }
        }
        if rows.len() < old_height {
            // Clear the rows the block no longer has, then step back up.
            out.extend_from_slice(b"\r\n\x1b[J\x1b[1A");
        }
        self.rows = rows;
    }

    /// Erases the block and forgets it, leaving the cursor where it started.
    pub fn clear(&mut self, out: &mut Vec<u8>) {
        let height = self.rows.len();
        if height == 0 {
            return;
        }
        if height > 1 {
            let _ = write!(out, "\x1b[{}A", height - 1);
        }
        out.extend_from_slice(b"\r\x1b[J");
        self.rows.clear();
    }

    pub const fn is_drawn(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Forgets the block, so the next frame starts below it.
    pub fn detach(&mut self) {
        self.rows.clear();
    }
}

/// Draws full-screen frames in the alternate screen.
#[derive(Debug, Default)]
pub struct ScreenRenderer {
    rows: Vec<Vec<u8>>,
}

impl ScreenRenderer {
    pub fn draw(&mut self, rows: Vec<Vec<u8>>, out: &mut Vec<u8>) {
        for (index, row) in rows.iter().enumerate() {
            if self.rows.get(index) != Some(row) {
                let _ = write!(out, "\x1b[{};1H\x1b[2K", index + 1);
                out.extend_from_slice(row);
            }
        }
        self.rows = rows;
    }

    /// Forgets what's on screen, so the next frame redraws every row.
    pub fn invalidate(&mut self) {
        self.rows.clear();
    }
}
