//! One task's output, kept in a terminal emulator.
//!
//! Task output arrives through a pipe, so each line feed is preceded by a
//! carriage return, the way a terminal's output processing would add one. Sequences
//! that clear the screen are replaced by a divider, so earlier output stays in
//! the scrollback.

use alacritty_terminal::{
    event::VoidListener,
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{
        Config, Term,
        cell::{Cell, Flags},
        search::{Match, RegexIter, RegexSearch},
        viewport_to_point,
    },
    vte::ansi::{Color as AnsiColor, NamedColor, Processor},
};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use vt_str::Str;

use crate::ansi::write_style;

const SCROLLBACK_LINES: usize = 10_000;
const DIVIDER: &[u8] = "\x1b[2m── screen cleared ──\x1b[0m\r\n".as_bytes();

#[derive(Debug, Clone, Copy)]
struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// An active search: the pattern typed so far and the match the view is on.
struct Search {
    pattern: Str,
    regex: Option<RegexSearch>,
    current: Option<Match>,
    /// Match count, and the output generation it was counted at.
    count: Option<(u64, usize, usize)>,
}

pub struct Emulator {
    term: Term<VoidListener>,
    parser: Processor,
    filter: ClearFilter,
    has_output: bool,
    /// Bumped on every write, so cached match counts know they're stale.
    generation: u64,
    search: Option<Search>,
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator").field("has_output", &self.has_output).finish_non_exhaustive()
    }
}

