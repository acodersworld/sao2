//! Minimal C emitter for the milestone-one walking skeleton.

use std::fmt::Write;

pub fn emit_print_program(bytes: &[u8]) -> String {
    let mut output = String::from(
        "#include <stdio.h>\n\nint main(void) {\n    static const unsigned char sao2_text[] = {",
    );

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
            emit_print_program(b"hello"),
            concat!(
                "#include <stdio.h>\n\n",
                "int main(void) {\n",
                "    static const unsigned char sao2_text[] = ",
                "{104, 101, 108, 108, 111};\n",
                "    return fwrite(sao2_text, 1, 5, stdout) == 5 ? 0 : 1;\n",
                "}\n",
            )
        );
    }

    #[test]
    fn emits_standard_c_for_empty_string() {
        let output = emit_print_program(b"");
        assert!(output.contains("sao2_text[] = {0}"));
        assert!(output.contains("fwrite(sao2_text, 1, 0, stdout) == 0"));
    }

    #[test]
    fn emits_every_ascii_byte_numerically() {
        let bytes: Vec<u8> = (0..=127).collect();
        let output = emit_print_program(&bytes);
        assert!(output.contains("{0, 1, 2, 3, 4, 5"));
        assert!(output.contains("122, 123, 124, 125, 126, 127}"));
        assert!(output.contains("fwrite(sao2_text, 1, 128, stdout) == 128"));
        assert!(!output.contains('"'));
    }

    #[test]
    fn output_is_deterministic() {
        let bytes = b"quotes: \"; slash: \\; newline: \n; nul: \0";
        assert_eq!(emit_print_program(bytes), emit_print_program(bytes));
    }
}
