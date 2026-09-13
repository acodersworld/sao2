//! Temporary direct C emitter over the analyzed AST.
//!
//! It supports the milestone-4 primitive subset directly over resolved syntax.
//! Milestone 6 replaces this emitter with typed-IR lowering.

use std::fmt::Write;

use crate::analysis::{
    Analysis, BindingNode, CallableId, CallTarget, FunctionSignature, IntrinsicId, LiteralValue,
    NameResolution, TypeState,
};
use crate::ast::{
    ArgumentKind, AssignmentOperator, AssignmentTarget, BinaryOperator, Block, Declaration,
    Expression, ExpressionKind, PrimitiveType, Program, Statement, StatementKind, Type,
    UnaryOperator,
};
use crate::diagnostic::Diagnostic;
use crate::source::{SourceFile, Span};

/// Validates the temporary backend subset and then renders a complete C
/// translation unit in memory. Callers may safely defer all filesystem writes
/// until this function succeeds.
pub(crate) fn emit<'source, 'ast>(
    source: &'source SourceFile,
    program: &'ast Program,
    analysis: &Analysis<'source, 'ast>,
    main: &FunctionSignature<'ast>,
) -> Result<String, Diagnostic> {
    TemporaryEmitter {
        source,
        program,
        analysis,
        main,
    }
    .emit()
}

struct TemporaryEmitter<'analysis, 'source, 'ast> {
    source: &'source SourceFile,
    program: &'ast Program,
    analysis: &'analysis Analysis<'source, 'ast>,
    main: &'analysis FunctionSignature<'ast>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CPrimitive {
    Int,
    Bool,
}

impl CPrimitive {
    const fn c_type(self) -> &'static str {
        match self {
            Self::Int => "int64_t",
            Self::Bool => "bool",
        }
    }

    const fn required_header(self) -> &'static str {
        match self {
            Self::Int => "<stdint.h>",
            Self::Bool => "<stdbool.h>",
        }
    }
}

#[derive(Default)]
struct PrimitiveUsage {
    int: bool,
    boolean: bool,
    output: bool,
    integer_output: bool,
    next_text: usize,
}

impl PrimitiveUsage {
    fn record(&mut self, primitive: CPrimitive) {
        match primitive {
            CPrimitive::Int => self.int = true,
            CPrimitive::Bool => self.boolean = true,
        }
    }

    fn next_text_id(&mut self) -> usize {
        let id = self.next_text;
        self.next_text += 1;
        id
    }
}

impl<'analysis, 'source, 'ast> TemporaryEmitter<'analysis, 'source, 'ast> {
    fn emit(&self) -> Result<String, Diagnostic> {
        self.validate_declarations()?;
        self.validate_main_signature()?;
        self.emit_program()
    }

    fn emit_program(&self) -> Result<String, Diagnostic> {
        let body = &self.main.node.body;
        let mut statements = String::new();
        let mut usage = PrimitiveUsage::default();
        let returns = self.render_block_contents(body, 1, &mut statements, &mut usage)?;
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let unit = self.analysis.types.unit();
        match (self.main.result, &body.value) {
            (TypeState::Resolved(result), None) if result == unit => statements.push_str("    return 0;\n"),
            (TypeState::Resolved(result), Some(value)) if result == unit => {
                return Err(self.unsupported(
                    value.span,
                    "temporary backend does not support a final value in unit-returning main",
                ));
            }
            (TypeState::Resolved(result), Some(value)) if result == int => {
                if self.primitive_expression_type(value)? != CPrimitive::Int {
                    return Err(self.inconsistent_expression_type(value));
                }
                usage.record(CPrimitive::Int);
                writeln!(statements, "    return {};", self.render_expression(value)?)
                    .expect("writing to a String cannot fail");
            }
            (TypeState::Resolved(result), None) if result == int && returns => {}
            (TypeState::Resolved(result), None) if result == int => {
                return Err(self.unsupported(
                    body.span,
                    "temporary backend requires integer main to have a final value or unconditional return",
                ));
            }
            _ => {
                return Err(Diagnostic::compiler(
                    "temporary backend received an inconsistent analyzed main result",
                ));
            }
        }

        let mut output = String::new();
        if usage.boolean {
            writeln!(output, "#include {}", CPrimitive::Bool.required_header())
                .expect("writing to a String cannot fail");
        }
        if usage.int {
            writeln!(output, "#include {}", CPrimitive::Int.required_header())
                .expect("writing to a String cannot fail");
        }
        if usage.integer_output {
            output.push_str("#include <inttypes.h>\n");
        }
        if usage.output {
            output.push_str("#include <stdio.h>\n");
        }
        if usage.boolean || usage.int || usage.output {
            output.push('\n');
        }
        if usage.output {
            output.push_str(concat!(
                "#ifdef _WIN32\n",
                "#include <fcntl.h>\n",
                "#include <io.h>\n",
                "#endif\n\n",
            ));
        }
        output.push_str("int main(void) {\n");
        if usage.output {
            output.push_str(concat!(
                "#ifdef _WIN32\n",
                "    if (_setmode(_fileno(stdout), _O_BINARY) == -1) return 1;\n",
                "#endif\n",
            ));
        }
        output.push_str(&statements);
        output.push_str("}\n");
        Ok(output)
    }

