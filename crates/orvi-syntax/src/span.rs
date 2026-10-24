use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Option<Self> {
        if start <= end {
            Some(Self { start, end })
        } else {
            None
        }
    }

    pub(crate) const fn from_valid_bounds(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn start(self) -> usize {
        self.start
    }

    pub const fn end(self) -> usize {
        self.end
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn range(self) -> Range<usize> {
        self.start..self.end
    }

    pub fn text(self, source: &str) -> Option<&str> {
        source.get(self.range())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LineColumn {
    pub line: usize,
    pub column: usize,
}

pub fn line_column(source: &str, offset: usize) -> Option<LineColumn> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }

    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, current_line)| current_line)
        .trim_end_matches('\r')
        .chars()
        .count()
        + 1;

    Some(LineColumn { line, column })
}

#[cfg(test)]
mod tests {
    use super::{LineColumn, Span, line_column};

    #[test]
    fn rejects_reversed_span_bounds() {
        assert_eq!(Span::new(4, 3), None);
    }

    #[test]
    fn extracts_text_at_utf8_boundaries() {
        let source = "let żółw";
        let span = Span::new(4, source.len()).expect("valid bounds");

        assert_eq!(span.text(source), Some("żółw"));
    }

    #[test]
    fn rejects_offsets_inside_utf8_characters() {
        assert_eq!(line_column("ą", 1), None);
    }

    #[test]
    fn calculates_one_based_line_and_column() {
        let source = "first\r\nżółw";

        assert_eq!(
            line_column(source, source.len()),
            Some(LineColumn { line: 2, column: 5 })
        );
    }
}
