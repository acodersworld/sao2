//! Temporary parser for the milestone-one walking skeleton.
//!
//! This syntax is deliberately separate from the permanent grammar and accepts
//! only one top-level `print("...");` statement.

use std::ops::Range;

use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;

#[derive(Debug, Eq, PartialEq)]
pub struct PrintStatement {
    pub bytes: Vec<u8>,
    pub span: Range<usize>,
}

pub fn parse(source: &SourceFile) -> Result<PrintStatement, Diagnostic> {
    Parser {
        source,
        position: 0,
    }
    .parse()
}

struct Parser<'a> {
    source: &'a SourceFile,
    position: usize,
}

impl Parser<'_> {
    fn parse(mut self) -> Result<PrintStatement, Diagnostic> {
        self.skip_trivia()?;
        let start = self.position;
        self.expect_bytes(b"print", "expected temporary top-level 'print' statement")?;
        if self.peek().is_some_and(is_identifier_continue) {
            return Err(self.error("expected '(' after 'print'"));
        }
        self.skip_trivia()?;
        self.expect_byte(b'(', "expected '(' after 'print'")?;
        self.skip_trivia()?;
        let bytes = self.parse_string()?;
        self.skip_trivia()?;
        self.expect_byte(b')', "expected ')' after string literal")?;
        self.skip_trivia()?;
        self.expect_byte(b';', "expected ';' after print statement")?;
        let end = self.position;
        self.skip_trivia()?;
        if self.position != self.source.text.len() {
            return Err(self.error("unexpected trailing input"));
        }

        Ok(PrintStatement {
            bytes,
            span: start..end,
        })
    }

    fn parse_string(&mut self) -> Result<Vec<u8>, Diagnostic> {
        self.expect_byte(b'"', "expected string literal")?;
        let mut decoded = Vec::new();

        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string literal"));
            };
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(decoded);
                }
                b'\\' => {
                    self.position += 1;
                    decoded.push(self.parse_escape()?);
                }
                b'\r' | b'\n' => {
                    return Err(self.error("raw newline is not allowed in a string literal"));
                }
                0x00..=0x7f => {
                    decoded.push(byte);
                    self.position += 1;
                }
                _ => return Err(self.error("string contents must be ASCII")),
            }
        }
    }

    fn parse_escape(&mut self) -> Result<u8, Diagnostic> {
        let Some(byte) = self.peek() else {
            return Err(self.error("unterminated escape sequence"));
        };
        self.position += 1;
        let decoded = match byte {
            b'\\' => b'\\',
            b'"' => b'"',
            b'\'' => b'\'',
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'0' => b'\0',
            b'x' => {
                let escape_start = self.position - 2;
                let high = self.hex_digit(escape_start)?;
                let low = self.hex_digit(escape_start)?;
                (high << 4) | low
            }
            _ => return Err(self.error_at(self.position - 1, "unknown escape sequence")),
        };
        if decoded > 0x7f {
            return Err(self.error_at(
                self.position.saturating_sub(1),
                "string contents must be ASCII",
            ));
        }
        Ok(decoded)
    }

    fn hex_digit(&mut self, escape_start: usize) -> Result<u8, Diagnostic> {
        let Some(byte) = self.peek() else {
            return Err(self.error_at(escape_start, "expected two hexadecimal escape digits"));
        };
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => {
                return Err(self.error_at(
                    self.position,
                    "expected hexadecimal digit in escape sequence",
                ));
            }
        };
        self.position += 1;
        Ok(value)
    }

    fn skip_trivia(&mut self) -> Result<(), Diagnostic> {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.position += 1;
            }
            if self.remaining().starts_with("//") {
                self.position += 2;
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.position += 1;
                }
            } else if self.remaining().starts_with("/*") {
                let start = self.position;
                self.position += 2;
                let Some(length) = self.remaining().find("*/") else {
                    return Err(self.error_at(start, "unterminated block comment"));
                };
                self.position += length + 2;
            } else {
                return Ok(());
            }
        }
    }

    fn expect_bytes(&mut self, expected: &[u8], message: &str) -> Result<(), Diagnostic> {
        if self.source.text.as_bytes()[self.position..].starts_with(expected) {
            self.position += expected.len();
            Ok(())
        } else {
            Err(self.error(message))
        }
    }

    fn expect_byte(&mut self, expected: u8, message: &str) -> Result<(), Diagnostic> {
        if self.peek() == Some(expected) {
            self.position += 1;
            Ok(())
        } else {
            Err(self.error(message))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.text.as_bytes().get(self.position).copied()
    }

    fn remaining(&self) -> &str {
        &self.source.text[self.position..]
    }

    fn error(&self, message: &str) -> Diagnostic {
        self.error_at(self.position, message)
    }

    fn error_at(&self, offset: usize, message: &str) -> Diagnostic {
        Diagnostic::source(self.source, offset, message)
    }
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("test.sao2"),
            text: text.to_owned(),
        }
    }

    #[test]
    fn parses_print_and_records_statement_span() {
        let parsed = parse(&source("  print(\"hello\");  ")).unwrap();
        assert_eq!(parsed.bytes, b"hello");
        assert_eq!(parsed.span, 2..17);
    }

    #[test]
    fn permits_whitespace_and_comments_between_tokens() {
        let parsed = parse(&source(
            "/* lead */ print // one\n ( /* two */ \"ok\" ) ; // end",
        ))
        .unwrap();
        assert_eq!(parsed.bytes, b"ok");
    }

    #[test]
    fn decodes_every_supported_escape() {
        let parsed = parse(&source(r#"print("\\\"\'\n\r\t\0\x41");"#)).unwrap();
        assert_eq!(parsed.bytes, b"\\\"'\n\r\t\0A");
    }

    #[test]
    fn accepts_empty_and_embedded_zero_strings() {
        assert_eq!(parse(&source(r#"print("");"#)).unwrap().bytes, b"");
        assert_eq!(parse(&source(r#"print("a\0b");"#)).unwrap().bytes, b"a\0b");
    }

    #[test]
    fn rejects_non_ascii_literal_and_escape() {
        assert!(parse(&source("print(\"é\");")).is_err());
        assert!(parse(&source(r#"print("\x80");"#)).is_err());
    }

    #[test]
    fn rejects_malformed_input_with_locations() {
        let cases = [
            ("print(123);", "1:7: expected string literal"),
            ("print(\"x\")", "1:11: expected ';'"),
            ("print(\"x\"); extra", "1:13: unexpected trailing input"),
            ("print(\"x", "1:9: unterminated string literal"),
            (r#"print("\q");"#, "1:9: unknown escape sequence"),
            ("/* never closed", "1:1: unterminated block comment"),
        ];

        for (text, expected) in cases {
            let diagnostic = parse(&source(text)).unwrap_err().to_string();
            assert!(diagnostic.contains(expected), "{diagnostic:?}");
        }
    }
}