    fn validate_declarations(&self) -> Result<(), Diagnostic> {
        for declaration in &self.program.declarations {
            let is_main = matches!(
                declaration,
                Declaration::Function(function) if std::ptr::eq(function, self.main.node)
            );
            if !is_main {
                let kind = match declaration {
                    Declaration::Type(_) => "type declaration",
                    Declaration::Function(_) => "function declaration other than 'main'",
                };
                return Err(self.unsupported(
                    declaration.span(),
                    format!("temporary backend does not support {kind}"),
                ));
            }
        }
        Ok(())
    }

    fn validate_main_signature(&self) -> Result<(), Diagnostic> {
        if let Some(parameter) = self.main.node.parameters.first() {
            return Err(self.unsupported_signature(
                parameter.span,
                "temporary backend does not support parameters on 'main'",
            ));
        }
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let unit = self.analysis.types.unit();
        match (&self.main.node.return_type, self.main.result) {
            (None, TypeState::Resolved(result)) if result == unit => {}
            (Some(return_type), TypeState::Resolved(result)) if result == unit && matches!(&return_type.kind, crate::ast::TypeKind::Unit) => {}
            (Some(_), TypeState::Resolved(result)) if result == int => {}
            (Some(return_type), TypeState::Resolved(_)) => {
                return Err(self.unsupported_type(return_type));
            }
            _ => {
                return Err(Diagnostic::compiler(
                    "temporary backend received an inconsistent analyzed main signature",
                ));
            }
        }
        Ok(())
    }

    fn render_block_contents(
        &self,
        block: &'ast Block,
        indentation: usize,
        output: &mut String,
        usage: &mut PrimitiveUsage,
    ) -> Result<bool, Diagnostic> {
        let mut returns = false;
        for statement in &block.statements {
            returns |= self.render_statement(statement, indentation, output, usage)?;
        }
        Ok(returns)
    }

    fn render_statement(
        &self,
        statement: &'ast Statement,
        indentation: usize,
        output: &mut String,
        usage: &mut PrimitiveUsage,
    ) -> Result<bool, Diagnostic> {
        match &statement.kind {
            StatementKind::Local {
                mutable,
                name,
                initializer,
            } => {
                let Some(binding) = self.analysis.local_binding(statement) else {
                    return Err(Diagnostic::compiler(
                        "temporary backend received a local without a binding identity",
                    ));
                };
                let record = self.analysis.binding(binding);
                if !matches!(
                    record.node,
                    BindingNode::Local(node) if std::ptr::eq(node, statement)
                ) || record.mutable != *mutable
                {
                    return Err(Diagnostic::compiler(
                        "temporary backend received an inconsistent local binding record",
                    ));
                }
                let binding_type = self.primitive_state(
                    self.analysis.binding_type(binding),
                    name.span,
                    "local binding",
                )?;
                let initializer_type = self.primitive_expression_type(initializer)?;
                if binding_type != initializer_type {
                    return Err(self.inconsistent_expression_type(initializer));
                }
                usage.record(binding_type);

                Self::write_indentation(output, indentation);
                if !mutable {
                    output.push_str("const ");
                }
                writeln!(
                    output,
                    "{} sao2_binding_{} = {};",
                    binding_type.c_type(),
                    binding.index(),
                    self.render_expression(initializer)?
                )
                .expect("writing to a String cannot fail");
                Ok(false)
            }
            StatementKind::Assignment {
                target,
                operator,
                value,
                ..
            } => {
                self.render_assignment(target, *operator, value, indentation, output, usage)?;
                Ok(false)
            }
            StatementKind::Block(block) => {
                if let Some(value) = &block.value {
                    return Err(self.unsupported_expression(value));
                }
                Self::write_indentation(output, indentation);
                output.push_str("{\n");
                let returns =
                    self.render_block_contents(block, indentation + 1, output, usage)?;
                Self::write_indentation(output, indentation);
                output.push_str("}\n");
                Ok(returns)
            }
            StatementKind::Expression(expression) => {
                self.render_output(expression, indentation, output, usage)?;
                Ok(false)
            }
            StatementKind::Return(value) => {
                let TypeState::Resolved(result) = self.main.result else {
                    return Err(Diagnostic::compiler(
                        "temporary backend received an unresolved main return type",
                    ));
                };
                let int = self.analysis.types.primitive(PrimitiveType::Int);
                let unit = self.analysis.types.unit();
                if result == unit {
                    return Err(self.unsupported_statement(
                        statement.span,
                        "temporary backend supports return only from integer main",
                    ));
                }
                if result != int {
                    return Err(Diagnostic::compiler(
                        "temporary backend received an inconsistent main return type",
                    ));
                }
                let Some(value) = value else {
                    return Err(self.unsupported_statement(
                        statement.span,
                        "temporary backend requires integer return to have a value",
                    ));
                };
                if self.primitive_expression_type(value)? != CPrimitive::Int {
                    return Err(self.inconsistent_expression_type(value));
                }
                usage.record(CPrimitive::Int);
                Self::write_indentation(output, indentation);
                writeln!(output, "return {};", self.render_expression(value)?)
                    .expect("writing to a String cannot fail");
                Ok(true)
            }
            _ => Err(self.unsupported_statement(
                statement.span,
                "temporary backend does not support this statement",
            )),
        }
    }

