//! The permanent SAO2 lexer.
//!
//! These tokens are the input to the permanent parser.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::source::{SourceFile, Span};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Identifier,
    Integer,
    Float,
    String(Vec<u8>),
    Character(u8),

    Type,
    Fn,
    Var,
    Return,
    Break,
    Continue,
    If,
    Else,
    While,
    For,
    In,
    Switch,
    Is,
    True,
    False,
    Int,
    FloatType,
    Str,
    Bool,
    Char,

    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Semicolon,
    Colon,
    Dot,

    Equal,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Ampersand,
    Pipe,
    Caret,
    Bang,
    Tilde,
    Less,
    Greater,
    Question,

    Declare,
    EqualEqual,
    BangEqual,
    LessEqual,
    GreaterEqual,
    LogicalOr,
    LogicalAnd,
    ShiftLeft,
    ShiftRight,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    PercentEqual,
    AmpersandEqual,
    PipeEqual,
    CaretEqual,
    ShiftLeftEqual,
    ShiftRightEqual,

    Eof,
}

pub fn lex(source: &SourceFile) -> Result<Vec<Token>, Diagnostics> {
    Lexer::new(source).lex()
}

struct Lexer<'source> {
    source: &'source SourceFile,
    position: usize,
    tokens: Vec<Token>,
    diagnostics: Diagnostics,
}

