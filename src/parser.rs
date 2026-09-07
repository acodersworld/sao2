//! Early permanent-parser slice for Phase 3.
//!
//! Later phases extend these routines with the remaining declaration, type,
//! expression, and statement productions. Entry-point rules are deliberately
//! enforced by `compiler`, not here.

use std::mem::discriminant;

use crate::ast::{
    Block, Declaration, Expression, ExpressionKind, FunctionDeclaration, Identifier, Parameter,
    PrimitiveType, Program, Statement, StatementKind, Type, TypeKind,
};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::lexer::{self, Token, TokenKind};
use crate::source::{SourceFile, Span};

pub fn parse(source: &SourceFile) -> Result<Program, Diagnostics> {
    let tokens = lexer::lex(source)?;
    match Parser::new(source, tokens).parse_program() {
        Ok(program) => Ok(program),
        Err(diagnostic) => {
            let mut diagnostics = Diagnostics::new();
            diagnostics.push(diagnostic);
            Err(diagnostics)
        }
    }
}

struct Parser<'source> {
    source: &'source SourceFile,
    tokens: Vec<Token>,
    current: usize,
}

impl<'source> Parser<'source> {
    fn new(source: &'source SourceFile, tokens: Vec<Token>) -> Self {
        Self {
            source,
            tokens,
            current: 0,
        }
    }

    fn parse_program(mut self) -> Result<Program, Diagnostic> {
        let mut declarations = Vec::new();
        while !self.at(&TokenKind::Eof) {
            declarations.push(Declaration::Function(self.parse_function()?));
        }
        Ok(Program {
            declarations,
            span: Span::new(0, self.source.text.len()),
        })
    }