impl Emulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = GridSize { cols: usize::from(cols.max(1)), rows: usize::from(rows.max(1)) };
        let config = Config { scrolling_history: SCROLLBACK_LINES, ..Config::default() };
        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            filter: ClearFilter::default(),
            has_output: false,
            generation: 0,
            search: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.generation += 1;
        for piece in self.filter.split(bytes) {
            match piece {
                Piece::Bytes(bytes) => {
                    if !bytes.is_empty() {
                        self.has_output = true;
                        self.parser.advance(&mut self.term, &bytes);
                    }
                }
                Piece::Clear => {
                    if !self.has_output {
                        continue;
                    }
                    if self.term.grid().cursor.point.column.0 != 0 {
                        self.parser.advance(&mut self.term, b"\r\n");
                    }
                    self.parser.advance(&mut self.term, DIVIDER);
                }
            }
        }
    }

    /// Shows a line the user sent to the task's stdin, as a terminal's echo would.
    pub fn echo(&mut self, line: &str) {
        self.generation += 1;
        self.has_output = true;
        self.parser.advance(&mut self.term, b"\x1b[1;36m");
        self.parser.advance(&mut self.term, line.as_bytes());
        self.parser.advance(&mut self.term, b"\x1b[0m\r\n");
    }

    pub const fn has_output(&self) -> bool {
        self.has_output
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term
            .resize(GridSize { cols: usize::from(cols.max(1)), rows: usize::from(rows.max(1)) });
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.term.scroll_display(scroll);
    }

    /// The cursor's column and row on screen, where typed input shows up.
    /// `None` while the view is scrolled back.
    pub fn cursor(&self) -> Option<(u16, u16)> {
        if self.term.grid().display_offset() != 0 {
            return None;
        }
        let point = self.term.grid().cursor.point;
        Some((u16::try_from(point.column.0).unwrap_or(u16::MAX), u16::try_from(point.line.0).ok()?))
    }

    /// How many lines the view is scrolled up from the newest output.
    pub fn scrolled_back(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn viewport_point(&self, col: u16, row: u16) -> Point {
        let row = usize::from(row).min(self.term.screen_lines().saturating_sub(1));
        let col = usize::from(col).min(self.term.columns().saturating_sub(1));
        viewport_to_point(self.term.grid().display_offset(), Point::new(row, Column(col)))
    }

    pub fn select_begin(&mut self, col: u16, row: u16) {
        let point = self.viewport_point(col, row);
        self.term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
    }

    pub fn select_extend(&mut self, col: u16, row: u16) {
        let point = self.viewport_point(col, row);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, Side::Right);
        }
    }

    /// The selected text, if any. The selection stays visible.
    pub fn selected_text(&self) -> Option<Str> {
        self.term.selection_to_string().filter(|text| !text.is_empty()).map(Str::from)
    }

    pub const fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub const fn has_selection(&self) -> bool {
        self.term.selection.is_some()
    }

    /// Starts or updates a search, jumping to the newest match above the
    /// bottom of the view. Returns whether anything matched.
    pub fn search(&mut self, pattern: &str) -> bool {
        let regex = if pattern.is_empty() { None } else { RegexSearch::new(&escape(pattern)).ok() };
        self.search =
            Some(Search { pattern: Str::from(pattern), regex, current: None, count: None });
        let rows = self.term.screen_lines();
        let offset = self.term.grid().display_offset();
        let bottom =
            Line(i32::try_from(rows).unwrap_or(i32::MAX) - 1 - i32::try_from(offset).unwrap_or(0));
        let origin = Point::new(bottom, self.term.last_column());
        self.jump(origin, Direction::Left)
    }

    /// Moves to the next older (`Direction::Left`) or newer match.
    pub fn search_step(&mut self, direction: Direction) -> bool {
        let Some(current) = self.search.as_ref().and_then(|search| search.current.clone()) else {
            return false;
        };
        let origin = match direction {
            Direction::Left => current.start().sub(&self.term, Boundary::None, 1),
            Direction::Right => current.end().add(&self.term, Boundary::None, 1),
        };
        self.jump(origin, direction)
    }

    fn jump(&mut self, origin: Point, direction: Direction) -> bool {
        let Some(search) = &mut self.search else { return false };
        let Some(regex) = &mut search.regex else {
            search.current = None;
            return false;
        };
        let found = self.term.search_next(regex, origin, direction, Side::Left, None);
        search.current.clone_from(&found);
        search.count = None;
        if let Some(found) = found {
            self.term.scroll_to_point(*found.start());
            true
        } else {
            false
        }
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    pub fn search_pattern(&self) -> Option<&str> {
        self.search.as_ref().map(|search| search.pattern.as_str())
    }

    /// The position of the current match among all matches, counting from the
    /// oldest, and how many there are.
    pub fn search_position(&mut self) -> Option<(usize, usize)> {
        let generation = self.generation;
        let search = self.search.as_mut()?;
        let regex = search.regex.as_mut()?;
        let current_start = search.current.as_ref().map(|current| *current.start());
        if let Some((counted_at, position, count)) = search.count
            && counted_at == generation
        {
            return Some((position, count));
        }
        let start = Point::new(self.term.topmost_line(), Column(0));
        let end = Point::new(self.term.bottommost_line(), self.term.last_column());
        let mut position = 0;
        let mut count = 0;
        for found in RegexIter::new(start, end, Direction::Right, &self.term, regex) {
            count += 1;
            if Some(*found.start()) == current_start {
                position = count;
            }
        }
        search.count = Some((generation, position, count));
        Some((position, count))
    }

    /// Draws the visible rows into `area`.
    pub fn render(&mut self, buffer: &mut Buffer, area: Rect) {
        let rows = usize::from(area.height).min(self.term.screen_lines());
        let cols = usize::from(area.width).min(self.term.columns());
        let offset = self.term.grid().display_offset();
        let top = Line(-i32::try_from(offset).unwrap_or(0));
        let selection =
            self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term));

        let mut matches: Vec<Match> = Vec::new();
        let mut current: Option<Match> = None;
        if let Some(search) = &mut self.search
            && let Some(regex) = &mut search.regex
            && rows > 0
        {
            let bottom = top + (rows - 1);
            let start = Point::new(top, Column(0));
            let end = Point::new(bottom, self.term.last_column());
            matches.extend(RegexIter::new(start, end, Direction::Right, &self.term, regex));
            current.clone_from(&search.current);
        }

        let grid = self.term.grid();
        for y in 0..rows {
            let line = top + y;
            let row = &grid[line];
            for x in 0..cols {
                let cell = &row[Column(x)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let point = Point::new(line, Column(x));
                let mut style = cell_style(cell);
                if let Some(range) = &selection
                    && range.contains(point)
                {
                    style = style.fg(Color::Black).bg(Color::Indexed(153));
                } else if current.as_ref().is_some_and(|current| current.contains(&point)) {
                    style =
                        style.fg(Color::Black).bg(Color::Indexed(214)).add_modifier(Modifier::BOLD);
                } else if matches.iter().any(|found| found.contains(&point)) {
                    style = style.fg(Color::Black).bg(Color::Indexed(222));
                }
                let target = &mut buffer[(area.x + to_u16(x), area.y + to_u16(y))];
                target.set_style(style);
                set_cell_symbol(target, cell);
            }
        }
    }

    /// Writes all output as text, oldest line first, each line ending in
    /// CR LF. Rows that wrapped are joined, so the terminal wraps them again.
    /// Without `styled`, colors and attributes are left out.
    pub fn write_text(&self, out: &mut Vec<u8>, styled: bool) {
        let grid = self.term.grid();
        let top = self.term.topmost_line();
        let mut bottom = self.term.bottommost_line();
        while bottom >= top && is_blank_row(&grid[bottom]) {
            bottom -= 1;
        }
        let mut line = top;
        let mut current = Style::new();
        let mut pending_newline = false;
        while line <= bottom {
            let row = &grid[line];
            let last = row.len().saturating_sub(1);
            let wrapped = row[Column(last)].flags.contains(Flags::WRAPLINE);
            if pending_newline {
                if current != Style::new() {
                    out.extend_from_slice(b"\x1b[0m");
                    current = Style::new();
                }
                out.extend_from_slice(b"\r\n");
            }
            let end = if wrapped { row.len() } else { content_end(row) };
            for x in 0..end {
                let cell = &row[Column(x)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                let style = if styled { cell_style(cell) } else { Style::new() };
                if style != current {
                    write_style(out, style);
                    current = style;
                }
                let mut utf8 = [0; 4];
                out.extend_from_slice(cell_char(cell).encode_utf8(&mut utf8).as_bytes());
                for &zerowidth in cell.zerowidth().unwrap_or_default() {
                    out.extend_from_slice(zerowidth.encode_utf8(&mut utf8).as_bytes());
                }
            }
            pending_newline = !wrapped;
            line += 1;
        }
        if current != Style::new() {
            out.extend_from_slice(b"\x1b[0m");
        }
        if line > top {
            out.extend_from_slice(b"\r\n");
        }
    }
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// The character to draw for a cell. The emulator keeps a tab in the cell it
/// started from, with spaces after it; drawn as is, it would move the
/// terminal's cursor a second time.
const fn cell_char(cell: &Cell) -> char {
    if cell.flags.contains(Flags::HIDDEN) || matches!(cell.c, '\0' | '\t') { ' ' } else { cell.c }
}

fn set_cell_symbol(target: &mut ratatui_core::buffer::Cell, cell: &Cell) {
    let mut utf8 = [0; 4];
    let c = cell_char(cell);
    match cell.zerowidth() {
        Some(zerowidth) if !zerowidth.is_empty() => {
            let mut symbol = Str::from(&*c.encode_utf8(&mut utf8));
            for &extra in zerowidth {
                symbol.push(extra);
            }
            target.set_symbol(&symbol);
        }
        _ => {
            target.set_symbol(c.encode_utf8(&mut utf8));
        }
    }
}

fn is_blank_cell(cell: &Cell) -> bool {
    cell_char(cell) == ' '
        && cell.bg == AnsiColor::Named(NamedColor::Background)
        && !cell.flags.contains(Flags::INVERSE)
}

fn is_blank_row(row: &alacritty_terminal::grid::Row<Cell>) -> bool {
    (0..row.len()).all(|x| is_blank_cell(&row[Column(x)]))
}

fn content_end(row: &alacritty_terminal::grid::Row<Cell>) -> usize {
    let mut end = row.len();
    while end > 0 && is_blank_cell(&row[Column(end - 1)]) {
        end -= 1;
    }
    end
}

fn cell_style(cell: &Cell) -> Style {
    let mut style = Style::new();
    let fg = map_color(cell.fg);
    if fg != Color::Reset {
        style = style.fg(fg);
    }
    let bg = map_color(cell.bg);
    if bg != Color::Reset {
        style = style.bg(bg);
    }
    for (flag, modifier) in [
        (Flags::BOLD, Modifier::BOLD),
        (Flags::DIM, Modifier::DIM),
        (Flags::ITALIC, Modifier::ITALIC),
        (Flags::UNDERLINE, Modifier::UNDERLINED),
        (Flags::INVERSE, Modifier::REVERSED),
        (Flags::STRIKEOUT, Modifier::CROSSED_OUT),
    ] {
        if cell.flags.contains(flag) {
            style = style.add_modifier(modifier);
        }
    }
    style
}

const fn map_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Named(named) => match named {
            NamedColor::Black | NamedColor::DimBlack => Color::Black,
            NamedColor::Red | NamedColor::DimRed => Color::Red,
            NamedColor::Green | NamedColor::DimGreen => Color::Green,
            NamedColor::Yellow | NamedColor::DimYellow => Color::Yellow,
            NamedColor::Blue | NamedColor::DimBlue => Color::Blue,
            NamedColor::Magenta | NamedColor::DimMagenta => Color::Magenta,
            NamedColor::Cyan | NamedColor::DimCyan => Color::Cyan,
            NamedColor::White | NamedColor::DimWhite => Color::Gray,
            NamedColor::BrightBlack => Color::DarkGray,
            NamedColor::BrightRed => Color::LightRed,
            NamedColor::BrightGreen => Color::LightGreen,
            NamedColor::BrightYellow => Color::LightYellow,
            NamedColor::BrightBlue => Color::LightBlue,
            NamedColor::BrightMagenta => Color::LightMagenta,
            NamedColor::BrightCyan => Color::LightCyan,
            NamedColor::BrightWhite => Color::White,
            NamedColor::Foreground
            | NamedColor::Background
            | NamedColor::Cursor
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => Color::Reset,
        },
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(index) => Color::Indexed(index),
    }
}