impl<'source> Lexer<'source> {
    fn new(source: &'source SourceFile) -> Self {
        Self {
            source,
            position: 0,
            tokens: Vec::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    fn lex(mut self) -> Result<Vec<Token>, Diagnostics> {
        while self.position < self.bytes().len() && !self.diagnostics.is_full() {
            if self.skip_trivia() {
                continue;
            }

            let byte = self.bytes()[self.position];
            if is_identifier_start(byte) {
                self.lex_identifier();
            } else if byte.is_ascii_digit() {
                self.lex_number();
            } else {
                match byte {
                    b'"' => self.lex_string(),
                    b'\'' => self.lex_character(),
                    b'.' if self.peek(1).is_some_and(|next| next.is_ascii_digit())
                        && !self.dot_can_be_member_access() =>
                    {
                        self.reject_leading_dot_float()
                    }
                    _ => self.lex_symbol_or_error(),
                }
            }
        }

        if self.diagnostics.is_empty() {
            self.tokens.push(Token {
                kind: TokenKind::Eof,
                span: Span::empty(self.source.text.len()),
            });
            Ok(self.tokens)
        } else {
            Err(self.diagnostics)
        }
    }

    /// Returns true when at least one byte of trivia was consumed.
    fn skip_trivia(&mut self) -> bool {
        let start = self.position;
        loop {
            while self
                .current()
                .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            {
                self.position += 1;
            }

            if self.remaining().starts_with("//") {
                self.position += 2;
                while self.current().is_some_and(|byte| byte != b'\n') {
                    self.position += 1;
                }
            } else if self.remaining().starts_with("/*") {
                let comment_start = self.position;
                self.position += 2;
                if let Some(length) = self.remaining().find("*/") {
                    self.position += length + 2;
                } else {
                    self.position = self.bytes().len();
                    self.error(
                        Span::new(comment_start, self.position),
                        "unterminated block comment",
                    );
                    return true;
                }
            } else {
                break;
            }
        }
        self.position != start
    }

    fn lex_identifier(&mut self) {
        let start = self.position;
        self.position += 1;
        while self.current().is_some_and(is_identifier_continue) {
            self.position += 1;
        }
        let kind = match &self.source.text[start..self.position] {
            "type" => TokenKind::Type,
            "fn" => TokenKind::Fn,
            "var" => TokenKind::Var,
            "return" => TokenKind::Return,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "switch" => TokenKind::Switch,
            "is" => TokenKind::Is,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "int" => TokenKind::Int,
            "float" => TokenKind::FloatType,
            "str" => TokenKind::Str,
            "bool" => TokenKind::Bool,
            "char" => TokenKind::Char,
            _ => TokenKind::Identifier,
        };
        self.push(kind, start);
    }

    fn lex_number(&mut self) {
        let start = self.position;
        if self.remaining().starts_with("0X") {
            self.reject_uppercase_base_prefix(start, "hexadecimal");
            return;
        }
        if self.remaining().starts_with("0B") {
            self.reject_uppercase_base_prefix(start, "binary");
            return;
        }
        if self.remaining().starts_with("0x") {
            self.position += 2;
            self.lex_based_integer(start, 16, "hexadecimal");
            return;
        }
        if self.remaining().starts_with("0b") {
            self.position += 2;
            self.lex_based_integer(start, 2, "binary");
            return;
        }
        if self
            .tokens
            .last()
            .is_some_and(|token| matches!(token.kind, TokenKind::Dot))
        {
            let valid = self.consume_digits(10);
            if valid {
                self.push(TokenKind::Integer, start);
            } else {
                self.error(
                    Span::new(start, self.position),
                    "numeric separators must appear between digits",
                );
            }
            return;
        }

        let integer_valid = self.consume_digits(10);
        let mut is_float = false;
        let mut valid = integer_valid;

        if self.current() == Some(b'.') && self.peek(1).is_some_and(|byte| byte.is_ascii_digit()) {
            is_float = true;
            self.position += 1;
            valid &= self.consume_digits(10);
        } else if self.current() == Some(b'.') && !self.peek(1).is_some_and(is_identifier_start) {
            self.position += 1;
            self.error(
                Span::new(start, self.position),
                "floating-point literal requires digits after '.'",
            );
            return;
        }

        if self
            .current()
            .is_some_and(|byte| matches!(byte, b'e' | b'E'))
        {
            is_float = true;
            self.position += 1;
            if self
                .current()
                .is_some_and(|byte| matches!(byte, b'+' | b'-'))
            {
                self.position += 1;
            }
            if !self.current().is_some_and(|byte| byte.is_ascii_digit()) {
                self.error(
                    Span::new(start, self.position),
                    "floating-point exponent requires digits",
                );
                return;
            }
            valid &= self.consume_digits(10);
        }

        if valid {
            self.push(
                if is_float {
                    TokenKind::Float
                } else {
                    TokenKind::Integer
                },
                start,
            );
        } else {
            self.error(
                Span::new(start, self.position),
                "numeric separators must appear between digits",
            );
        }
    }

    fn lex_based_integer(&mut self, start: usize, radix: u32, description: &str) {
        let digits_start = self.position;
        let valid_separators = self.consume_digits(radix);
        let mut valid_digits = self.position > digits_start;

        while self
            .current()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            valid_digits = false;
            self.position += 1;
        }

        if !valid_digits {
            self.error(
                Span::new(start, self.position),
                format!("invalid {description} integer literal"),
            );
        } else if !valid_separators {
            self.error(
                Span::new(start, self.position),
                "numeric separators must appear between digits",
            );
        } else {
            self.push(TokenKind::Integer, start);
        }
    }

    fn reject_uppercase_base_prefix(&mut self, start: usize, description: &str) {
        self.position += 2;
        while self
            .current()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.position += 1;
        }
        self.error(
            Span::new(start, self.position),
            format!("{description} integer prefix must be lowercase"),
        );
    }

    /// Consumes digits and underscores, returning whether every underscore was
    /// surrounded by digits of the requested radix.
    fn consume_digits(&mut self, radix: u32) -> bool {
        let mut valid = true;
        let mut previous_was_digit = false;
        while let Some(byte) = self.current() {
            if digit_value(byte).is_some_and(|value| value < radix) {
                previous_was_digit = true;
                self.position += 1;
            } else if byte == b'_' {
                let next_is_digit = self
                    .peek(1)
                    .and_then(digit_value)
                    .is_some_and(|value| value < radix);
                valid &= previous_was_digit && next_is_digit;
                previous_was_digit = false;
                self.position += 1;
            } else {
                break;
            }
        }
        valid
    }