    fn parse_function(&mut self) -> Result<FunctionDeclaration, Diagnostic> {
        let start = self
            .expect(
                &TokenKind::Fn,
                "expected function declaration beginning with 'fn'",
            )?
            .span
            .start;
        let name = self.parse_identifier("expected function name after 'fn'")?;
        self.expect(&TokenKind::LeftParen, "expected '(' after function name")?;

        let mut parameters = Vec::new();
        if !self.at(&TokenKind::RightParen) {
            loop {
                parameters.push(self.parse_parameter()?);
                if self.take(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(
            &TokenKind::RightParen,
            "expected ')' after function parameters",
        )?;

        let return_type = if self.at_type_start() {
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(FunctionDeclaration {
            name,
            parameters,
            return_type,
            span: Span::new(start, body.span.end),
            body,
        })
    }

    fn parse_parameter(&mut self) -> Result<Parameter, Diagnostic> {
        let mutable_token = self.take(&TokenKind::Var);
        let name = self.parse_identifier("expected parameter name")?;
        let start = mutable_token
            .as_ref()
            .map_or(name.span.start, |token| token.span.start);
        let ty = self.parse_type()?;
        Ok(Parameter {
            mutable: mutable_token.is_some(),
            name,
            span: Span::new(start, ty.span.end),
            ty,
        })
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        let token = self.current_token().clone();
        let (kind, span) = match token.kind {
            TokenKind::Int => (TypeKind::Primitive(PrimitiveType::Int), token.span),
            TokenKind::FloatType => (TypeKind::Primitive(PrimitiveType::Float), token.span),
            TokenKind::Str => (TypeKind::Primitive(PrimitiveType::Str), token.span),
            TokenKind::Bool => (TypeKind::Primitive(PrimitiveType::Bool), token.span),
            TokenKind::Char => (TypeKind::Primitive(PrimitiveType::Char), token.span),
            TokenKind::Identifier => {
                let identifier = Identifier { span: token.span };
                (TypeKind::Named(identifier), token.span)
            }
            TokenKind::LeftBracket => {
                self.current += 1;
                let element = self.parse_type()?;
                let end = self
                    .expect(&TokenKind::RightBracket, "expected ']' after list type")?
                    .span
                    .end;
                return Ok(Type {
                    kind: TypeKind::List(Box::new(element)),
                    span: Span::new(token.span.start, end),
                });
            }
            _ => return Err(self.error_current("expected type")),
        };
        self.current += 1;
        Ok(Type { kind, span })
    }

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let start = self
            .expect(
                &TokenKind::LeftBrace,
                "expected function body beginning with '{'",
            )?
            .span
            .start;
        let mut statements = Vec::new();
        let mut value = None;

        while !self.at(&TokenKind::RightBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(self.error_current("expected '}' to close block"));
            }
            if self.at(&TokenKind::LeftBrace) {
                let block = self.parse_block()?;
                let span = block.span;
                statements.push(Statement {
                    kind: StatementKind::Block(block),
                    span,
                });
                continue;
            }

            let expression = self.parse_expression()?;
            if let Some(semicolon) = self.take(&TokenKind::Semicolon) {
                statements.push(Statement {
                    span: Span::new(expression.span.start, semicolon.span.end),
                    kind: StatementKind::Expression(expression),
                });
            } else if self.at(&TokenKind::RightBrace) {
                value = Some(Box::new(expression));
                break;
            } else {
                return Err(self.error_current("expected ';' or '}' after expression"));
            }
        }

        let end = self
            .expect(&TokenKind::RightBrace, "expected '}' to close block")?
            .span
            .end;
        Ok(Block {
            statements,
            value,
            span: Span::new(start, end),
        })
    }

    fn parse_expression(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.parse_primary()?;
        while self.take(&TokenKind::LeftParen).is_some() {
            let mut arguments = Vec::new();
            if !self.at(&TokenKind::RightParen) {
                loop {
                    arguments.push(self.parse_expression()?);
                    if self.take(&TokenKind::Comma).is_none() {
                        break;
                    }
                }
            }
            let end = self
                .expect(&TokenKind::RightParen, "expected ')' after call arguments")?
                .span
                .end;
            let start = expression.span.start;
            expression = Expression {
                kind: ExpressionKind::Call {
                    callee: Box::new(expression),
                    arguments,
                },
                span: Span::new(start, end),
            };
        }
        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<Expression, Diagnostic> {
        let token = self.current_token().clone();
        match token.kind {
            TokenKind::Identifier => {
                self.current += 1;
                let identifier = Identifier { span: token.span };
                Ok(Expression {
                    kind: ExpressionKind::Identifier(identifier),
                    span: token.span,
                })
            }
            TokenKind::String(bytes) => {
                self.current += 1;
                Ok(Expression {
                    kind: ExpressionKind::String(bytes),
                    span: token.span,
                })
            }
            _ => Err(self.error_current("expected identifier or string literal")),
        }
    }

    fn parse_identifier(&mut self, message: &str) -> Result<Identifier, Diagnostic> {
        let token = self.expect(&TokenKind::Identifier, message)?;
        Ok(Identifier { span: token.span })
    }

    fn at_type_start(&self) -> bool {
        self.at(&TokenKind::Int)
            || self.at(&TokenKind::FloatType)
            || self.at(&TokenKind::Str)
            || self.at(&TokenKind::Bool)
            || self.at(&TokenKind::Char)
            || self.at(&TokenKind::Identifier)
            || self.at(&TokenKind::LeftBracket)
    }

    fn at(&self, kind: &TokenKind) -> bool {
        discriminant(&self.current_token().kind) == discriminant(kind)
    }

    fn take(&mut self, kind: &TokenKind) -> Option<Token> {
        if self.at(kind) {
            let token = self.current_token().clone();
            self.current += 1;
            Some(token)
        } else {
            None
        }
    }

    fn expect(&mut self, kind: &TokenKind, message: &str) -> Result<Token, Diagnostic> {
        self.take(kind).ok_or_else(|| self.error_current(message))
    }

    fn current_token(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn error_current(&self, message: &str) -> Diagnostic {
        Diagnostic::source(self.source, self.current_token().span, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    #[test]
    fn parses_spanned_functions_blocks_calls_and_literals() {
        let text = "fn helper(value str) str { print(value); value }";
        let program = parse(&source(text)).unwrap();
        assert_eq!(program.span, Span::new(0, text.len()));
        let Declaration::Function(function) = &program.declarations[0];
        assert_eq!(function.span, Span::new(0, text.len()));
        assert_eq!(function.name.span, Span::new(3, 9));
        assert_eq!(function.parameters[0].span, Span::new(10, 19));
        assert_eq!(function.body.statements.len(), 1);
        assert!(function.body.value.is_some());
    }

    #[test]
    fn parses_all_entry_point_signature_syntax_without_contextual_validation() {
        for text in [
            "fn main() {}",
            "fn main() int {}",
            "fn main(args [str]) {}",
            "fn main(args [str]) int {}",
        ] {
            assert!(parse(&source(text)).is_ok(), "{text}");
        }
    }

    #[test]
    fn parses_chained_calls_and_nested_blocks() {
        let program = parse(&source("fn f() { { print(make(\"x\")); } }")).unwrap();
        let Declaration::Function(function) = &program.declarations[0];
        assert!(matches!(
            function.body.statements[0].kind,
            StatementKind::Block(_)
        ));
    }

    #[test]
    fn rejects_temporary_top_level_statements() {
        let diagnostics = parse(&source("print(\"old syntax\");")).unwrap_err();
        assert!(diagnostics
            .to_string()
            .contains("expected function declaration"));
    }
}
