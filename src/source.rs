use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

/// A half-open range of UTF-8 byte offsets in a source file.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn empty(offset: usize) -> Self {
        Self::new(offset, offset)
    }
}

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(path: PathBuf, text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
        );
        Self {
            path,
            text,
            line_starts,
        }
    }

    pub fn load(path: &Path) -> Result<Self, Diagnostic> {
        let metadata = fs::metadata(path).map_err(|error| {
            Diagnostic::input(format!("cannot access '{}': {error}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(Diagnostic::input(format!(
                "source path '{}' is not a regular file",
                path.display()
            )));
        }

        let bytes = fs::read(path).map_err(|error| {
            Diagnostic::input(format!("cannot read '{}': {error}", path.display()))
        })?;
        let text = String::from_utf8(bytes).map_err(|error| {
            Diagnostic::input(format!(
                "source file '{}' is not valid UTF-8 at byte {}",
                path.display(),
                error.utf8_error().valid_up_to()
            ))
        })?;

        Ok(Self::new(path.to_path_buf(), text))
    }

    /// Returns a one-based line and display column for a byte offset.
    pub fn line_and_column(&self, offset: usize) -> (usize, usize) {
        let offset = self.valid_offset(offset);
        let line_index = self.line_index(offset);
        let line_end = self.line_end(line_index);
        let position = offset.min(line_end);
        let column = display_width(&self.text[self.line_starts[line_index]..position]) + 1;
        (line_index + 1, column)
    }

    pub(crate) fn diagnostic_excerpt(&self, span: Span) -> DiagnosticExcerpt {
        let start = self.valid_offset(span.start);
        let end = self.valid_offset(span.end.max(start));
        let line_index = self.line_index(start);
        let line_start = self.line_starts[line_index];
        let line_end = self.line_end(line_index);
        let rendered_line = self.text[line_start..line_end].replace('\t', "    ");
        let caret_start = start.min(line_end);
        let end_on_line = end.max(caret_start).min(line_end);
        let caret_offset = display_width(&self.text[line_start..caret_start]);
        let caret_width = display_width(&self.text[caret_start..end_on_line]).max(1);
        DiagnosticExcerpt {
            line: line_index + 1,
            column: caret_offset + 1,
            source_line: rendered_line,
            caret_offset,
            caret_width,
        }
    }

    fn line_end(&self, line_index: usize) -> usize {
        let line_start = self.line_starts[line_index];
        let raw_line_end = self.text[line_start..]
            .find('\n')
            .map_or(self.text.len(), |relative| line_start + relative);
        raw_line_end
            - usize::from(
                raw_line_end > line_start && self.text.as_bytes()[raw_line_end - 1] == b'\r',
            )
    }

    fn line_index(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= offset) - 1
    }

    fn valid_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiagnosticExcerpt {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) source_line: String,
    pub(crate) caret_offset: usize,
    pub(crate) caret_width: usize,
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|character| usize::from(character != '\t') + usize::from(character == '\t') * 4)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sao2-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn loads_utf8_and_preserves_path() {
        let path = temporary_path("valid");
        fs::write(&path, "print(\"hello\");").unwrap();
        let source = SourceFile::load(&path).unwrap();
        assert_eq!(source.path, path);
        assert_eq!(source.text, "print(\"hello\");");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn precomputes_one_based_locations_using_byte_offsets() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "one\n\tαx\nthree".to_owned());
        assert_eq!(source.line_and_column(0), (1, 1));
        assert_eq!(source.line_and_column(5), (2, 5));
        assert_eq!(source.line_and_column(7), (2, 6));
        assert_eq!(source.line_and_column(source.text.len()), (3, 6));
    }

    #[test]
    fn rejects_missing_files_and_directories() {
        let missing = temporary_path("missing");
        assert!(SourceFile::load(&missing).is_err());
        assert!(SourceFile::load(std::env::temp_dir().as_path()).is_err());
    }

    #[test]
    fn renders_a_caret_safely_at_crlf_boundaries() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "bad\r\nnext".to_owned());
        let excerpt = source.diagnostic_excerpt(Span::empty(3));
        assert_eq!(excerpt.column, 4);
        assert_eq!(excerpt.source_line, "bad");
        assert_eq!(excerpt.caret_width, 1);
    }

    #[test]
    fn renders_empty_and_boundary_spans_on_the_selected_line() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "abc\ndef\n".to_owned());
        let first = source.diagnostic_excerpt(Span::empty(0));
        assert_eq!((first.line, first.column, first.caret_offset, first.caret_width), (1, 1, 0, 1));
        let interior = source.diagnostic_excerpt(Span::empty(1));
        assert_eq!((interior.line, interior.column, interior.caret_offset, interior.caret_width), (1, 2, 1, 1));
        let end = source.diagnostic_excerpt(Span::empty(source.text.len()));
        assert_eq!((end.line, end.column, end.caret_offset, end.caret_width), (3, 1, 0, 1));
        let crossing = source.diagnostic_excerpt(Span::new(1, 5));
        assert_eq!((crossing.line, crossing.column, crossing.caret_offset, crossing.caret_width), (1, 2, 1, 2));
        assert_eq!(crossing.source_line, "abc");
    }

    #[test]
    fn expands_tabs_for_lines_and_carets() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "a\tbc".to_owned());
        let before_tab = source.diagnostic_excerpt(Span::new(1, 2));
        assert_eq!(before_tab.source_line, "a    bc");
        assert_eq!((before_tab.column, before_tab.caret_offset, before_tab.caret_width), (2, 1, 4));
        let after_tab = source.diagnostic_excerpt(Span::empty(2));
        assert_eq!((after_tab.column, after_tab.caret_offset, after_tab.caret_width), (6, 5, 1));
    }

    #[test]
    fn clamps_unicode_boundaries_and_out_of_range_offsets() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "αβ".to_owned());
        let inside = source.diagnostic_excerpt(Span::new(1, 3));
        assert_eq!((inside.column, inside.caret_offset, inside.caret_width), (1, 0, 1));
        let beyond = source.diagnostic_excerpt(Span::new(99, 120));
        assert_eq!((beyond.line, beyond.column, beyond.caret_offset, beyond.caret_width), (1, 3, 2, 1));
    }

    #[test]
    fn handles_empty_files_and_crlf_carriage_returns_without_rendering_them() {
        let empty = SourceFile::new(PathBuf::from("empty.sao2"), String::new());
        let excerpt = empty.diagnostic_excerpt(Span::empty(100));
        assert_eq!((excerpt.line, excerpt.column, excerpt.caret_offset, excerpt.caret_width), (1, 1, 0, 1));

        let crlf = SourceFile::new(PathBuf::from("crlf.sao2"), "bad\r\nnext".to_owned());
        let after_cr = crlf.diagnostic_excerpt(Span::empty(4));
        assert_eq!((after_cr.line, after_cr.column, after_cr.caret_offset), (1, 4, 3));
        assert_eq!(after_cr.source_line, "bad");
    }

    #[test]
    fn rejects_invalid_utf8() {
        let path = temporary_path("invalid-utf8");
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let error = SourceFile::load(&path).unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8 at byte 0"));
        fs::remove_file(path).unwrap();
    }
}