/// Escapes regex syntax, so searches match the typed text literally.
fn escape(pattern: &str) -> Str {
    let mut escaped = Str::with_capacity(pattern.len());
    for c in pattern.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

enum Piece {
    Bytes(Vec<u8>),
    Clear,
}

/// Finds the sequences that clear the screen, carrying an incomplete escape
/// sequence over to the next write.
#[derive(Debug, Default)]
struct ClearFilter {
    pending: Vec<u8>,
    /// Set right after a clear, so the cursor-home that follows is dropped too.
    after_clear: bool,
}

impl ClearFilter {
    fn split(&mut self, input: &[u8]) -> Vec<Piece> {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(input);
        let mut pieces = Vec::new();
        let mut text = Vec::new();
        let mut index = 0;
        while index < data.len() {
            if data[index] != 0x1b {
                // A line feed also returns the carriage, as a terminal's
                // output processing does for programs that write to it.
                if data[index] == b'\n' {
                    text.push(b'\r');
                }
                text.push(data[index]);
                self.after_clear = false;
                index += 1;
                continue;
            }
            let Some(len) = sequence_len(&data[index..]) else {
                self.pending = data[index..].to_vec();
                break;
            };
            let sequence = &data[index..index + len];
            if matches!(sequence, b"\x1b[2J" | b"\x1b[3J" | b"\x1bc") {
                if !text.is_empty() {
                    pieces.push(Piece::Bytes(std::mem::take(&mut text)));
                }
                if !self.after_clear {
                    pieces.push(Piece::Clear);
                }
                self.after_clear = true;
            } else if self.after_clear
                && matches!(sequence, b"\x1b[H" | b"\x1b[1;1H" | b"\x1b[0;0H")
            {
                // Cursor home right after a clear: it would overwrite old output.
            } else {
                text.extend_from_slice(sequence);
                self.after_clear = false;
            }
            index += len;
        }
        if !text.is_empty() {
            pieces.push(Piece::Bytes(text));
        }
        pieces
    }
}

/// The length of the escape sequence at the start of `data`, or `None` if it
/// isn't complete yet. Only CSI sequences are measured; for anything else the
/// escape and the next byte are passed on, and the parser sees the rest as is.
fn sequence_len(data: &[u8]) -> Option<usize> {
    let &second = data.get(1)?;
    if second != b'[' {
        return Some(2);
    }
    for (index, &byte) in data.iter().enumerate().skip(2) {
        if (0x40..=0x7e).contains(&byte) {
            return Some(index + 1);
        }
        if !(0x20..=0x3f).contains(&byte) {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(emulator: &Emulator) -> Vec<u8> {
        let mut out = Vec::new();
        emulator.write_text(&mut out, true);
        out
    }

    #[test]
    fn line_feeds_start_new_lines() {
        let mut emulator = Emulator::new(20, 5);
        emulator.feed(b"one\ntwo\n");
        assert_eq!(text(&emulator), b"one\r\ntwo\r\n");
    }

    #[test]
    fn clearing_the_screen_keeps_earlier_output() {
        let mut emulator = Emulator::new(30, 3);
        emulator.feed(b"\x1b[2J\x1b[3J\x1b[Hfirst run\n");
        emulator.feed(b"\x1b[2J\x1b[3");
        emulator.feed(b"J\x1b[Hsecond run\n");
        assert_eq!(
            text(&emulator),
            "first run\r\n\x1b[0;2m── screen cleared ──\x1b[0m\r\nsecond run\r\n".as_bytes()
        );
    }

    #[test]
    fn wrapped_rows_are_joined() {
        let mut emulator = Emulator::new(4, 3);
        emulator.feed(b"abcdefgh\nij\n");
        assert_eq!(text(&emulator), b"abcdefgh\r\nij\r\n");
    }

    #[test]
    fn search_finds_the_newest_match_first() {
        let mut emulator = Emulator::new(20, 2);
        emulator.feed(b"error one\nok\nerror two\nok\n");
        assert!(emulator.search("error"));
        assert_eq!(emulator.search_position(), Some((2, 2)));
        assert!(emulator.search_step(Direction::Left));
        assert_eq!(emulator.search_position(), Some((1, 2)));
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn output_longer_than_the_screen() {
        let mut emulator = Emulator::new(20, 5);
        let mut input = Vec::new();
        let mut expected = Vec::new();
        for index in 0..12 {
            input.extend_from_slice(vt_str::format!("line {index}\n").as_bytes());
            expected.extend_from_slice(vt_str::format!("line {index}\r\n").as_bytes());
        }
        emulator.feed(&input);
        let mut out = Vec::new();
        emulator.write_text(&mut out, true);
        assert_eq!(std::str::from_utf8(&out).unwrap(), std::str::from_utf8(&expected).unwrap());
    }

    #[test]
    fn tabs_become_spaces() {
        let mut emulator = Emulator::new(20, 3);
        emulator.feed(b"{\n\tDB;\n}\n");
        let mut out = Vec::new();
        emulator.write_text(&mut out, true);
        assert_eq!(std::str::from_utf8(&out).unwrap(), "{\r\n        DB;\r\n}\r\n");
    }

    #[test]
    fn last_line_after_blank_line() {
        let mut emulator = Emulator::new(100, 29);
        emulator.feed(b"a\n\nFound 2 warnings.\nFinished in 212ms.\n");
        let mut out = Vec::new();
        emulator.write_text(&mut out, true);
        assert_eq!(out, b"a\r\n\r\nFound 2 warnings.\r\nFinished in 212ms.\r\n");
    }
}
