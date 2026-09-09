//! Temporary direct C emitter over the analyzed AST.
//!
//! This deliberately supports only the walking skeleton's string `print` program.
//! Later milestone-4 phases expand the accepted subset; milestone 6 replaces this
//! emitter with typed-IR lowering.

use std::fmt::Write;

use crate::analysis::{
    Analysis, CallableId, CallTarget, FunctionSignature, IntrinsicId, LiteralValue, TypeState,
};
use crate::ast::{
    ArgumentKind, Declaration, Expression, ExpressionKind, Program, Statement, StatementKind, Type,
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

impl<'analysis, 'source, 'ast> TemporaryEmitter<'analysis, 'source, 'ast> {
    fn emit(&self) -> Result<String, Diagnostic> {
        let bytes = self.validate_subset()?;
        Ok(render_print_program(bytes))
    }

    fn validate_subset(&self) -> Result<&'analysis [u8], Diagnostic> {
        self.validate_declarations()?;
        self.validate_main_signature()?;

        let body = &self.main.node.body;
        if let Some(value) = &body.value {
            return Err(self.unsupported_expression(value));
        }
        let [statement] = body.statements.as_slice() else {
            let span = body
                .statements
                .get(1)
                .map_or(body.span, |statement| statement.span);
            return Err(self.unsupported_statement(
                span,
                "temporary backend requires main to contain exactly one print statement",
            ));
        };
        self.validate_print_statement(statement)
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
        if let Some(return_type) = &self.main.node.return_type {
            return Err(self.unsupported_type(return_type));
        }
        if self.main.result != TypeState::NoValue {
            return Err(Diagnostic::compiler(
                "temporary backend received an inconsistent analyzed main signature",
            ));
        }
        Ok(())
    }

    fn validate_print_statement(
        &self,
        statement: &'ast Statement,
    ) -> Result<&'analysis [u8], Diagnostic> {
        let StatementKind::Expression(expression) = &statement.kind else {
            return Err(self.unsupported_statement(
                statement.span,
                "temporary backend does not support this statement",
            ));
        };
        let ExpressionKind::Call { arguments, .. } = &expression.kind else {
            return Err(self.unsupported_expression(expression));
        };

        if !matches!(
            self.analysis.call_resolution(expression).map(|call| call.target),
            Some(CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)))
        ) {
            return Err(self.unsupported_call(expression));
        }
        let [argument] = arguments.as_slice() else {
            return Err(self.unsupported_call(expression));
        };
        let ArgumentKind::Positional(argument) = &argument.kind else {
            return Err(self.unsupported(
                argument.span,
                "temporary backend does not support named call arguments",
            ));
        };
        if !matches!(&argument.kind, ExpressionKind::String(_)) {
            return Err(self.unsupported_expression(argument));
        }

        let str_type = self
            .analysis
            .types
            .primitive(crate::ast::PrimitiveType::Str);
        if self.analysis.expression_annotation(expression).map(|annotation| annotation.state)
            != Some(TypeState::NoValue)
            || self.analysis.expression_annotation(argument).map(|annotation| annotation.state)
                != Some(TypeState::Resolved(str_type))
        {
            return Err(Diagnostic::compiler(
                "temporary backend received an unresolved expression from analysis",
            ));
        }
        match self.analysis.literal(argument) {
            Some(LiteralValue::String(bytes)) => Ok(bytes),
            _ => Err(Diagnostic::compiler(
                "temporary backend received a string expression without a decoded literal",
            )),
        }
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
        self.unsupported(span, "temporary backend supports only a direct call to 'print'")
    }

    fn unsupported_type(&self, ty: &Type) -> Diagnostic {
        self.unsupported(ty.span, "temporary backend does not support this type")
    }

    fn unsupported(&self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::source(self.source, span, message)
    }
}

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
        "    static const unsigned char sao2_text[] = {",
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
        "}};\n    return fwrite(sao2_text, 1, {}, stdout) == {} ? 0 : 1;\n}}\n",
        bytes.len(),
        bytes.len()
    )
    .expect("writing to a String cannot fail");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "    static const unsigned char sao2_text[] = ",
                "{104, 101, 108, 108, 111};\n",
                "    return fwrite(sao2_text, 1, 5, stdout) == 5 ? 0 : 1;\n",
                "}\n",
            )
        );
    }

    #[test]
    fn emits_standard_c_for_empty_string() {
        let output = render_print_program(b"");
        assert!(output.contains("sao2_text[] = {0}"));
        assert!(output.contains("fwrite(sao2_text, 1, 0, stdout) == 0"));
    }

    #[test]
    fn emits_every_ascii_byte_numerically() {
        let bytes: Vec<u8> = (0..=127).collect();
        let output = render_print_program(&bytes);
        assert!(output.contains("{0, 1, 2, 3, 4, 5"));
        assert!(output.contains("122, 123, 124, 125, 126, 127}"));
        assert!(output.contains("fwrite(sao2_text, 1, 128, stdout) == 128"));
        assert!(!output.contains('"'));
    }

    #[test]
    fn output_is_deterministic() {
        let bytes = b"quotes: \"; slash: \\; newline: \n; nul: \0";
        assert_eq!(render_print_program(bytes), render_print_program(bytes));
    }
}
