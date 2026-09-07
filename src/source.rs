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
        let column = self.text[self.line_starts[line_index]..offset]
            .chars()
            .fold(1, |column, character| {
                column + if character == '\t' { 4 } else { 1 }
            });
        (line_index + 1, column)
    }

    pub(crate) fn diagnostic_excerpt(&self, span: Span) -> (usize, usize, String, usize) {
        let start = self.valid_offset(span.start);
        let end = self.valid_offset(span.end.max(start));
        let line_index = self.line_index(start);
        let line_start = self.line_starts[line_index];
        let raw_line_end = self.text[line_start..]
            .find('\n')
            .map_or(self.text.len(), |relative| line_start + relative);
        let line_end = raw_line_end
            - usize::from(
                raw_line_end > line_start && self.text.as_bytes()[raw_line_end - 1] == b'\r',
            );
        let (_, column) = self.line_and_column(start);
        let rendered_line = self.text[line_start..line_end].replace('\t', "    ");
        let end_on_line = end.min(line_end);
        let caret_start = start.min(line_end);
        let caret_width = self.text[caret_start..end_on_line]
            .chars()
            .map(|character| if character == '\t' { 4 } else { 1 })
            .sum::<usize>()
            .max(1);
        (line_index + 1, column, rendered_line, caret_width)
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
        let (_, column, line, caret_width) = source.diagnostic_excerpt(Span::empty(3));
        assert_eq!(column, 4);
        assert_eq!(line, "bad");
        assert_eq!(caret_width, 1);
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
