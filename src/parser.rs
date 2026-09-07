//! Permanent declaration and type parser through Phase 4.
//!
//! Later phases extend these routines with the remaining expression and
//! statement productions. Entry-point rules are deliberately enforced by
//! `compiler`, not here.

use std::mem::discriminant;

use crate::ast::{
    Block, Declaration, Expression, ExpressionKind, FunctionDeclaration, Identifier, Parameter,
    PrimitiveType, Program, Statement, StatementKind, Type, TypeDeclaration, TypeKind, TypeMember,
    TypeMemberKind,
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
            let declaration = if self.at(&TokenKind::Type) {
                Declaration::Type(self.parse_type_declaration()?)
            } else if self.at(&TokenKind::Fn) {
                Declaration::Function(self.parse_function()?)
            } else {
                return Err(self.error_current("expected top-level 'type' or 'fn' declaration"));
            };
            declarations.push(declaration);
        }
        Ok(Program {
            declarations,
            span: Span::new(0, self.source.text.len()),
        })
    }

    fn parse_type_declaration(&mut self) -> Result<TypeDeclaration, Diagnostic> {
        let start = self
            .expect(
                &TokenKind::Type,
                "expected type declaration beginning with 'type'",
            )?
            .span
            .start;
        let name = self.parse_identifier("expected type name after 'type'")?;
        self.expect(&TokenKind::LeftParen, "expected '(' after type name")?;

        let mut members = Vec::new();
        if self.at(&TokenKind::RightParen) {
            return Err(self.error_current("expected at least one type member"));
        }
        loop {
            members.push(self.parse_type_member()?);
            if self.take(&TokenKind::Comma).is_none() {
                break;
            }
        }

        self.expect(
            &TokenKind::RightParen,
            "expected ')' after type declaration members",
        )?;
        let end = self
            .expect(&TokenKind::Semicolon, "expected ';' after type declaration")?
            .span
            .end;
        Ok(TypeDeclaration {
            name,
            members,
            span: Span::new(start, end),
        })
    }

    fn parse_type_member(&mut self) -> Result<TypeMember, Diagnostic> {
        if self.at(&TokenKind::Identifier)
            && (self.at_next(&TokenKind::Ampersand) || self.at_next_unambiguous_type_start())
        {
            let name = self.parse_identifier("expected member name")?;
            let referenced = self.take(&TokenKind::Ampersand).is_some();
            let ty = self.parse_type()?;
            return Ok(TypeMember {
                span: Span::new(name.span.start, ty.span.end),
                kind: TypeMemberKind::Named {
                    name,
                    referenced,
                    ty,
                },
            });
        }

        let ty = self.parse_type()?;
        Ok(TypeMember {
            span: ty.span,
            kind: TypeMemberKind::Unnamed(ty),
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

        let return_type = if self.at(&TokenKind::LeftBrace) {
            self.try_parse_map_return_type()?
        } else if self.at_type_start() {
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

    fn try_parse_map_return_type(&mut self) -> Result<Option<Type>, Diagnostic> {
        let checkpoint = self.current;
        let parsed = self.parse_type();
        if let Ok(ty) = parsed
            && self.at(&TokenKind::LeftBrace)
        {
            return Ok(Some(ty));
        }
        self.current = checkpoint;
        Ok(None)
    }

    fn parse_type(&mut self) -> Result<Type, Diagnostic> {
        let first = self.parse_type_atom()?;
        if self.take(&TokenKind::Pipe).is_none() {
            return Ok(first);
        }

        let start = first.span.start;
        let mut alternatives = vec![first];
        loop {
            alternatives.push(self.parse_type_atom()?);
            if self.take(&TokenKind::Pipe).is_none() {
                break;
            }
        }
        let end = alternatives.last().unwrap().span.end;
        Ok(Type {
            kind: TypeKind::Union(alternatives.into_boxed_slice()),
            span: Span::new(start, end),
        })
    }

    fn parse_type_atom(&mut self) -> Result<Type, Diagnostic> {
        let token = self.current_token().clone();
        let (kind, span) = match token.kind {
            TokenKind::Int => (TypeKind::Primitive(PrimitiveType::Int), token.span),
            TokenKind::FloatType => (TypeKind::Primitive(PrimitiveType::Float), token.span),
            TokenKind::Str => (TypeKind::Primitive(PrimitiveType::Str), token.span),
            TokenKind::Bool => (TypeKind::Primitive(PrimitiveType::Bool), token.span),
            TokenKind::Char => (TypeKind::Primitive(PrimitiveType::Char), token.span),
            TokenKind::Identifier => {
                self.current += 1;
                let identifier = Identifier { span: token.span };
                if self.take(&TokenKind::LeftParen).is_some() {
                    let payload = self.parse_type()?;
                    let end = self
                        .expect(
                            &TokenKind::RightParen,
                            "expected ')' after tagged alternative payload",
                        )?
                        .span
                        .end;
                    return Ok(Type {
                        kind: TypeKind::Tagged {
                            tag: identifier,
                            payload: Box::new(payload),
                        },
                        span: Span::new(token.span.start, end),
                    });
                }
                return Ok(Type {
                    kind: TypeKind::Named(identifier),
                    span: token.span,
                });
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
            TokenKind::LeftBrace => {
                self.current += 1;
                let key = self.parse_type()?;
                self.expect(
                    &TokenKind::Colon,
                    "expected ':' between map key and value types",
                )?;
                let value = self.parse_type()?;
                let end = self
                    .expect(&TokenKind::RightBrace, "expected '}' after map type")?
                    .span
                    .end;
                return Ok(Type {
                    kind: TypeKind::Map {
                        key: Box::new(key),
                        value: Box::new(value),
                    },
                    span: Span::new(token.span.start, end),
                });
            }
            TokenKind::LeftParen => {
                self.current += 1;
                let inner = self.parse_type()?;
                let end = self
                    .expect(
                        &TokenKind::RightParen,
                        "expected ')' after parenthesized type",
                    )?
                    .span
                    .end;
                return Ok(Type {
                    kind: TypeKind::Parenthesized(Box::new(inner)),
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
            || self.at(&TokenKind::LeftBrace)
            || self.at(&TokenKind::LeftParen)
    }

    fn at_next_unambiguous_type_start(&self) -> bool {
        self.at_offset(1, &TokenKind::Int)
            || self.at_offset(1, &TokenKind::FloatType)
            || self.at_offset(1, &TokenKind::Str)
            || self.at_offset(1, &TokenKind::Bool)
            || self.at_offset(1, &TokenKind::Char)
            || self.at_offset(1, &TokenKind::Identifier)
            || self.at_offset(1, &TokenKind::LeftBracket)
            || self.at_offset(1, &TokenKind::LeftBrace)
    }

    fn at_next(&self, kind: &TokenKind) -> bool {
        self.at_offset(1, kind)
    }

    fn at_offset(&self, offset: usize, kind: &TokenKind) -> bool {
        self.tokens
            .get(self.current + offset)
            .is_some_and(|token| discriminant(&token.kind) == discriminant(kind))
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

    fn span_text(text: &str, span: Span) -> &str {
        &text[span.start..span.end]
    }

    #[test]
    fn parses_spanned_functions_blocks_calls_and_literals() {
        let text = "fn helper(value str) str { print(value); value }";
        let program = parse(&source(text)).unwrap();
        assert_eq!(program.span, Span::new(0, text.len()));
        let Declaration::Function(function) = &program.declarations[0] else {
            panic!("expected function declaration");
        };
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
        let Declaration::Function(function) = &program.declarations[0] else {
            panic!("expected function declaration");
        };
        assert!(matches!(
            function.body.statements[0].kind,
            StatementKind::Block(_)
        ));
    }

    #[test]
    fn rejects_temporary_top_level_statements() {
        let diagnostics = parse(&source("print(\"old syntax\");")).unwrap_err();
        assert!(
            diagnostics
                .to_string()
                .contains("expected top-level 'type' or 'fn' declaration")
        );
    }

    #[test]
    fn parses_named_unnamed_and_referenced_type_members() {
        let text =
            "type Point(x int, y float, target Target, shared &Target); type Pair(int, [str]);";
        let program = parse(&source(text)).unwrap();
        assert_eq!(program.declarations.len(), 2);

        let Declaration::Type(point) = &program.declarations[0] else {
            panic!("expected type declaration");
        };
        assert_eq!(
            span_text(text, point.span),
            "type Point(x int, y float, target Target, shared &Target);"
        );
        assert_eq!(span_text(text, point.name.span), "Point");
        assert_eq!(point.members.len(), 4);
        let TypeMemberKind::Named {
            name,
            referenced,
            ty,
        } = &point.members[3].kind
        else {
            panic!("expected named member");
        };
        assert_eq!(span_text(text, name.span), "shared");
        assert!(*referenced);
        assert_eq!(span_text(text, ty.span), "Target");
        assert_eq!(span_text(text, point.members[3].span), "shared &Target");

        let Declaration::Type(pair) = &program.declarations[1] else {
            panic!("expected type declaration");
        };
        assert!(
            pair.members
                .iter()
                .all(|member| matches!(member.kind, TypeMemberKind::Unnamed(_)))
        );
    }

    #[test]
    fn parses_every_type_atom_and_union_in_function_signatures() {
        let text = "fn types(a int, b float, c str, d bool, e char, f Name, g [Name], h {str: [int]}, i (A | B), j Ok(int) | Error(str)) (A | B) | C {}";
        let program = parse(&source(text)).unwrap();
        let Declaration::Function(function) = &program.declarations[0] else {
            panic!("expected function declaration");
        };

        assert_eq!(function.parameters.len(), 10);
        assert!(matches!(
            function.parameters[0].ty.kind,
            TypeKind::Primitive(PrimitiveType::Int)
        ));
        assert!(matches!(function.parameters[5].ty.kind, TypeKind::Named(_)));
        assert!(matches!(function.parameters[6].ty.kind, TypeKind::List(_)));
        assert!(matches!(
            function.parameters[7].ty.kind,
            TypeKind::Map { .. }
        ));
        assert!(matches!(
            function.parameters[8].ty.kind,
            TypeKind::Parenthesized(_)
        ));
        let TypeKind::Union(tagged) = &function.parameters[9].ty.kind else {
            panic!("expected tagged union");
        };
        assert_eq!(tagged.len(), 2);
        assert!(
            tagged
                .iter()
                .all(|alternative| matches!(alternative.kind, TypeKind::Tagged { .. }))
        );
        assert!(matches!(
            function.return_type.as_ref().unwrap().kind,
            TypeKind::Union(_)
        ));
    }

    #[test]
    fn preserves_parenthesized_nested_unions() {
        let text = "type Nested((int | float) | str);";
        let program = parse(&source(text)).unwrap();
        let Declaration::Type(declaration) = &program.declarations[0] else {
            panic!("expected type declaration");
        };
        let TypeMemberKind::Unnamed(ty) = &declaration.members[0].kind else {
            panic!("expected unnamed member");
        };
        let TypeKind::Union(outer) = &ty.kind else {
            panic!("expected outer union");
        };
        assert_eq!(outer.len(), 2);
        let TypeKind::Parenthesized(inner) = &outer[0].kind else {
            panic!("expected preserved parentheses");
        };
        assert!(matches!(inner.kind, TypeKind::Union(ref types) if types.len() == 2));
        assert_eq!(span_text(text, outer[0].span), "(int | float)");
        assert_eq!(span_text(text, inner.span), "int | float");
    }

    #[test]
    fn distinguishes_map_return_types_from_function_bodies() {
        let map_source = source("fn lookup() {str: [int]} {}");
        let program = parse(&map_source).unwrap();
        let Declaration::Function(function) = &program.declarations[0] else {
            panic!("expected function declaration");
        };
        assert!(matches!(
            function.return_type.as_ref().unwrap().kind,
            TypeKind::Map { .. }
        ));

        let body_source = source("fn no_return() {}");
        let program = parse(&body_source).unwrap();
        let Declaration::Function(function) = &program.declarations[0] else {
            panic!("expected function declaration");
        };
        assert!(function.return_type.is_none());
    }

    #[test]
    fn defers_contextual_type_member_validation() {
        for text in [
            "type Mixed(a int, float);",
            "type ReferencedPrimitive(value &int);",
            "type SingleAlternative(int);",
            "type TaggedOutsideUnion(Only(int));",
        ] {
            assert!(parse(&source(text)).is_ok(), "{text}");
        }
    }
}