    fn render_output(
        &self,
        expression: &'ast Expression,
        indentation: usize,
        output: &mut String,
        usage: &mut PrimitiveUsage,
    ) -> Result<(), Diagnostic> {
        let ExpressionKind::Call { arguments, .. } = &expression.kind else {
            return Err(self.unsupported_expression(expression));
        };
        let Some(resolution) = self.analysis.call_resolution(expression) else {
            return Err(Diagnostic::compiler(
                "temporary backend received an output call without call resolution",
            ));
        };
        let intrinsic = match resolution.target {
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)) => IntrinsicId::Print,
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Println)) => {
                IntrinsicId::Println
            }
            _ => return Err(self.unsupported_call(expression)),
        };
        if self
            .analysis
            .expression_annotation(expression)
            .map(|annotation| annotation.state)
            != Some(TypeState::Resolved(self.analysis.types.unit()))
        {
            return Err(Diagnostic::compiler(
                "temporary backend received an unresolved output call",
            ));
        }

        let argument = match (intrinsic, arguments.as_slice()) {
            (IntrinsicId::Print, [argument]) | (IntrinsicId::Println, [argument]) => {
                let ArgumentKind::Positional(argument) = &argument.kind else {
                    return Err(self.unsupported(
                        argument.span,
                        "temporary backend does not support named output arguments",
                    ));
                };
                Some(argument)
            }
            (IntrinsicId::Println, []) => None,
            _ => return Err(self.unsupported_call(expression)),
        };

        usage.output = true;
        let newline = intrinsic == IntrinsicId::Println;
        if let Some(argument) = argument {
            self.render_output_value(argument, newline, indentation, output, usage)?;
        } else if newline {
            Self::write_indentation(output, indentation);
            output.push_str("if (fputc('\\n', stdout) == EOF) return 1;\n");
        }
        Ok(())
    }

    fn render_output_value(
        &self,
        argument: &'ast Expression,
        newline: bool,
        indentation: usize,
        output: &mut String,
        usage: &mut PrimitiveUsage,
    ) -> Result<(), Diagnostic> {
        let Some(annotation) = self.analysis.expression_annotation(argument) else {
            return Err(Diagnostic::compiler(
                "temporary backend received an output argument without a type annotation",
            ));
        };
        let str_type = self.analysis.types.primitive(PrimitiveType::Str);
        if annotation.state == TypeState::Resolved(str_type) {
            if !matches!(&argument.kind, ExpressionKind::String(_)) {
                return Err(self.unsupported(
                    argument.span,
                    "temporary backend supports string output only for direct literals",
                ));
            }
            let Some(LiteralValue::String(bytes)) = self.analysis.literal(argument) else {
                return Err(Diagnostic::compiler(
                    "temporary backend received a string output without a decoded literal",
                ));
            };
            let id = usage.next_text_id();
            Self::write_indentation(output, indentation);
            write!(output, "static const unsigned char sao2_text_{id}[] = {{")
                .expect("writing to a String cannot fail");
            if bytes.is_empty() {
                if newline {
                    output.push_str("10");
                } else {
                    output.push('0');
                }
            } else {
                for (index, byte) in bytes.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    write!(output, "{byte}").expect("writing to a String cannot fail");
                }
                if newline {
                    output.push_str(", 10");
                }
            }
            output.push_str("};\n");
            let output_length = bytes.len() + if newline { 1 } else { 0 };
            Self::write_indentation(output, indentation);
            writeln!(
                output,
                "if (fwrite(sao2_text_{id}, 1, {}, stdout) != {}) return 1;",
                output_length,
                output_length
            )
            .expect("writing to a String cannot fail");
            return Ok(());
        }

        let primitive = self.primitive_expression_type(argument)?;
        usage.record(primitive);
        Self::write_indentation(output, indentation);
        match primitive {
            CPrimitive::Int => {
                usage.integer_output = true;
                if newline {
                    writeln!(
                        output,
                        "if (printf(\"%\" PRId64 \"\\n\", {}) < 0) return 1;",
                        self.render_expression(argument)?
                    )
                    .expect("writing to a String cannot fail");
                } else {
                    writeln!(
                        output,
                        "if (printf(\"%\" PRId64, {}) < 0) return 1;",
                        self.render_expression(argument)?
                    )
                    .expect("writing to a String cannot fail");
                }
            }
            CPrimitive::Bool => {
                let suffix = if newline { "\\n" } else { "" };
                writeln!(
                    output,
                    "if (fputs(({}) ? \"true{suffix}\" : \"false{suffix}\", stdout) == EOF) return 1;",
                    self.render_expression(argument)?
                )
                .expect("writing to a String cannot fail");
            }
        }
        Ok(())
    }

    fn render_assignment(
        &self,
        target: &'ast AssignmentTarget,
        operator: AssignmentOperator,
        value: &'ast Expression,
        indentation: usize,
        output: &mut String,
        usage: &mut PrimitiveUsage,
    ) -> Result<(), Diagnostic> {
        if let Some(suffix) = target.suffixes.first() {
            return Err(self.unsupported(
                suffix.span,
                "temporary backend supports only direct local assignment",
            ));
        }
        let Some(name_use) = self.analysis.name_use(&target.root) else {
            return Err(Diagnostic::compiler(
                "temporary backend received an assignment without name resolution",
            ));
        };
        let NameResolution::Binding(binding) = name_use.resolution else {
            return Err(Diagnostic::compiler(
                "temporary backend received a non-binding assignment target",
            ));
        };
        let record = self.analysis.binding(binding);
        if !matches!(record.node, BindingNode::Local(_)) {
            return Err(self.unsupported(
                target.root.span,
                "temporary backend supports assignment only to local bindings",
            ));
        }
        if !record.mutable {
            return Err(self.unsupported(
                target.root.span,
                "temporary backend supports assignment only to mutable local bindings",
            ));
        }

        let Some(annotation) = self.analysis.assignment_target(target) else {
            return Err(Diagnostic::compiler(
                "temporary backend received an assignment target without a type annotation",
            ));
        };
        let target_type = self.primitive_state(annotation.state, target.span, "assignment target")?;
        let binding_type = self.primitive_state(
            self.analysis.binding_type(binding),
            target.root.span,
            "local binding",
        )?;
        let value_type = self.primitive_expression_type(value)?;
        let valid = match operator {
            AssignmentOperator::Assign => {
                target_type == binding_type && value_type == target_type
            }
            AssignmentOperator::Add
            | AssignmentOperator::Subtract
            | AssignmentOperator::Multiply
            | AssignmentOperator::Divide
            | AssignmentOperator::Remainder
            | AssignmentOperator::BitwiseAnd
            | AssignmentOperator::BitwiseOr
            | AssignmentOperator::BitwiseXor
            | AssignmentOperator::ShiftLeft
            | AssignmentOperator::ShiftRight => {
                target_type == CPrimitive::Int
                    && binding_type == CPrimitive::Int
                    && value_type == CPrimitive::Int
            }
        };
        if !valid {
            return Err(Diagnostic::compiler(
                "temporary backend received inconsistent analyzed assignment types",
            ));
        }
        usage.record(target_type);

        let symbol = match operator {
            AssignmentOperator::Assign => "=",
            AssignmentOperator::Add => "+=",
            AssignmentOperator::Subtract => "-=",
            AssignmentOperator::Multiply => "*=",
            AssignmentOperator::Divide => "/=",
            AssignmentOperator::Remainder => "%=",
            AssignmentOperator::BitwiseAnd => "&=",
            AssignmentOperator::BitwiseOr => "|=",
            AssignmentOperator::BitwiseXor => "^=",
            AssignmentOperator::ShiftLeft => "<<=",
            AssignmentOperator::ShiftRight => ">>=",
        };
        Self::write_indentation(output, indentation);
        writeln!(
            output,
            "sao2_binding_{} {symbol} {};",
            binding.index(),
            self.render_expression(value)?
        )
        .expect("writing to a String cannot fail");
        Ok(())
    }

    fn write_indentation(output: &mut String, indentation: usize) {
        for _ in 0..indentation {
            output.push_str("    ");
        }
    }

    fn render_expression(&self, expression: &'ast Expression) -> Result<String, Diagnostic> {
        match &expression.kind {
            ExpressionKind::Identifier(identifier) => {
                let expression_type = self.primitive_expression_type(expression)?;
                let Some(name_use) = self.analysis.name_use(identifier) else {
                    return Err(Diagnostic::compiler(
                        "temporary backend received a binding read without name resolution",
                    ));
                };
                let NameResolution::Binding(binding) = name_use.resolution else {
                    return Err(Diagnostic::compiler(
                        "temporary backend received a non-binding identifier expression",
                    ));
                };
                let binding_type = self.primitive_state(
                    self.analysis.binding_type(binding),
                    identifier.span,
                    "local binding",
                )?;
                if expression_type != binding_type {
                    return Err(self.inconsistent_expression_type(expression));
                }
                Ok(format!("sao2_binding_{}", binding.index()))
            }
            ExpressionKind::Integer => {
                if self.primitive_expression_type(expression)? != CPrimitive::Int {
                    return Err(self.inconsistent_expression_type(expression));
                }
                self.render_integer_literal(expression)
            }
            ExpressionKind::Boolean(_) => {
                if self.primitive_expression_type(expression)? != CPrimitive::Bool {
                    return Err(self.inconsistent_expression_type(expression));
                }
                self.render_boolean_literal(expression)
            }
            ExpressionKind::Parenthesized(inner) => {
                self.primitive_expression_type(expression)?;
                Ok(format!("({})", self.render_expression(inner)?))
            }
            ExpressionKind::Unary {
                operator, operand, ..
            } => self.render_unary(expression, *operator, operand),
            ExpressionKind::Binary {
                left,
                operator,
                operator_span,
                right,
            } => self.render_binary(expression, left, *operator, *operator_span, right),
            _ => Err(self.unsupported_expression(expression)),
        }
    }

    fn render_integer_literal(&self, expression: &'ast Expression) -> Result<String, Diagnostic> {
        match self.analysis.literal(expression) {
            Some(LiteralValue::Integer(value)) if *value <= i64::MAX as u64 => {
                Ok(format!("INT64_C({value})"))
            }
            Some(LiteralValue::Integer(_)) => Err(Diagnostic::compiler(
                "temporary backend received an out-of-range positive integer literal",
            )),
            _ => Err(Diagnostic::compiler(
                "temporary backend received an integer expression without a converted literal",
            )),
        }
    }

    fn render_boolean_literal(&self, expression: &'ast Expression) -> Result<String, Diagnostic> {
        match self.analysis.literal(expression) {
            Some(LiteralValue::Boolean(true)) => Ok("true".to_owned()),
            Some(LiteralValue::Boolean(false)) => Ok("false".to_owned()),
            _ => Err(Diagnostic::compiler(
                "temporary backend received a boolean expression without a converted literal",
            )),
        }
    }

    fn render_unary(
        &self,
        expression: &'ast Expression,
        operator: UnaryOperator,
        operand: &'ast Expression,
    ) -> Result<String, Diagnostic> {
        let result_type = self.primitive_expression_type(expression)?;
        let operand_type = self.primitive_expression_type(operand)?;
        let valid = match operator {
            UnaryOperator::LogicalNot => {
                result_type == CPrimitive::Bool && operand_type == CPrimitive::Bool
            }
            UnaryOperator::BitwiseNot | UnaryOperator::Plus | UnaryOperator::Minus => {
                result_type == CPrimitive::Int && operand_type == CPrimitive::Int
            }
        };
        if !valid {
            return Err(self.inconsistent_expression_type(expression));
        }

        if operator == UnaryOperator::Minus
            && matches!(&operand.kind, ExpressionKind::Integer)
            && matches!(
                self.analysis.literal(operand),
                Some(LiteralValue::Integer(value)) if *value == (i64::MAX as u64) + 1
            )
        {
            return Ok("INT64_MIN".to_owned());
        }

        let symbol = match operator {
            UnaryOperator::LogicalNot => "!",
            UnaryOperator::BitwiseNot => "~",
            UnaryOperator::Plus => "+",
            UnaryOperator::Minus => "-",
        };
        Ok(format!("({symbol}{})", self.render_expression(operand)?))
    }

    fn render_binary(
        &self,
        expression: &'ast Expression,
        left: &'ast Expression,
        operator: BinaryOperator,
        operator_span: Span,
        right: &'ast Expression,
    ) -> Result<String, Diagnostic> {
        let result_type = self.primitive_expression_type(expression)?;
        let left_type = self.primitive_expression_type(left)?;
        let right_type = self.primitive_expression_type(right)?;
        let (symbol, valid) = match operator {
            BinaryOperator::LogicalOr => (
                "||",
                left_type == CPrimitive::Bool
                    && right_type == CPrimitive::Bool
                    && result_type == CPrimitive::Bool,
            ),
            BinaryOperator::LogicalAnd => (
                "&&",
                left_type == CPrimitive::Bool
                    && right_type == CPrimitive::Bool
                    && result_type == CPrimitive::Bool,
            ),
            BinaryOperator::BitwiseOr => ("|", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::BitwiseXor => ("^", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::BitwiseAnd => ("&", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::ShiftLeft => ("<<", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::ShiftRight => (">>", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Add => ("+", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Subtract => ("-", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Multiply => ("*", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Divide => ("/", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Remainder => ("%", self.all_int(left_type, right_type, result_type)),
            BinaryOperator::Equal => (
                "==",
                left_type == right_type && result_type == CPrimitive::Bool,
            ),
            BinaryOperator::NotEqual => (
                "!=",
                left_type == right_type && result_type == CPrimitive::Bool,
            ),
            BinaryOperator::Less => ("<", self.int_comparison(left_type, right_type, result_type)),
            BinaryOperator::LessEqual => {
                ("<=", self.int_comparison(left_type, right_type, result_type))
            }
            BinaryOperator::Greater => {
                (">", self.int_comparison(left_type, right_type, result_type))
            }
            BinaryOperator::GreaterEqual => {
                (">=", self.int_comparison(left_type, right_type, result_type))
            }
            BinaryOperator::In => {
                return Err(self.unsupported(
                    operator_span,
                    "temporary backend does not support this binary operator",
                ));
            }
        };
        if !valid {
            return Err(self.inconsistent_expression_type(expression));
        }

        Ok(format!(
            "({} {symbol} {})",
            self.render_expression(left)?,
            self.render_expression(right)?
        ))
    }

    fn all_int(&self, left: CPrimitive, right: CPrimitive, result: CPrimitive) -> bool {
        left == CPrimitive::Int && right == CPrimitive::Int && result == CPrimitive::Int
    }

    fn int_comparison(&self, left: CPrimitive, right: CPrimitive, result: CPrimitive) -> bool {
        left == CPrimitive::Int && right == CPrimitive::Int && result == CPrimitive::Bool
    }

    fn primitive_expression_type(
        &self,
        expression: &'ast Expression,
    ) -> Result<CPrimitive, Diagnostic> {
        let Some(annotation) = self.analysis.expression_annotation(expression) else {
            return Err(Diagnostic::compiler(
                "temporary backend received an expression without an analysis annotation",
            ));
        };
        match annotation.state {
            TypeState::Resolved(ty)
                if ty == self.analysis.types.primitive(PrimitiveType::Int) =>
            {
                Ok(CPrimitive::Int)
            }
            TypeState::Resolved(ty)
                if ty == self.analysis.types.primitive(PrimitiveType::Bool) =>
            {
                Ok(CPrimitive::Bool)
            }
            TypeState::Resolved(_) => Err(self.unsupported(
                expression.span,
                "temporary backend does not support this expression type",
            )),
            TypeState::Deferred(_) => Err(self.unsupported(
                expression.span,
                "temporary backend does not support flow-dependent expressions",
            )),
            TypeState::Never => Err(self.unsupported_expression(expression)),
            TypeState::Error => Err(Diagnostic::compiler(
                "temporary backend received an expression with an analysis error",
            )),
        }
    }

    fn primitive_state(
        &self,
        state: TypeState,
        span: Span,
        subject: &str,
    ) -> Result<CPrimitive, Diagnostic> {
        match state {
            TypeState::Resolved(ty)
                if ty == self.analysis.types.primitive(PrimitiveType::Int) =>
            {
                Ok(CPrimitive::Int)
            }
            TypeState::Resolved(ty)
                if ty == self.analysis.types.primitive(PrimitiveType::Bool) =>
            {
                Ok(CPrimitive::Bool)
            }
            TypeState::Resolved(_) => Err(self.unsupported(
                span,
                format!("temporary backend does not support this {subject} type"),
            )),
            TypeState::Deferred(_) => Err(self.unsupported(
                span,
                format!("temporary backend does not support flow-dependent {subject} types"),
            )),
            TypeState::Never => Err(self.unsupported(
                span,
                format!("temporary backend requires {subject} to produce a value"),
            )),
            TypeState::Error => Err(Diagnostic::compiler(format!(
                "temporary backend received {subject} with an analysis error"
            ))),
        }
    }

    fn inconsistent_expression_type(&self, expression: &Expression) -> Diagnostic {
        Diagnostic::compiler(format!(
            "temporary backend received inconsistent analyzed types for expression at byte {}",
            expression.span.start
        ))
    }

    fn unsupported_statement(&self, span: Span, message: &str) -> Diagnostic {
        self.unsupported(span, message)
    }

    fn unsupported_signature(&self, span: Span, message: &str) -> Diagnostic {
        self.unsupported(span, message)
    }

    fn unsupported_expression(&self, expression: &Expression) -> Diagnostic {
        self.unsupported(
            expression.span,
            "temporary backend does not support this expression",
        )
    }

    fn unsupported_call(&self, expression: &Expression) -> Diagnostic {
        let span = match &expression.kind {
            ExpressionKind::Call { callee, .. } => callee.span,
            _ => expression.span,
        };
        self.unsupported(
            span,
            "temporary backend supports only direct calls to 'print' and 'println'",
        )
    }

    fn unsupported_type(&self, ty: &Type) -> Diagnostic {
        self.unsupported(ty.span, "temporary backend does not support this type")
    }

    fn unsupported(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::source(self.source, span, message)
    }
}

#[cfg(test)]
pub(crate) fn render_print_program(bytes: &[u8]) -> String {
    let mut output = String::from(concat!(
        "#include <stdio.h>\n\n",
        "#ifdef _WIN32\n",
        "#include <fcntl.h>\n",
        "#include <io.h>\n",
        "#endif\n\n",
        "int main(void) {\n",
        "#ifdef _WIN32\n",
        "    if (_setmode(_fileno(stdout), _O_BINARY) == -1) return 1;\n",
        "#endif\n",
        "    static const unsigned char sao2_text_0[] = {",
    ));

    if bytes.is_empty() {
        output.push('0');
    } else {
        for (index, byte) in bytes.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            write!(output, "{byte}").expect("writing to a String cannot fail");
        }
    }

    write!(
        output,
        "}};\n    if (fwrite(sao2_text_0, 1, {}, stdout) != {}) return 1;\n    return 0;\n}}\n",
        bytes.len(),
        bytes.len()
    )
    .expect("writing to a String cannot fail");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analysis, parser};
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    fn render_program(text: &str) -> Result<String, Diagnostic> {
        let source = source(text);
        let program = parser::parse(&source).unwrap();
        let analysis = analysis::analyze(&source, &program);
        assert!(
            analysis.diagnostics.is_empty(),
            "{}",
            analysis.diagnostics
        );
        let main = analysis.function_signature(analysis.function_by_name("main").unwrap());
        emit(&source, &program, &analysis, main)
    }

    fn render_statement_at(text: &str, statement_index: usize) -> Result<String, Diagnostic> {
        let source = source(text);
        let program = parser::parse(&source).unwrap();
        let analysis = analysis::analyze(&source, &program);
        assert!(
            analysis.diagnostics.is_empty(),
            "{}",
            analysis.diagnostics
        );
        let main = analysis.function_signature(analysis.function_by_name("main").unwrap());
        let mut output = String::new();
        let mut usage = PrimitiveUsage::default();
        TemporaryEmitter {
            source: &source,
            program: &program,
            analysis: &analysis,
            main,
        }
        .render_statement(
            &main.node.body.statements[statement_index],
            1,
            &mut output,
            &mut usage,
        )?;
        Ok(output)
    }

    fn render_initializer(text: &str, statement_index: usize) -> Result<String, Diagnostic> {
        let source = source(text);
        let program = parser::parse(&source).unwrap();
        let analysis = analysis::analyze(&source, &program);
        assert!(
            analysis.diagnostics.is_empty(),
            "{}",
            analysis.diagnostics
        );
        let main = analysis.function_signature(analysis.function_by_name("main").unwrap());
        let StatementKind::Local { initializer, .. } =
            &main.node.body.statements[statement_index].kind
        else {
            panic!("expected local statement");
        };
        TemporaryEmitter {
            source: &source,
            program: &program,
            analysis: &analysis,
            main,
        }
        .render_expression(initializer)
    }

    #[test]
    fn emits_hello_snapshot() {
        assert_eq!(
            render_print_program(b"hello"),
            concat!(
                "#include <stdio.h>\n\n",
                "#ifdef _WIN32\n",
                "#include <fcntl.h>\n",
                "#include <io.h>\n",
                "#endif\n\n",
                "int main(void) {\n",
                "#ifdef _WIN32\n",
                "    if (_setmode(_fileno(stdout), _O_BINARY) == -1) return 1;\n",
                "#endif\n",
                "    static const unsigned char sao2_text_0[] = ",
                "{104, 101, 108, 108, 111};\n",
                "    if (fwrite(sao2_text_0, 1, 5, stdout) != 5) return 1;\n",
                "    return 0;\n",
                "}\n",
            )
        );
    }

    #[test]
    fn emits_standard_c_for_empty_string() {
        let output = render_print_program(b"");
        assert!(output.contains("sao2_text_0[] = {0}"));
        assert!(output.contains("fwrite(sao2_text_0, 1, 0, stdout) != 0"));
    }

    #[test]
    fn emits_every_ascii_byte_numerically() {
        let bytes: Vec<u8> = (0..=127).collect();
        let output = render_print_program(&bytes);
        assert!(output.contains("{0, 1, 2, 3, 4, 5"));
        assert!(output.contains("122, 123, 124, 125, 126, 127}"));
        assert!(output.contains("fwrite(sao2_text_0, 1, 128, stdout) != 128"));
        assert!(!output.contains('"'));
    }

    #[test]
    fn output_is_deterministic() {
        let bytes = b"quotes: \"; slash: \\; newline: \n; nul: \0";
        assert_eq!(render_print_program(bytes), render_print_program(bytes));
    }

    #[test]
    fn retains_the_exact_legacy_print_translation() {
        assert_eq!(
            render_program("fn main() { print(\"hello\"); }").unwrap(),
            render_print_program(b"hello")
        );
    }

    #[test]
    fn emits_primitive_locals_scopes_shadowing_and_assignment() {
        let output = render_program(concat!(
            "fn main() { ",
            "base := 1; var total := base + 2; ",
            "{ flag := true; total += 3; var flag := false; flag = true; } ",
            "total <<= 1; ",
            "}"
        ))
        .unwrap();
        assert_eq!(
            output,
            concat!(
                "#include <stdbool.h>\n",
                "#include <stdint.h>\n\n",
                "int main(void) {\n",
                "    const int64_t sao2_binding_0 = INT64_C(1);\n",
                "    int64_t sao2_binding_1 = (sao2_binding_0 + INT64_C(2));\n",
                "    {\n",
                "        const bool sao2_binding_2 = true;\n",
                "        sao2_binding_1 += INT64_C(3);\n",
                "        bool sao2_binding_3 = false;\n",
                "        sao2_binding_3 = true;\n",
                "    }\n",
                "    sao2_binding_1 <<= INT64_C(1);\n",
                "    return 0;\n",
                "}\n",
            )
        );
    }

    #[test]
    fn emits_every_supported_compound_assignment() {
        for operator in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<=", ">>="] {
            let text = format!("fn main() {{ var value := 8; value {operator} 1; }}");
            let output = render_program(&text).unwrap();
            assert!(
                output.contains(&format!("sao2_binding_0 {operator} INT64_C(1);")),
                "{operator}: {output}"
            );
        }
    }

    #[test]
    fn emits_only_headers_required_by_primitive_locals() {
        let integers = render_program("fn main() { value := 1; }").unwrap();
        assert!(integers.contains("#include <stdint.h>"));
        assert!(!integers.contains("#include <stdbool.h>"));

        let booleans = render_program("fn main() { value := true; }").unwrap();
        assert!(booleans.contains("#include <stdbool.h>"));
        assert!(!booleans.contains("#include <stdint.h>"));
    }

    #[test]
    fn rejects_immutable_and_indirect_assignment() {
        let immutable = render_program("fn main() { value := 1; value = 2; }")
            .unwrap_err()
            .to_string();
        assert!(immutable.contains("only to mutable local bindings"), "{immutable}");

        let indirect = render_statement_at(
            "fn main() { var values := [1]; values[0] = 2; }",
            1,
        )
        .unwrap_err()
        .to_string();
        assert!(indirect.contains("only direct local assignment"), "{indirect}");
    }

    #[test]
    fn emits_multiple_output_calls_in_source_order() {
        let output = render_program(concat!(
            "fn main() { value := 7; flag := true; ",
            "print(\"value=\"); println(value); print(flag); println(\"done\"); println(); }"
        ))
        .unwrap();
        let text = output.find("sao2_text_0[] = {118, 97, 108, 117, 101, 61}").unwrap();
        let integer = output
            .find("printf(\"%\" PRId64 \"\\n\", sao2_binding_0)")
            .unwrap();
        let boolean = output.find("fputs((sao2_binding_1) ? \"true\" : \"false\"").unwrap();
        let string_newline = output
            .find("sao2_text_1[] = {100, 111, 110, 101, 10}")
            .unwrap();
        let final_newline = output.rfind("fputc('\\n', stdout)").unwrap();
        assert!(
            text < integer
                && integer < boolean
                && boolean < string_newline
                && string_newline < final_newline
        );
        assert_eq!(output.matches("fputc('\\n', stdout)").count(), 1);
    }

    #[test]
    fn emits_no_value_and_integer_main_results() {
        assert_eq!(
            render_program("fn main() {}").unwrap(),
            "int main(void) {\n    return 0;\n}\n"
        );
        let final_value = render_program("fn main() int { 7 }").unwrap();
        assert!(final_value.contains("return INT64_C(7);"));
        let explicit = render_program("fn main() int { return 9; }").unwrap();
        assert!(explicit.contains("return INT64_C(9);"));
    }

    #[test]
    fn records_primitive_c_representations() {
        assert_eq!(CPrimitive::Int.c_type(), "int64_t");
        assert_eq!(CPrimitive::Int.required_header(), "<stdint.h>");
        assert_eq!(CPrimitive::Bool.c_type(), "bool");
        assert_eq!(CPrimitive::Bool.required_header(), "<stdbool.h>");
    }

    #[test]
    fn renders_converted_integer_and_boolean_literals() {
        for (expression, expected) in [
            ("1_000", "INT64_C(1000)"),
            ("0xff", "INT64_C(255)"),
            ("0b1010", "INT64_C(10)"),
            ("true", "true"),
            ("false", "false"),
            ("-9_223_372_036_854_775_808", "INT64_MIN"),
        ] {
            let text = format!("fn main() {{ value := {expression}; }}");
            assert_eq!(render_initializer(&text, 0).unwrap(), expected, "{expression}");
        }
    }

    #[test]
    fn renders_every_supported_unary_and_binary_operator() {
        for (expression, expected) in [
            ("+1", "(+INT64_C(1))"),
            ("-1", "(-INT64_C(1))"),
            ("~1", "(~INT64_C(1))"),
            ("!false", "(!false)"),
            ("1 + 2", "(INT64_C(1) + INT64_C(2))"),
            ("3 - 2", "(INT64_C(3) - INT64_C(2))"),
            ("2 * 3", "(INT64_C(2) * INT64_C(3))"),
            ("6 / 2", "(INT64_C(6) / INT64_C(2))"),
            ("7 % 3", "(INT64_C(7) % INT64_C(3))"),
            ("1 | 2", "(INT64_C(1) | INT64_C(2))"),
            ("1 ^ 2", "(INT64_C(1) ^ INT64_C(2))"),
            ("1 & 2", "(INT64_C(1) & INT64_C(2))"),
            ("1 << 2", "(INT64_C(1) << INT64_C(2))"),
            ("4 >> 1", "(INT64_C(4) >> INT64_C(1))"),
            ("1 == 2", "(INT64_C(1) == INT64_C(2))"),
            ("true != false", "(true != false)"),
            ("1 < 2", "(INT64_C(1) < INT64_C(2))"),
            ("1 <= 2", "(INT64_C(1) <= INT64_C(2))"),
            ("2 > 1", "(INT64_C(2) > INT64_C(1))"),
            ("2 >= 1", "(INT64_C(2) >= INT64_C(1))"),
            ("true && false", "(true && false)"),
            ("true || false", "(true || false)"),
        ] {
            let text = format!("fn main() {{ value := {expression}; }}");
            assert_eq!(render_initializer(&text, 0).unwrap(), expected, "{expression}");
        }
    }

    #[test]
    fn preserves_ast_grouping_with_deliberate_parentheses() {
        assert_eq!(
            render_initializer("fn main() { value := 1 + 2 * 3; }", 0).unwrap(),
            "(INT64_C(1) + (INT64_C(2) * INT64_C(3)))"
        );
        assert_eq!(
            render_initializer("fn main() { value := (1 + 2) * 3; }", 0).unwrap(),
            "(((INT64_C(1) + INT64_C(2))) * INT64_C(3))"
        );
        assert_eq!(
            render_initializer("fn main() { value := true || false && !false; }", 0).unwrap(),
            "(true || (false && (!false)))"
        );
    }

    #[test]
    fn renders_binding_reads_from_stable_identities() {
        let text = concat!(
            "fn main() { ",
            "value := 1; first := value; value := 2; second := value; ",
            "}"
        );
        assert_eq!(render_initializer(text, 1).unwrap(), "sao2_binding_0");
        assert_eq!(render_initializer(text, 3).unwrap(), "sao2_binding_2");
    }

    #[test]
    fn rejects_unsupported_expression_forms_and_types() {
        for (text, expected) in [
            (
                "fn main() { value := 1.0; result := value; }",
                "does not support this expression type",
            ),
            (
                "fn main() { value := [1, 2]; }",
                "does not support this expression",
            ),
            (
                "fn helper() int { 1 } fn main() { value := helper(); }",
                "does not support this expression",
            ),
        ] {
            let statement_index = if text.contains("result") { 1 } else { 0 };
            let diagnostic = render_initializer(text, statement_index)
                .unwrap_err()
                .to_string();
            assert!(diagnostic.contains(expected), "{diagnostic}");
        }
    }

    #[test]
    fn reports_missing_analysis_facts_as_compiler_invariants() {
        let source = source("fn main() { print(\"hello\"); }");
        let program = parser::parse(&source).unwrap();
        let unannotated = Expression {
            kind: ExpressionKind::Integer,
            span: Span::empty(0),
        };
        let analysis = analysis::analyze(&source, &program);
        let main = analysis.function_signature(analysis.function_by_name("main").unwrap());
        let emitter = TemporaryEmitter {
            source: &source,
            program: &program,
            analysis: &analysis,
            main,
        };

        let annotation = emitter
            .render_expression(&unannotated)
            .unwrap_err()
            .to_string();
        assert!(annotation.contains("compiler error"), "{annotation}");
        assert!(annotation.contains("without an analysis annotation"), "{annotation}");

        let literal = emitter
            .render_integer_literal(&unannotated)
            .unwrap_err()
            .to_string();
        assert!(literal.contains("compiler error"), "{literal}");
        assert!(literal.contains("without a converted literal"), "{literal}");
    }
}