    fn reject_leading_dot_float(&mut self) {
        let start = self.position;
        self.position += 1;
        self.consume_digits(10);
        self.error(
            Span::new(start, self.position),
            "floating-point literal requires digits before '.'",
        );
    }

    fn dot_can_be_member_access(&self) -> bool {
        self.tokens.last().is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::Identifier
                    | TokenKind::Integer
                    | TokenKind::Float
                    | TokenKind::String(_)
                    | TokenKind::Character(_)
                    | TokenKind::True
                    | TokenKind::False
                    | TokenKind::RightParen
                    | TokenKind::RightBracket
                    | TokenKind::RightBrace
                    | TokenKind::Question
            )
        })
    }

    fn lex_string(&mut self) {
        let start = self.position;
        self.position += 1;
        let mut decoded = Vec::new();
        let mut valid = true;

        loop {
            let Some(byte) = self.current() else {
                self.error(
                    Span::new(start, self.position),
                    "unterminated string literal",
                );
                return;
            };
            match byte {
                b'"' => {
                    self.position += 1;
                    if valid {
                        self.push(TokenKind::String(decoded), start);
                    }
                    return;
                }
                b'\r' | b'\n' => {
                    self.error(
                        Span::new(self.position, self.position + 1),
                        "raw newline is not allowed in a string literal",
                    );
                    self.recover_quoted_literal(b'"');
                    return;
                }
                b'\\' => match self.decode_escape() {
                    Some(value) => decoded.push(value),
                    None if self.position == self.bytes().len() => return,
                    None => valid = false,
                },
                0x00..=0x7f => {
                    decoded.push(byte);
                    self.position += 1;
                }
                _ => {
                    let invalid_start = self.position;
                    self.advance_character();
                    self.error(
                        Span::new(invalid_start, self.position),
                        "string contents must be ASCII",
                    );
                    valid = false;
                }
            }
        }
    }

    fn lex_character(&mut self) {
        let start = self.position;
        self.position += 1;
        let mut decoded = Vec::new();
        let mut valid = true;

        loop {
            let Some(byte) = self.current() else {
                self.error(
                    Span::new(start, self.position),
                    "unterminated character literal",
                );
                return;
            };
            match byte {
                b'\'' => {
                    self.position += 1;
                    if valid && decoded.len() == 1 {
                        self.push(TokenKind::Character(decoded[0]), start);
                    } else if valid {
                        self.error(
                            Span::new(start, self.position),
                            "character literal must contain exactly one ASCII character",
                        );
                    }
                    return;
                }
                b'\r' | b'\n' => {
                    self.error(
                        Span::new(self.position, self.position + 1),
                        "raw newline is not allowed in a character literal",
                    );
                    self.recover_quoted_literal(b'\'');
                    return;
                }
                b'\\' => match self.decode_escape() {
                    Some(value) => decoded.push(value),
                    None if self.position == self.bytes().len() => return,
                    None => valid = false,
                },
                0x00..=0x7f => {
                    decoded.push(byte);
                    self.position += 1;
                }
                _ => {
                    let invalid_start = self.position;
                    self.advance_character();
                    self.error(
                        Span::new(invalid_start, self.position),
                        "character contents must be ASCII",
                    );
                    valid = false;
                }
            }
        }
    }

    fn decode_escape(&mut self) -> Option<u8> {
        let start = self.position;
        self.position += 1;
        let Some(byte) = self.current() else {
            self.error(
                Span::new(start, self.position),
                "unterminated escape sequence",
            );
            return None;
        };
        self.position += 1;
        let value = match byte {
            b'\\' => b'\\',
            b'"' => b'"',
            b'\'' => b'\'',
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'0' => b'\0',
            b'x' => {
                let Some(high) = self.consume_hex_escape_digit() else {
                    self.error(
                        Span::new(start, self.position),
                        "expected two hexadecimal escape digits",
                    );
                    return None;
                };
                let Some(low) = self.consume_hex_escape_digit() else {
                    self.error(
                        Span::new(start, self.position),
                        "expected two hexadecimal escape digits",
                    );
                    return None;
                };
                (high << 4) | low
            }
            _ => {
                self.error(Span::new(start, self.position), "unknown escape sequence");
                return None;
            }
        };
        if value > 0x7f {
            self.error(
                Span::new(start, self.position),
                "literal contents must be ASCII",
            );
            None
        } else {
            Some(value)
        }
    }

    fn consume_hex_escape_digit(&mut self) -> Option<u8> {
        let value = self
            .current()
            .and_then(digit_value)
            .filter(|value| *value < 16)?;
        self.position += 1;
        Some(value as u8)
    }

    fn recover_quoted_literal(&mut self, quote: u8) {
        self.position += 1;
        while let Some(byte) = self.current() {
            self.position += 1;
            if byte == quote || byte == b'\n' {
                break;
            }
        }
    }

    fn lex_symbol_or_error(&mut self) {
        let start = self.position;
        let (kind, length) = if self.remaining().starts_with(">>=") {
            (TokenKind::ShiftRightEqual, 3)
        } else if self.remaining().starts_with("<<=") {
            (TokenKind::ShiftLeftEqual, 3)
        } else if self.remaining().starts_with(":=") {
            (TokenKind::Declare, 2)
        } else if self.remaining().starts_with("==") {
            (TokenKind::EqualEqual, 2)
        } else if self.remaining().starts_with("!=") {
            (TokenKind::BangEqual, 2)
        } else if self.remaining().starts_with("<=") {
            (TokenKind::LessEqual, 2)
        } else if self.remaining().starts_with(">=") {
            (TokenKind::GreaterEqual, 2)
        } else if self.remaining().starts_with("||") {
            (TokenKind::LogicalOr, 2)
        } else if self.remaining().starts_with("&&") {
            (TokenKind::LogicalAnd, 2)
        } else if self.remaining().starts_with("<<") {
            (TokenKind::ShiftLeft, 2)
        } else if self.remaining().starts_with(">>") {
            (TokenKind::ShiftRight, 2)
        } else if self.remaining().starts_with("+=") {
            (TokenKind::PlusEqual, 2)
        } else if self.remaining().starts_with("-=") {
            (TokenKind::MinusEqual, 2)
        } else if self.remaining().starts_with("*=") {
            (TokenKind::StarEqual, 2)
        } else if self.remaining().starts_with("/=") {
            (TokenKind::SlashEqual, 2)
        } else if self.remaining().starts_with("%=") {
            (TokenKind::PercentEqual, 2)
        } else if self.remaining().starts_with("&=") {
            (TokenKind::AmpersandEqual, 2)
        } else if self.remaining().starts_with("|=") {
            (TokenKind::PipeEqual, 2)
        } else if self.remaining().starts_with("^=") {
            (TokenKind::CaretEqual, 2)
        } else {
            let kind = match self.bytes()[self.position] {
                b'(' => TokenKind::LeftParen,
                b')' => TokenKind::RightParen,
                b'{' => TokenKind::LeftBrace,
                b'}' => TokenKind::RightBrace,
                b'[' => TokenKind::LeftBracket,
                b']' => TokenKind::RightBracket,
                b',' => TokenKind::Comma,
                b';' => TokenKind::Semicolon,
                b':' => TokenKind::Colon,
                b'.' => TokenKind::Dot,
                b'=' => TokenKind::Equal,
                b'+' => TokenKind::Plus,
                b'-' => TokenKind::Minus,
                b'*' => TokenKind::Star,
                b'/' => TokenKind::Slash,
                b'%' => TokenKind::Percent,
                b'&' => TokenKind::Ampersand,
                b'|' => TokenKind::Pipe,
                b'^' => TokenKind::Caret,
                b'!' => TokenKind::Bang,
                b'~' => TokenKind::Tilde,
                b'<' => TokenKind::Less,
                b'>' => TokenKind::Greater,
                b'?' => TokenKind::Question,
                _ => {
                    self.advance_character();
                    self.error(
                        Span::new(start, self.position),
                        "unexpected character in source",
                    );
                    return;
                }
            };
            (kind, 1)
        };
        self.position += length;
        self.push(kind, start);
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start, self.position),
        });
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::source(self.source, span, message));
    }

    fn advance_character(&mut self) {
        let character = self.source.text[self.position..].chars().next().unwrap();
        self.position += character.len_utf8();
    }

    fn bytes(&self) -> &[u8] {
        self.source.text.as_bytes()
    }

    fn remaining(&self) -> &str {
        &self.source.text[self.position..]
    }

    fn current(&self) -> Option<u8> {
        self.peek(0)
    }

    fn peek(&self, distance: usize) -> Option<u8> {
        self.bytes().get(self.position + distance).copied()
    }
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_identifier_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a' + 10)),
        b'A'..=b'F' => Some(u32::from(byte - b'A' + 10)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex(&source(text))
            .unwrap()
            .into_iter()
            .map(|token| token.kind)
            .collect()
    }

    #[test]
    fn recognizes_every_keyword_only_as_a_complete_token() {
        assert_eq!(
            kinds(
                "type fn var return break continue if else while for in switch is true false int float str bool char typeName inner"
            ),
            vec![
                TokenKind::Type,
                TokenKind::Fn,
                TokenKind::Var,
                TokenKind::Return,
                TokenKind::Break,
                TokenKind::Continue,
                TokenKind::If,
                TokenKind::Else,
                TokenKind::While,
                TokenKind::For,
                TokenKind::In,
                TokenKind::Switch,
                TokenKind::Is,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Int,
                TokenKind::FloatType,
                TokenKind::Str,
                TokenKind::Bool,
                TokenKind::Char,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_every_operator_and_delimiter_with_longest_match() {
        assert_eq!(
            kinds(
                "( ) { } [ ] , ; : . = + - * / % & | ^ ! ~ < > ? := == != <= >= || && << >> += -= *= /= %= &= |= ^= <<= >>="
            ),
            vec![
                TokenKind::LeftParen,
                TokenKind::RightParen,
                TokenKind::LeftBrace,
                TokenKind::RightBrace,
                TokenKind::LeftBracket,
                TokenKind::RightBracket,
                TokenKind::Comma,
                TokenKind::Semicolon,
                TokenKind::Colon,
                TokenKind::Dot,
                TokenKind::Equal,
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Ampersand,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Bang,
                TokenKind::Tilde,
                TokenKind::Less,
                TokenKind::Greater,
                TokenKind::Question,
                TokenKind::Declare,
                TokenKind::EqualEqual,
                TokenKind::BangEqual,
                TokenKind::LessEqual,
                TokenKind::GreaterEqual,
                TokenKind::LogicalOr,
                TokenKind::LogicalAnd,
                TokenKind::ShiftLeft,
                TokenKind::ShiftRight,
                TokenKind::PlusEqual,
                TokenKind::MinusEqual,
                TokenKind::StarEqual,
                TokenKind::SlashEqual,
                TokenKind::PercentEqual,
                TokenKind::AmpersandEqual,
                TokenKind::PipeEqual,
                TokenKind::CaretEqual,
                TokenKind::ShiftLeftEqual,
                TokenKind::ShiftRightEqual,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn records_half_open_byte_spans_and_eof() {
        let tokens = lex(&source("α fn_name")).unwrap_err();
        assert!(tokens.to_string().contains("1:1"));

        let tokens = lex(&source("fn name")).unwrap();
        assert_eq!(tokens[0].span, Span::new(0, 2));
        assert_eq!(tokens[1].span, Span::new(3, 7));
        assert_eq!(tokens[2].span, Span::empty(7));
    }

    #[test]
    fn discards_whitespace_and_nonnested_comments() {
        assert_eq!(
            kinds(" /* outer /* not nested */ fn // rest\r\n name"),
            vec![TokenKind::Fn, TokenKind::Identifier, TokenKind::Eof]
        );
    }

    #[test]
    fn decodes_string_and_character_escapes() {
        assert_eq!(
            kinds(r#""\\\"\'\n\r\t\0\x41" '\n' 'A'"#),
            vec![
                TokenKind::String(b"\\\"'\n\r\t\0A".to_vec()),
                TokenKind::Character(b'\n'),
                TokenKind::Character(b'A'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn recognizes_valid_numeric_forms_and_keeps_signs_separate() {
        assert_eq!(
            kinds("42 1_000 0xff 0xca_fe 0b1010 1.0 1_0.2_5 1e10 2.5e-3 -42 +1"),
            vec![
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Float,
                TokenKind::Float,
                TokenKind::Float,
                TokenKind::Float,
                TokenKind::Minus,
                TokenKind::Integer,
                TokenKind::Plus,
                TokenKind::Integer,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn rejects_malformed_numbers() {
        for (text, message) in [
            ("1_", "numeric separators"),
            ("1__0", "numeric separators"),
            ("0x", "invalid hexadecimal"),
            ("0x_1", "numeric separators"),
            ("0b102", "invalid binary"),
            ("0Xff", "prefix must be lowercase"),
            ("0B10", "prefix must be lowercase"),
            ("1.", "digits after"),
            (".5", "digits before"),
            ("1e+", "exponent requires digits"),
        ] {
            let diagnostic = lex(&source(text)).unwrap_err().to_string();
            assert!(diagnostic.contains(message), "{text:?}: {diagnostic}");
        }
    }

    #[test]
    fn rejects_malformed_literals_and_comments() {
        for (text, message) in [
            (r#""\q""#, "unknown escape"),
            (r#""\x8""#, "two hexadecimal"),
            (r#""\x80""#, "must be ASCII"),
            ("\"unterminated", "unterminated string"),
            ("'ab'", "exactly one"),
            ("'é'", "must be ASCII"),
            ("/* unterminated", "unterminated block comment"),
        ] {
            let diagnostic = lex(&source(text)).unwrap_err().to_string();
            assert!(diagnostic.contains(message), "{text:?}: {diagnostic}");
        }
    }

    #[test]
    fn comment_markers_inside_literals_are_plain_content() {
        assert_eq!(
            kinds(r#""// not a comment" "/* neither */" '/'"#),
            vec![
                TokenKind::String(b"// not a comment".to_vec()),
                TokenKind::String(b"/* neither */".to_vec()),
                TokenKind::Character(b'/'),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn collects_independent_errors_in_source_order_and_stops_at_twenty() {
        let diagnostics = lex(&source(&"@ ".repeat(25))).unwrap_err();
        assert_eq!(diagnostics.len(), 20);
        let rendered = diagnostics.to_string();
        assert_eq!(rendered.matches("unexpected character").count(), 20);
        assert!(rendered.find("1:1").unwrap() < rendered.find("1:3").unwrap());
    }

    #[test]
    fn distinguishes_tuple_member_access_from_a_leading_dot_float() {
        assert_eq!(
            kinds("value.0.1 1.member 1.0.member"),
            vec![
                TokenKind::Identifier,
                TokenKind::Dot,
                TokenKind::Integer,
                TokenKind::Dot,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::Dot,
                TokenKind::Identifier,
                TokenKind::Float,
                TokenKind::Dot,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
        assert!(lex(&source(".5")).is_err());
    }
}
