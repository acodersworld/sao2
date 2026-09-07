//! Permanent declaration, type, expression, and statement parser through Phase 6.
//!
//! The remaining parser phase adds multi-error recovery and conformance
//! fixtures. Entry-point rules are deliberately enforced by `compiler`, not
//! here.

use std::mem::discriminant;

use crate::ast::{
    Argument, ArgumentKind, AssignmentOperator, AssignmentTarget, AssignmentTargetSuffix,
    AssignmentTargetSuffixKind, BinaryOperator, Block, ConditionalExpressionBranch,
    ConditionalStatementBranch, Declaration, Expression, ExpressionBody, ExpressionBodyKind,
    ExpressionKind, FunctionDeclaration, Identifier, MapEntry, Member, Parameter, PrimitiveType,
    Program, Statement, StatementBody, StatementBodyKind, StatementKind, SwitchArm, Type,
    TypeDeclaration, TypeKind, TypeMember, TypeMemberKind, UnaryOperator,
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
                return self.parse_list_type();
            }
            TokenKind::LeftBrace => {
                return self.parse_map_type();
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

    fn parse_list_type(&mut self) -> Result<Type, Diagnostic> {
        let start = self
            .expect(&TokenKind::LeftBracket, "expected '[' to begin list type")?
            .span
            .start;
        let element = self.parse_type()?;
        let end = self
            .expect(&TokenKind::RightBracket, "expected ']' after list type")?
            .span
            .end;
        Ok(Type {
            kind: TypeKind::List(Box::new(element)),
            span: Span::new(start, end),
        })
    }

    fn parse_map_type(&mut self) -> Result<Type, Diagnostic> {
        let start = self
            .expect(&TokenKind::LeftBrace, "expected '{' to begin map type")?
            .span
            .start;
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
        Ok(Type {
            kind: TypeKind::Map {
                key: Box::new(key),
                value: Box::new(value),
            },
            span: Span::new(start, end),
        })
    }

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let start = self
            .expect(&TokenKind::LeftBrace, "expected block beginning with '{'")?
            .span
            .start;
        let mut statements = Vec::new();
        let mut value = None;

        while !self.at(&TokenKind::RightBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(self.error_current("expected '}' to close block"));
            }

            if self.at_non_expression_statement_start()
                || (self.at(&TokenKind::Identifier) && self.at_next(&TokenKind::Declare))
            {
                statements.push(self.parse_statement()?);
                continue;
            }

            let expression = self.parse_expression()?;
            if let Some((operator, operator_span)) = self.take_assignment_operator() {
                statements.push(self.finish_assignment_statement(
                    expression,
                    operator,
                    operator_span,
                )?);
            } else if let Some(semicolon) = self.take(&TokenKind::Semicolon) {
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

    fn parse_statement(&mut self) -> Result<Statement, Diagnostic> {
        match self.current_token().kind {
            TokenKind::Var => self.parse_local_declaration(),
            TokenKind::Identifier if self.at_next(&TokenKind::Declare) => {
                self.parse_local_declaration()
            }
            TokenKind::Return => self.parse_return_statement(),
            TokenKind::Break => self.parse_keyword_statement(false),
            TokenKind::Continue => self.parse_keyword_statement(true),
            TokenKind::If => self.parse_if_statement(),
            TokenKind::While => self.parse_while_statement(),
            TokenKind::For => self.parse_for_statement(),
            TokenKind::Switch => self.parse_switch_statement(),
            TokenKind::LeftBrace => {
                let block = self.parse_block()?;
                let span = block.span;
                Ok(Statement {
                    kind: StatementKind::Block(block),
                    span,
                })
            }
            _ => {
                let expression = self.parse_expression()?;
                if let Some((operator, operator_span)) = self.take_assignment_operator() {
                    self.finish_assignment_statement(expression, operator, operator_span)
                } else {
                    let semicolon = self.expect(
                        &TokenKind::Semicolon,
                        "expected ';' after expression statement",
                    )?;
                    Ok(Statement {
                        span: Span::new(expression.span.start, semicolon.span.end),
                        kind: StatementKind::Expression(expression),
                    })
                }
            }
        }
    }

    fn at_non_expression_statement_start(&self) -> bool {
        self.at(&TokenKind::Var)
            || self.at(&TokenKind::Return)
            || self.at(&TokenKind::Break)
            || self.at(&TokenKind::Continue)
            || self.at(&TokenKind::If)
            || self.at(&TokenKind::While)
            || self.at(&TokenKind::For)
            || self.at(&TokenKind::Switch)
            || self.at(&TokenKind::LeftBrace)
    }

    fn parse_local_declaration(&mut self) -> Result<Statement, Diagnostic> {
        let mutable_token = self.take(&TokenKind::Var);
        let name = self.parse_identifier("expected local name")?;
        let start = mutable_token
            .as_ref()
            .map_or(name.span.start, |token| token.span.start);
        self.expect(&TokenKind::Declare, "expected ':=' after local name")?;
        let initializer = self.parse_expression()?;
        let end = self
            .expect(
                &TokenKind::Semicolon,
                "expected ';' after local declaration",
            )?
            .span
            .end;
        Ok(Statement {
            kind: StatementKind::Local {
                mutable: mutable_token.is_some(),
                name,
                initializer,
            },
            span: Span::new(start, end),
        })
    }

    fn take_assignment_operator(&mut self) -> Option<(AssignmentOperator, Span)> {
        let operator = match self.current_token().kind {
            TokenKind::Equal => AssignmentOperator::Assign,
            TokenKind::PlusEqual => AssignmentOperator::Add,
            TokenKind::MinusEqual => AssignmentOperator::Subtract,
            TokenKind::StarEqual => AssignmentOperator::Multiply,
            TokenKind::SlashEqual => AssignmentOperator::Divide,
            TokenKind::PercentEqual => AssignmentOperator::Remainder,
            TokenKind::AmpersandEqual => AssignmentOperator::BitwiseAnd,
            TokenKind::PipeEqual => AssignmentOperator::BitwiseOr,
            TokenKind::CaretEqual => AssignmentOperator::BitwiseXor,
            TokenKind::ShiftLeftEqual => AssignmentOperator::ShiftLeft,
            TokenKind::ShiftRightEqual => AssignmentOperator::ShiftRight,
            _ => return None,
        };
        let span = self.current_token().span;
        self.current += 1;
        Some((operator, span))
    }

    fn finish_assignment_statement(
        &mut self,
        expression: Expression,
        operator: AssignmentOperator,
        operator_span: Span,
    ) -> Result<Statement, Diagnostic> {
        let expression_span = expression.span;
        let Some(target) = Self::assignment_target_from_expression(expression) else {
            return Err(Diagnostic::source(
                self.source,
                expression_span,
                "assignment target must begin with an identifier and contain only member or index access",
            ));
        };
        let value = self.parse_expression()?;
        let end = self
            .expect(&TokenKind::Semicolon, "expected ';' after assignment")?
            .span
            .end;
        Ok(Statement {
            span: Span::new(target.span.start, end),
            kind: StatementKind::Assignment {
                target,
                operator,
                operator_span,
                value,
            },
        })
    }

    fn assignment_target_from_expression(expression: Expression) -> Option<AssignmentTarget> {
        let span = expression.span;
        let mut suffixes = Vec::new();
        let root = Self::collect_assignment_target(expression, &mut suffixes)?;
        Some(AssignmentTarget {
            root,
            suffixes,
            span,
        })
    }

    fn collect_assignment_target(
        expression: Expression,
        suffixes: &mut Vec<AssignmentTargetSuffix>,
    ) -> Option<Identifier> {
        let expression_span = expression.span;
        match expression.kind {
            ExpressionKind::Identifier(identifier) => Some(identifier),
            ExpressionKind::Member { value, member } => {
                let suffix_start = value.span.end;
                let root = Self::collect_assignment_target(*value, suffixes)?;
                suffixes.push(AssignmentTargetSuffix {
                    kind: AssignmentTargetSuffixKind::Member(member),
                    span: Span::new(suffix_start, expression_span.end),
                });
                Some(root)
            }
            ExpressionKind::Index { value, index } => {
                let suffix_start = value.span.end;
                let root = Self::collect_assignment_target(*value, suffixes)?;
                suffixes.push(AssignmentTargetSuffix {
                    kind: AssignmentTargetSuffixKind::Index(*index),
                    span: Span::new(suffix_start, expression_span.end),
                });
                Some(root)
            }
            _ => None,
        }
    }

    fn parse_return_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self
            .expect(&TokenKind::Return, "expected 'return'")?
            .span
            .start;
        let value = if self.at(&TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expression()?)
        };
        let end = self
            .expect(&TokenKind::Semicolon, "expected ';' after return")?
            .span
            .end;
        Ok(Statement {
            kind: StatementKind::Return(value),
            span: Span::new(start, end),
        })
    }

    fn parse_keyword_statement(&mut self, is_continue: bool) -> Result<Statement, Diagnostic> {
        let (keyword, message, kind) = if is_continue {
            (
                TokenKind::Continue,
                "expected 'continue'",
                StatementKind::Continue,
            )
        } else {
            (TokenKind::Break, "expected 'break'", StatementKind::Break)
        };
        let start = self.expect(&keyword, message)?.span.start;
        let end = self
            .expect(
                &TokenKind::Semicolon,
                "expected ';' after loop control statement",
            )?
            .span
            .end;
        Ok(Statement {
            kind,
            span: Span::new(start, end),
        })
    }

    fn parse_if_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(&TokenKind::If, "expected 'if'")?.span.start;
        let condition = self.parse_expression()?;
        let body = self.parse_statement_body()?;
        let first_span = Span::new(start, body.span.end);
        let mut branches = vec![ConditionalStatementBranch {
            condition,
            body,
            span: first_span,
        }];

        let mut else_body = None;
        while let Some(else_token) = self.take(&TokenKind::Else) {
            if self.take(&TokenKind::If).is_some() {
                let condition = self.parse_expression()?;
                let body = self.parse_statement_body()?;
                let span = Span::new(else_token.span.start, body.span.end);
                branches.push(ConditionalStatementBranch {
                    condition,
                    body,
                    span,
                });
            } else {
                else_body = Some(self.parse_statement_body()?);
                break;
            }
        }

        let end = else_body
            .as_ref()
            .map_or_else(|| branches.last().unwrap().span.end, |body| body.span.end);
        Ok(Statement {
            kind: StatementKind::If {
                branches,
                else_body,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_while_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self
            .expect(&TokenKind::While, "expected 'while'")?
            .span
            .start;
        let condition = self.parse_expression()?;
        let body = self.parse_statement_body()?;
        let span = Span::new(start, body.span.end);
        Ok(Statement {
            kind: StatementKind::While { condition, body },
            span,
        })
    }

    fn parse_for_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self.expect(&TokenKind::For, "expected 'for'")?.span.start;
        let binding = self.parse_identifier("expected iteration binding after 'for'")?;
        self.expect(&TokenKind::In, "expected 'in' after iteration binding")?;
        let iterable = self.parse_expression()?;
        let body = self.parse_statement_body()?;
        let span = Span::new(start, body.span.end);
        Ok(Statement {
            kind: StatementKind::For {
                binding,
                iterable,
                body,
            },
            span,
        })
    }

    fn parse_switch_statement(&mut self) -> Result<Statement, Diagnostic> {
        let start = self
            .expect(&TokenKind::Switch, "expected 'switch'")?
            .span
            .start;
        let value = self.parse_expression()?;
        self.expect(
            &TokenKind::LeftBrace,
            "expected '{' after switched expression",
        )?;
        let mut arms = Vec::new();
        let mut else_body = None;

        while !self.at(&TokenKind::RightBrace) {
            if self.at(&TokenKind::Eof) {
                return Err(self.error_current("expected '}' to close switch"));
            }
            if self.take(&TokenKind::Else).is_some() {
                else_body = Some(self.parse_statement_body()?);
                if !self.at(&TokenKind::RightBrace) {
                    return Err(self.error_current("else must be the final switch arm"));
                }
                break;
            }

            let label = self.parse_type()?;
            let body = self.parse_statement_body()?;
            let span = Span::new(label.span.start, body.span.end);
            arms.push(SwitchArm { label, body, span });
        }

        let end = self
            .expect(&TokenKind::RightBrace, "expected '}' to close switch")?
            .span
            .end;
        Ok(Statement {
            kind: StatementKind::Switch {
                value,
                arms,
                else_body,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_statement_body(&mut self) -> Result<StatementBody, Diagnostic> {
        if self.at(&TokenKind::LeftBrace) {
            let block = self.parse_block()?;
            let span = block.span;
            return Ok(StatementBody {
                kind: StatementBodyKind::Block(block),
                span,
            });
        }

        let colon = self.expect(&TokenKind::Colon, "expected block or ':' statement body")?;
        let statement = self.parse_statement()?;
        Ok(StatementBody {
            span: Span::new(colon.span.start, statement.span.end),
            kind: StatementBodyKind::Statement(Box::new(statement)),
        })
    }

    fn parse_expression(&mut self) -> Result<Expression, Diagnostic> {
        self.parse_binary_expression(1)
    }

    fn parse_binary_expression(
        &mut self,
        minimum_precedence: u8,
    ) -> Result<Expression, Diagnostic> {
        let mut left = self.parse_unary_expression()?;

        loop {
            if self.at(&TokenKind::Is) {
                const IS_PRECEDENCE: u8 = 7;
                if IS_PRECEDENCE < minimum_precedence {
                    break;
                }
                let operator_span = self.current_token().span;
                self.current += 1;
                let ty = self.parse_type()?;
                let span = Span::new(left.span.start, ty.span.end);
                left = Expression {
                    kind: ExpressionKind::Is {
                        value: Box::new(left),
                        operator_span,
                        ty,
                    },
                    span,
                };
                continue;
            }

            let Some((operator, precedence)) = self.current_binary_operator() else {
                break;
            };
            if precedence < minimum_precedence {
                break;
            }
            let operator_span = self.current_token().span;
            self.current += 1;
            let right = self.parse_binary_expression(precedence + 1)?;
            let span = Span::new(left.span.start, right.span.end);
            left = Expression {
                kind: ExpressionKind::Binary {
                    left: Box::new(left),
                    operator,
                    operator_span,
                    right: Box::new(right),
                },
                span,
            };
        }

        Ok(left)
    }

    fn current_binary_operator(&self) -> Option<(BinaryOperator, u8)> {
        let operator = match self.current_token().kind {
            TokenKind::LogicalOr => (BinaryOperator::LogicalOr, 1),
            TokenKind::LogicalAnd => (BinaryOperator::LogicalAnd, 2),
            TokenKind::Pipe => (BinaryOperator::BitwiseOr, 3),
            TokenKind::Caret => (BinaryOperator::BitwiseXor, 4),
            TokenKind::Ampersand => (BinaryOperator::BitwiseAnd, 5),
            TokenKind::EqualEqual => (BinaryOperator::Equal, 6),
            TokenKind::BangEqual => (BinaryOperator::NotEqual, 6),
            TokenKind::Less => (BinaryOperator::Less, 7),
            TokenKind::LessEqual => (BinaryOperator::LessEqual, 7),
            TokenKind::Greater => (BinaryOperator::Greater, 7),
            TokenKind::GreaterEqual => (BinaryOperator::GreaterEqual, 7),
            TokenKind::In => (BinaryOperator::In, 7),
            TokenKind::ShiftLeft => (BinaryOperator::ShiftLeft, 8),
            TokenKind::ShiftRight => (BinaryOperator::ShiftRight, 8),
            TokenKind::Plus => (BinaryOperator::Add, 9),
            TokenKind::Minus => (BinaryOperator::Subtract, 9),
            TokenKind::Star => (BinaryOperator::Multiply, 10),
            TokenKind::Slash => (BinaryOperator::Divide, 10),
            TokenKind::Percent => (BinaryOperator::Remainder, 10),
            _ => return None,
        };
        Some(operator)
    }

    fn parse_unary_expression(&mut self) -> Result<Expression, Diagnostic> {
        let operator = match self.current_token().kind {
            TokenKind::Bang => UnaryOperator::LogicalNot,
            TokenKind::Tilde => UnaryOperator::BitwiseNot,
            TokenKind::Plus => UnaryOperator::Plus,
            TokenKind::Minus => UnaryOperator::Minus,
            _ => return self.parse_postfix_expression(),
        };
        let operator_span = self.current_token().span;
        self.current += 1;
        let operand = self.parse_unary_expression()?;
        let span = Span::new(operator_span.start, operand.span.end);
        Ok(Expression {
            kind: ExpressionKind::Unary {
                operator,
                operator_span,
                operand: Box::new(operand),
            },
            span,
        })
    }

    fn parse_postfix_expression(&mut self) -> Result<Expression, Diagnostic> {
        let mut expression = self.parse_primary()?;
        loop {
            if self.at(&TokenKind::LeftParen) {
                expression = self.parse_call_suffix(expression)?;
            } else if self.take(&TokenKind::LeftBracket).is_some() {
                let index = self.parse_expression()?;
                let end = self
                    .expect(
                        &TokenKind::RightBracket,
                        "expected ']' after index expression",
                    )?
                    .span
                    .end;
                let start = expression.span.start;
                expression = Expression {
                    kind: ExpressionKind::Index {
                        value: Box::new(expression),
                        index: Box::new(index),
                    },
                    span: Span::new(start, end),
                };
            } else if self.take(&TokenKind::Dot).is_some() {
                let token = self.current_token().clone();
                let member = match token.kind {
                    TokenKind::Identifier => {
                        self.current += 1;
                        Member::Named(Identifier { span: token.span })
                    }
                    TokenKind::Integer if self.integer_token_is_decimal(&token) => {
                        self.current += 1;
                        Member::TupleIndex(token.span)
                    }
                    _ => {
                        return Err(self.error_current(
                            "expected member name or decimal tuple index after '.'",
                        ));
                    }
                };
                let start = expression.span.start;
                expression = Expression {
                    kind: ExpressionKind::Member {
                        value: Box::new(expression),
                        member,
                    },
                    span: Span::new(start, token.span.end),
                };
            } else if let Some(question) = self.take(&TokenKind::Question) {
                let start = expression.span.start;
                expression = Expression {
                    kind: ExpressionKind::Try {
                        value: Box::new(expression),
                        operator_span: question.span,
                    },
                    span: Span::new(start, question.span.end),
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn parse_call_suffix(&mut self, callee: Expression) -> Result<Expression, Diagnostic> {
        self.expect(&TokenKind::LeftParen, "expected '(' to begin arguments")?;
        let mut arguments = Vec::new();
        if !self.at(&TokenKind::RightParen) {
            loop {
                arguments.push(self.parse_argument()?);
                if self.take(&TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        let end = self
            .expect(&TokenKind::RightParen, "expected ')' after call arguments")?
            .span
            .end;
        let start = callee.span.start;
        Ok(Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(callee),
                arguments,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_argument(&mut self) -> Result<Argument, Diagnostic> {
        if self.at(&TokenKind::Identifier) && self.at_next(&TokenKind::Equal) {
            let name = self.parse_identifier("expected named argument name")?;
            self.expect(&TokenKind::Equal, "expected '=' after named argument name")?;
            let value = self.parse_expression()?;
            return Ok(Argument {
                span: Span::new(name.span.start, value.span.end),
                kind: ArgumentKind::Named { name, value },
            });
        }

        let expression = self.parse_expression()?;
        Ok(Argument {
            span: expression.span,
            kind: ArgumentKind::Positional(expression),
        })
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
            TokenKind::Integer => {
                self.current += 1;
                Ok(Expression {
                    kind: ExpressionKind::Integer,
                    span: token.span,
                })
            }
            TokenKind::Float => {
                self.current += 1;
                Ok(Expression {
                    kind: ExpressionKind::Float,
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
            TokenKind::Character(value) => {
                self.current += 1;
                Ok(Expression {
                    kind: ExpressionKind::Character(value),
                    span: token.span,
                })
            }
            TokenKind::True | TokenKind::False => {
                self.current += 1;
                Ok(Expression {
                    kind: ExpressionKind::Boolean(matches!(token.kind, TokenKind::True)),
                    span: token.span,
                })
            }
            TokenKind::LeftParen => self.parse_parenthesized_expression(),
            TokenKind::LeftBracket => self.parse_list_expression(),
            TokenKind::LeftBrace => self.parse_braced_expression(),
            TokenKind::If => self.parse_if_expression(),
            _ => Err(self.error_current("expected expression")),
        }
    }

    fn parse_parenthesized_expression(&mut self) -> Result<Expression, Diagnostic> {
        let start = self
            .expect(
                &TokenKind::LeftParen,
                "expected '(' to begin parenthesized expression",
            )?
            .span
            .start;
        let inner = self.parse_expression()?;
        let end = self
            .expect(
                &TokenKind::RightParen,
                "expected ')' after parenthesized expression",
            )?
            .span
            .end;
        Ok(Expression {
            kind: ExpressionKind::Parenthesized(Box::new(inner)),
            span: Span::new(start, end),
        })
    }

    fn parse_list_expression(&mut self) -> Result<Expression, Diagnostic> {
        let start = self
            .expect(
                &TokenKind::LeftBracket,
                "expected '[' to begin list literal",
            )?
            .span
            .start;
        if let Some(close) = self.take(&TokenKind::RightBracket) {
            if self.take(&TokenKind::Colon).is_some() {
                let ty = self.parse_list_type()?;
                let span = Span::new(start, ty.span.end);
                return Ok(Expression {
                    kind: ExpressionKind::TypedEmptyList(ty),
                    span,
                });
            }
            return Ok(Expression {
                kind: ExpressionKind::List(Vec::new()),
                span: Span::new(start, close.span.end),
            });
        }

        let mut elements = Vec::new();
        loop {
            elements.push(self.parse_expression()?);
            if self.take(&TokenKind::Comma).is_none() {
                break;
            }
        }
        let end = self
            .expect(&TokenKind::RightBracket, "expected ']' after list elements")?
            .span
            .end;
        Ok(Expression {
            kind: ExpressionKind::List(elements),
            span: Span::new(start, end),
        })
    }

    fn parse_braced_expression(&mut self) -> Result<Expression, Diagnostic> {
        let checkpoint = self.current;
        let start = self
            .expect(
                &TokenKind::LeftBrace,
                "expected '{' to begin braced expression",
            )?
            .span
            .start;

        if let Some(close) = self.take(&TokenKind::RightBrace) {
            if self.take(&TokenKind::Colon).is_some() {
                let ty = self.parse_map_type()?;
                let span = Span::new(start, ty.span.end);
                return Ok(Expression {
                    kind: ExpressionKind::TypedEmptyMap(ty),
                    span,
                });
            }
            return Ok(Expression {
                kind: ExpressionKind::Map(Vec::new()),
                span: Span::new(start, close.span.end),
            });
        }

        let first = match self.parse_expression() {
            Ok(first) => first,
            Err(_) => {
                self.current = checkpoint;
                let block = self.parse_block()?;
                let span = block.span;
                return Ok(Expression {
                    kind: ExpressionKind::Block(block),
                    span,
                });
            }
        };
        if self.take(&TokenKind::Colon).is_some() {
            return self.parse_map_expression(start, first);
        }

        self.current = checkpoint;
        let block = self.parse_block()?;
        let span = block.span;
        Ok(Expression {
            kind: ExpressionKind::Block(block),
            span,
        })
    }

    fn parse_map_expression(
        &mut self,
        start: usize,
        first_key: Expression,
    ) -> Result<Expression, Diagnostic> {
        let first_value = self.parse_expression()?;
        let first_span = Span::new(first_key.span.start, first_value.span.end);
        let mut entries = vec![MapEntry {
            key: first_key,
            value: first_value,
            span: first_span,
        }];

        while self.take(&TokenKind::Comma).is_some() {
            let key = self.parse_expression()?;
            self.expect(&TokenKind::Colon, "expected ':' between map key and value")?;
            let value = self.parse_expression()?;
            let span = Span::new(key.span.start, value.span.end);
            entries.push(MapEntry { key, value, span });
        }
        let end = self
            .expect(&TokenKind::RightBrace, "expected '}' after map entries")?
            .span
            .end;
        Ok(Expression {
            kind: ExpressionKind::Map(entries),
            span: Span::new(start, end),
        })
    }

    fn parse_if_expression(&mut self) -> Result<Expression, Diagnostic> {
        let start = self.expect(&TokenKind::If, "expected 'if'")?.span.start;
        let condition = self.parse_expression()?;
        let body = self.parse_expression_body()?;
        let first_span = Span::new(start, body.span.end);
        let mut branches = vec![ConditionalExpressionBranch {
            condition,
            body,
            span: first_span,
        }];

        loop {
            let else_token = self.expect(
                &TokenKind::Else,
                "if expression requires a final 'else' branch",
            )?;
            if self.take(&TokenKind::If).is_some() {
                let condition = self.parse_expression()?;
                let body = self.parse_expression_body()?;
                let span = Span::new(else_token.span.start, body.span.end);
                branches.push(ConditionalExpressionBranch {
                    condition,
                    body,
                    span,
                });
            } else {
                let else_branch = self.parse_expression_body()?;
                let span = Span::new(start, else_branch.span.end);
                return Ok(Expression {
                    kind: ExpressionKind::If {
                        branches,
                        else_branch,
                    },
                    span,
                });
            }
        }
    }

    fn parse_expression_body(&mut self) -> Result<ExpressionBody, Diagnostic> {
        if self.at(&TokenKind::LeftBrace) {
            let block = self.parse_block()?;
            let span = block.span;
            return Ok(ExpressionBody {
                kind: ExpressionBodyKind::Block(block),
                span,
            });
        }

        let colon = self.expect(&TokenKind::Colon, "expected block or ':' expression body")?;
        let expression = self.parse_expression()?;
        Ok(ExpressionBody {
            span: Span::new(colon.span.start, expression.span.end),
            kind: ExpressionBodyKind::Expression(Box::new(expression)),
        })
    }

    fn integer_token_is_decimal(&self, token: &Token) -> bool {
        let text = &self.source.text[token.span.start..token.span.end];
        !text.starts_with("0x") && !text.starts_with("0b")
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

    fn value_expression(text: &str) -> Expression {
        let program = parse(&source(&format!("fn f() {{ {text} }}"))).unwrap();
        let Declaration::Function(function) = program.declarations.into_iter().next().unwrap()
        else {
            panic!("expected function declaration");
        };
        *function.body.value.unwrap()
    }

    fn function_declaration(text: &str) -> FunctionDeclaration {
        let program = parse(&source(text)).unwrap();
        let Declaration::Function(function) = program.declarations.into_iter().next().unwrap()
        else {
            panic!("expected function declaration");
        };
        function
    }

    fn binary_parts(
        expression: &Expression,
        expected: BinaryOperator,
    ) -> (&Expression, &Expression) {
        let ExpressionKind::Binary {
            left,
            operator,
            right,
            ..
        } = &expression.kind
        else {
            panic!("expected {expected:?}, got {expression:?}");
        };
        assert_eq!(*operator, expected);
        (left, right)
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

    #[test]
    fn parses_every_primitive_expression_literal() {
        for (text, expected) in [
            ("42", ExpressionKind::Integer),
            ("1.5", ExpressionKind::Float),
            ("\"value\"", ExpressionKind::String(b"value".to_vec())),
            ("'x'", ExpressionKind::Character(b'x')),
            ("true", ExpressionKind::Boolean(true)),
            ("false", ExpressionKind::Boolean(false)),
        ] {
            assert_eq!(value_expression(text).kind, expected, "{text}");
        }
    }

    #[test]
    fn applies_all_binary_precedence_levels() {
        let expression = value_expression("a || b && c | d ^ e & f == g < h << i + j * k");
        let (_, right) = binary_parts(&expression, BinaryOperator::LogicalOr);
        let (_, right) = binary_parts(right, BinaryOperator::LogicalAnd);
        let (_, right) = binary_parts(right, BinaryOperator::BitwiseOr);
        let (_, right) = binary_parts(right, BinaryOperator::BitwiseXor);
        let (_, right) = binary_parts(right, BinaryOperator::BitwiseAnd);
        let (_, right) = binary_parts(right, BinaryOperator::Equal);
        let (_, right) = binary_parts(right, BinaryOperator::Less);
        let (_, right) = binary_parts(right, BinaryOperator::ShiftLeft);
        let (_, right) = binary_parts(right, BinaryOperator::Add);
        binary_parts(right, BinaryOperator::Multiply);
    }

    #[test]
    fn parses_every_binary_operator_and_left_associativity() {
        for (source, expected) in [
            ("a || b", BinaryOperator::LogicalOr),
            ("a && b", BinaryOperator::LogicalAnd),
            ("a | b", BinaryOperator::BitwiseOr),
            ("a ^ b", BinaryOperator::BitwiseXor),
            ("a & b", BinaryOperator::BitwiseAnd),
            ("a == b", BinaryOperator::Equal),
            ("a != b", BinaryOperator::NotEqual),
            ("a < b", BinaryOperator::Less),
            ("a <= b", BinaryOperator::LessEqual),
            ("a > b", BinaryOperator::Greater),
            ("a >= b", BinaryOperator::GreaterEqual),
            ("a in b", BinaryOperator::In),
            ("a << b", BinaryOperator::ShiftLeft),
            ("a >> b", BinaryOperator::ShiftRight),
            ("a + b", BinaryOperator::Add),
            ("a - b", BinaryOperator::Subtract),
            ("a * b", BinaryOperator::Multiply),
            ("a / b", BinaryOperator::Divide),
            ("a % b", BinaryOperator::Remainder),
        ] {
            binary_parts(&value_expression(source), expected);
        }

        let expression = value_expression("a - b - c");
        let (left, _) = binary_parts(&expression, BinaryOperator::Subtract);
        binary_parts(left, BinaryOperator::Subtract);
    }

    #[test]
    fn parses_recursive_unary_operators() {
        let expression = value_expression("!~-+value");
        let mut current = &expression;
        for expected in [
            UnaryOperator::LogicalNot,
            UnaryOperator::BitwiseNot,
            UnaryOperator::Minus,
            UnaryOperator::Plus,
        ] {
            let ExpressionKind::Unary {
                operator, operand, ..
            } = &current.kind
            else {
                panic!("expected unary expression");
            };
            assert_eq!(*operator, expected);
            current = operand;
        }
        assert!(matches!(current.kind, ExpressionKind::Identifier(_)));
    }

    #[test]
    fn parses_chained_postfix_operations_and_named_arguments() {
        let expression = value_expression("make(a, named = b)(c)[i].field.0??");
        let ExpressionKind::Try { value, .. } = &expression.kind else {
            panic!("expected postfix try");
        };
        let ExpressionKind::Try { value, .. } = &value.kind else {
            panic!("expected second postfix try");
        };
        let ExpressionKind::Member { value, member } = &value.kind else {
            panic!("expected tuple member");
        };
        assert!(matches!(member, Member::TupleIndex(_)));
        let ExpressionKind::Member { value, member } = &value.kind else {
            panic!("expected named member");
        };
        assert!(matches!(member, Member::Named(_)));
        let ExpressionKind::Index { value, .. } = &value.kind else {
            panic!("expected index expression");
        };
        let ExpressionKind::Call { callee, arguments } = &value.kind else {
            panic!("expected chained call");
        };
        assert_eq!(arguments.len(), 1);
        let ExpressionKind::Call { arguments, .. } = &callee.kind else {
            panic!("expected initial call");
        };
        assert_eq!(arguments.len(), 2);
        assert!(matches!(arguments[0].kind, ArgumentKind::Positional(_)));
        assert!(matches!(arguments[1].kind, ArgumentKind::Named { .. }));
    }

    #[test]
    fn parses_collection_literals_and_typed_empty_collections() {
        let expression = value_expression("[1, true, 'x']");
        assert!(matches!(expression.kind, ExpressionKind::List(ref values) if values.len() == 3));

        let expression = value_expression("({a: 1, b: 2})");
        let ExpressionKind::Parenthesized(inner) = expression.kind else {
            panic!("expected parentheses");
        };
        assert!(matches!(inner.kind, ExpressionKind::Map(ref entries) if entries.len() == 2));

        let expression = value_expression("[] : [int]");
        assert!(matches!(
            expression.kind,
            ExpressionKind::TypedEmptyList(Type {
                kind: TypeKind::List(_),
                ..
            })
        ));

        let expression = value_expression("({} : {str: int})");
        let ExpressionKind::Parenthesized(inner) = expression.kind else {
            panic!("expected parentheses");
        };
        assert!(matches!(
            inner.kind,
            ExpressionKind::TypedEmptyMap(Type {
                kind: TypeKind::Map { .. },
                ..
            })
        ));
    }

    #[test]
    fn distinguishes_map_and_block_expressions_by_colon_structure() {
        let map = value_expression("({key: value})");
        let ExpressionKind::Parenthesized(map) = map.kind else {
            panic!("expected parentheses");
        };
        assert!(matches!(map.kind, ExpressionKind::Map(_)));

        let block = value_expression("({value})");
        let ExpressionKind::Parenthesized(block) = block.kind else {
            panic!("expected parentheses");
        };
        let ExpressionKind::Block(block) = block.kind else {
            panic!("expected block expression");
        };
        assert!(block.value.is_some());

        let empty_map = value_expression("({})");
        let ExpressionKind::Parenthesized(empty_map) = empty_map.kind else {
            panic!("expected parentheses");
        };
        assert!(matches!(empty_map.kind, ExpressionKind::Map(ref entries) if entries.is_empty()));
    }

    #[test]
    fn parses_if_expressions_with_colon_and_block_bodies() {
        let expression = value_expression("(if first: one else if second { two } else: three)");
        let ExpressionKind::Parenthesized(expression) = expression.kind else {
            panic!("expected parentheses");
        };
        let ExpressionKind::If {
            branches,
            else_branch,
        } = expression.kind
        else {
            panic!("expected if expression");
        };
        assert_eq!(branches.len(), 2);
        assert!(matches!(
            branches[0].body.kind,
            ExpressionBodyKind::Expression(_)
        ));
        assert!(matches!(
            branches[1].body.kind,
            ExpressionBodyKind::Block(_)
        ));
        assert!(matches!(
            else_branch.kind,
            ExpressionBodyKind::Expression(_)
        ));
    }

    #[test]
    fn parses_is_with_a_type_and_rejects_assignment_expressions() {
        let expression = value_expression("value is Success(int) | Error(str)");
        let ExpressionKind::Is { ty, .. } = expression.kind else {
            panic!("expected type test");
        };
        assert!(matches!(ty.kind, TypeKind::Union(ref types) if types.len() == 2));

        let diagnostic = parse(&source("fn f() { (value = other) }")).unwrap_err();
        assert!(diagnostic.to_string().contains("expected ')'"));
        let diagnostic = parse(&source("fn f() { value.0x1 }")).unwrap_err();
        assert!(diagnostic.to_string().contains("decimal tuple index"));
    }

    #[test]
    fn parses_local_declarations_and_every_assignment_operator() {
        let function = function_declaration(
            "fn f() { value := initial; var mutable := other; target.field[index].0 += amount; }",
        );
        assert_eq!(function.body.statements.len(), 3);
        assert!(matches!(
            function.body.statements[0].kind,
            StatementKind::Local { mutable: false, .. }
        ));
        assert!(matches!(
            function.body.statements[1].kind,
            StatementKind::Local { mutable: true, .. }
        ));
        let StatementKind::Assignment {
            target, operator, ..
        } = &function.body.statements[2].kind
        else {
            panic!("expected assignment");
        };
        assert_eq!(*operator, AssignmentOperator::Add);
        assert_eq!(target.suffixes.len(), 3);
        assert!(matches!(
            target.suffixes[0].kind,
            AssignmentTargetSuffixKind::Member(Member::Named(_))
        ));
        assert!(matches!(
            target.suffixes[1].kind,
            AssignmentTargetSuffixKind::Index(_)
        ));
        assert!(matches!(
            target.suffixes[2].kind,
            AssignmentTargetSuffixKind::Member(Member::TupleIndex(_))
        ));

        for (symbol, expected) in [
            ("=", AssignmentOperator::Assign),
            ("+=", AssignmentOperator::Add),
            ("-=", AssignmentOperator::Subtract),
            ("*=", AssignmentOperator::Multiply),
            ("/=", AssignmentOperator::Divide),
            ("%=", AssignmentOperator::Remainder),
            ("&=", AssignmentOperator::BitwiseAnd),
            ("|=", AssignmentOperator::BitwiseOr),
            ("^=", AssignmentOperator::BitwiseXor),
            ("<<=", AssignmentOperator::ShiftLeft),
            (">>=", AssignmentOperator::ShiftRight),
        ] {
            let function = function_declaration(&format!("fn f() {{ target {symbol} value; }}"));
            assert!(matches!(
                function.body.statements[0].kind,
                StatementKind::Assignment {
                    operator,
                    ..
                } if operator == expected
            ));
        }
    }

    #[test]
    fn parses_expression_return_and_loop_control_statements() {
        let function = function_declaration(
            "fn f() { call(); return; return value; break; continue; final_value }",
        );
        assert_eq!(function.body.statements.len(), 5);
        assert!(matches!(
            function.body.statements[0].kind,
            StatementKind::Expression(_)
        ));
        assert!(matches!(
            function.body.statements[1].kind,
            StatementKind::Return(None)
        ));
        assert!(matches!(
            function.body.statements[2].kind,
            StatementKind::Return(Some(_))
        ));
        assert!(matches!(
            function.body.statements[3].kind,
            StatementKind::Break
        ));
        assert!(matches!(
            function.body.statements[4].kind,
            StatementKind::Continue
        ));
        assert!(function.body.value.is_some());
    }

    #[test]
    fn parses_if_statements_and_binds_else_to_the_nearest_if() {
        let function = function_declaration(
            "fn f() { if first { one(); } else if second: two(); else { three(); } if outer: if inner: one(); else: two(); }",
        );
        let StatementKind::If {
            branches,
            else_body,
        } = &function.body.statements[0].kind
        else {
            panic!("expected if statement");
        };
        assert_eq!(branches.len(), 2);
        assert!(matches!(branches[0].body.kind, StatementBodyKind::Block(_)));
        assert!(matches!(
            branches[1].body.kind,
            StatementBodyKind::Statement(_)
        ));
        assert!(matches!(
            else_body.as_ref().unwrap().kind,
            StatementBodyKind::Block(_)
        ));

        let StatementKind::If {
            branches,
            else_body,
        } = &function.body.statements[1].kind
        else {
            panic!("expected outer if statement");
        };
        assert!(else_body.is_none());
        let StatementBodyKind::Statement(inner) = &branches[0].body.kind else {
            panic!("expected colon-form inner statement");
        };
        assert!(matches!(
            inner.kind,
            StatementKind::If {
                else_body: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn parses_while_and_for_with_both_body_forms() {
        let function = function_declaration(
            "fn f() { while condition: continue; for item in items { break; } }",
        );
        let StatementKind::While { body, .. } = &function.body.statements[0].kind else {
            panic!("expected while statement");
        };
        assert!(matches!(body.kind, StatementBodyKind::Statement(_)));
        let StatementKind::For { body, .. } = &function.body.statements[1].kind else {
            panic!("expected for statement");
        };
        assert!(matches!(body.kind, StatementBodyKind::Block(_)));
    }

    #[test]
    fn parses_switch_arms_and_final_else_arm() {
        let function = function_declaration(
            "fn f() { switch value { int: first(); Ok(str) { second(); } else: fallback(); } }",
        );
        let StatementKind::Switch {
            arms, else_body, ..
        } = &function.body.statements[0].kind
        else {
            panic!("expected switch statement");
        };
        assert_eq!(arms.len(), 2);
        assert!(matches!(
            arms[0].label.kind,
            TypeKind::Primitive(PrimitiveType::Int)
        ));
        assert!(matches!(arms[1].label.kind, TypeKind::Tagged { .. }));
        assert!(matches!(arms[0].body.kind, StatementBodyKind::Statement(_)));
        assert!(matches!(arms[1].body.kind, StatementBodyKind::Block(_)));
        assert!(else_body.is_some());
    }

    #[test]
    fn keeps_final_block_values_separate_from_discarded_expressions() {
        let function = function_declaration("fn f() { discarded; value }");
        assert_eq!(function.body.statements.len(), 1);
        assert!(function.body.value.is_some());

        let function = function_declaration("fn f() { value; }");
        assert_eq!(function.body.statements.len(), 1);
        assert!(function.body.value.is_none());

        let function = function_declaration(
            "fn f() { result := { var value := initial; if condition: value = other; value }; }",
        );
        let StatementKind::Local { initializer, .. } = &function.body.statements[0].kind else {
            panic!("expected outer local declaration");
        };
        let ExpressionKind::Block(block) = &initializer.kind else {
            panic!("expected block expression initializer");
        };
        assert_eq!(block.statements.len(), 2);
        assert!(block.value.is_some());
    }

    #[test]
    fn parses_if_expressions_in_expression_context() {
        let function = function_declaration("fn f() { result := if condition: yes else: no; }");
        let StatementKind::Local { initializer, .. } = &function.body.statements[0].kind else {
            panic!("expected local declaration");
        };
        assert!(matches!(initializer.kind, ExpressionKind::If { .. }));
    }

    #[test]
    fn rejects_non_grammar_assignment_targets_and_nonfinal_switch_else() {
        for target in ["call()", "(value)", "value?"] {
            let diagnostic = parse(&source(&format!("fn f() {{ {target} = other; }}")))
                .unwrap_err()
                .to_string();
            assert!(
                diagnostic.contains("assignment target"),
                "{target}: {diagnostic}"
            );
        }

        let diagnostic = parse(&source(
            "fn f() { switch value { else: fallback(); int: unreachable(); } }",
        ))
        .unwrap_err()
        .to_string();
        assert!(diagnostic.contains("else must be the final switch arm"));
    }
}
