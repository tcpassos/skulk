//! A generic scrollable text viewer — pure state, no `embedded-graphics`,
//! same split as [`crate::menu`]/[`crate::form`]. Loot content is the first
//! caller; any future "show me some text" screen (full task output, a log
//! tail, help text) can reuse it without a new screen type.

/// How many lines a "page" (Left/Right) jumps, vs. Up/Down's one line at a
/// time. Not tied to the real visible line count (the renderer's font size
/// varies by theme/screen) — just a fixed, reasonable jump.
const PAGE_LINES: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct TextView {
    title: String,
    lines: Vec<String>,
    scroll: usize,
}

impl TextView {
    /// Build a view from raw bytes: best-effort UTF-8 decode, split into
    /// lines. Anything that isn't (mostly) printable text becomes a single
    /// explanatory line instead of failing — there's always something to
    /// show, never a missing screen.
    pub fn from_bytes(title: impl Into<String>, bytes: &[u8]) -> Self {
        let lines = match std::str::from_utf8(bytes) {
            Ok(text) if !text.is_empty() => text.lines().map(str::to_string).collect(),
            Ok(_) => vec!["(empty)".to_string()],
            Err(_) => vec![format!("(binary content, {} bytes -- not previewable)", bytes.len())],
        };
        Self { title: title.into(), lines, scroll: 0 }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// First visible line index, for the renderer to slice from.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn line_down(&mut self) {
        if self.scroll + 1 < self.lines.len() {
            self.scroll += 1;
        }
    }

    pub fn line_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn page_down(&mut self) {
        self.scroll = (self.scroll + PAGE_LINES).min(self.lines.len().saturating_sub(1));
    }

    pub fn page_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(PAGE_LINES);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_valid_utf8_into_lines() {
        let tv = TextView::from_bytes("k", b"one\ntwo\nthree");
        assert_eq!(tv.lines(), &["one", "two", "three"]);
        assert_eq!(tv.title(), "k");
    }

    #[test]
    fn empty_bytes_show_a_placeholder_line() {
        let tv = TextView::from_bytes("k", b"");
        assert_eq!(tv.lines(), &["(empty)"]);
    }

    #[test]
    fn non_utf8_bytes_show_a_binary_placeholder_instead_of_failing() {
        let tv = TextView::from_bytes("k", &[0xff, 0xfe, 0x00, 0x01]);
        assert_eq!(tv.lines().len(), 1);
        assert!(tv.lines()[0].contains("binary content"));
        assert!(tv.lines()[0].contains("4 bytes"));
    }

    #[test]
    fn line_scroll_clamps_at_both_ends() {
        let mut tv = TextView::from_bytes("k", b"a\nb\nc");
        assert_eq!(tv.scroll(), 0);
        tv.line_up(); // already at 0
        assert_eq!(tv.scroll(), 0);
        tv.line_down();
        tv.line_down();
        assert_eq!(tv.scroll(), 2);
        tv.line_down(); // already at the last line
        assert_eq!(tv.scroll(), 2);
    }

    #[test]
    fn page_scroll_jumps_several_lines_and_clamps() {
        let bytes = (0..20).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
        let mut tv = TextView::from_bytes("k", bytes.as_bytes());
        tv.page_down();
        assert_eq!(tv.scroll(), PAGE_LINES);
        tv.page_down();
        tv.page_down();
        tv.page_down();
        tv.page_down(); // overshoots past the end -- clamps to the last line
        assert_eq!(tv.scroll(), 19);
        tv.page_up();
        assert_eq!(tv.scroll(), 19 - PAGE_LINES);
    }
}
