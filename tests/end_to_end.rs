use std::ffi::OsStr;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("sao2 e2e {label} {} {nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write_source(&self, name: &str, source: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, source).unwrap();
        path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn sao2(directory: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .unwrap()
}

fn run_source(directory: &TestDirectory, source: &[u8]) -> Output {
    let source_path = directory.write_source("program with spaces.sao2", source);
    sao2(&directory.0, &[OsStr::new("run"), source_path.as_os_str()])
}

fn run_fixture(directory: &TestDirectory, name: &str, source: &[u8]) -> Output {
    let source_path = directory.write_source(name, source);
    sao2(&directory.0, &[OsStr::new("run"), source_path.as_os_str()])
}

fn compiler_is_missing(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("no supported C compiler found")
}

struct FuzzRng(u64);

impl FuzzRng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next() % limit
    }
}

fn fuzz_seeds() -> Vec<u64> {
    match std::env::var("SAO2_FUZZ_SEED") {
        Ok(seed) => vec![seed
            .parse()
            .expect("SAO2_FUZZ_SEED must be an unsigned decimal integer")],
        Err(_) => vec![
            0x243f_6a88_85a3_08d3,
            0x1319_8a2e_0370_7344,
            0xa409_3822_299f_31d0,
        ],
    }
}

fn fuzzy_primitive_program(seed: u64) -> (String, Vec<u8>) {
    let mut rng = FuzzRng(seed);
    let mut value = (rng.below(1_000) + 1) as i64;
    let mut source = format!("fn main() {{ var value := {value}; println(value); ");
    let mut expected = format!("{value}\n");

    // Operands are nonzero where required, shifts are small, and normalization
    // keeps every operation far inside the signed 64-bit range.
    for _ in 0..64 {
        match rng.below(11) {
            0 => {
                let operand = rng.below(101) as i64;
                writeln!(source, "value += {operand};").unwrap();
                value += operand;
            }
            1 => {
                let operand = rng.below((value + 1) as u64) as i64;
                writeln!(source, "value -= {operand};").unwrap();
                value -= operand;
            }
            2 => {
                let operand = rng.below(101) as i64;
                writeln!(source, "value = value + {operand};").unwrap();
                value += operand;
            }
            3 => {
                let operand = (rng.below(3) + 1) as i64;
                writeln!(source, "value *= {operand};").unwrap();
                value *= operand;
            }
            4 => {
                let operand = (rng.below(9) + 1) as i64;
                writeln!(source, "value /= {operand};").unwrap();
                value /= operand;
            }
            5 => {
                let operand = (rng.below(97) + 1) as i64;
                writeln!(source, "value %= {operand};").unwrap();
                value %= operand;
            }
            6 => {
                let operand = rng.below(1 << 17) as i64;
                writeln!(source, "value ^= {operand};").unwrap();
                value ^= operand;
            }
            7 => {
                let operand = rng.below(1 << 17) as i64;
                writeln!(source, "value &= {operand};").unwrap();
                value &= operand;
            }
            8 => {
                let operand = rng.below(1 << 17) as i64;
                writeln!(source, "value |= {operand};").unwrap();
                value |= operand;
            }
            9 => {
                let operand = rng.below(4);
                writeln!(source, "value <<= {operand};").unwrap();
                value <<= operand;
            }
            _ => {
                let operand = rng.below(4);
                writeln!(source, "value >>= {operand};").unwrap();
                value >>= operand;
            }
        }
        source.push_str("value %= 100000; println(value); ");
        value %= 100_000;
        writeln!(expected, "{value}").unwrap();

        let threshold = rng.below(100_000) as i64;
        writeln!(source, "println(value >= {threshold});").unwrap();
        expected.push_str(if value >= threshold { "true\n" } else { "false\n" });
    }

    source.push_str("println(\"done\"); }");
    expected.push_str("done\n");
    (source, expected.into_bytes())
}

#[test]
fn malformed_source_has_stable_diagnostic() {
    let directory = TestDirectory::new("malformed");
    let source_path = directory.write_source("bad.sao2", b"print(123);");
    let output = sao2(
        &directory.0,
        &[OsStr::new("build"), source_path.as_os_str()],
    );

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.sao2:1:1: expected top-level 'type' or 'fn' declaration"));
    assert!(!directory.0.join("build/program.c").exists());
}

#[test]
fn missing_configured_compiler_is_a_toolchain_error() {
    let directory = TestDirectory::new("missing compiler");
    let source_path = directory.write_source("hello.sao2", b"fn main() { print(\"hello\"); }");
    let output = Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args([OsStr::new("build"), source_path.as_os_str()])
        .current_dir(&directory.0)
        .env("SAO2_CC", directory.0.join("definitely missing compiler"))
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("configured C compiler"));
}

#[test]
fn failing_compiler_reports_captured_failure() {
    let directory = TestDirectory::new("failing compiler");
    let source_path = directory.write_source("hello.sao2", b"fn main() { print(\"hello\"); }");
    let output = Command::new(env!("CARGO_BIN_EXE_sao2"))
        .args([OsStr::new("build"), source_path.as_os_str()])
        .current_dir(&directory.0)
        .env("SAO2_CC", std::env::current_exe().unwrap())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("using C compiler:"));
    assert!(stderr.contains("C compiler"));
    assert!(stderr.contains("failed with status"));
}

#[test]
fn compiles_and_runs_exact_bytes_twice() {
    let directory = TestDirectory::new("exact output");
    let source = br#"/* before */ fn main() { print // between
        ("\\\"\'\n\r\t\0\x41") /* after */ ; }"#;
    let expected = b"\\\"'\n\r\t\0A";

    let first = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&first),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, expected);

    let second = run_source(&directory, source);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(second.stdout, expected);
}

#[test]
fn runs_an_empty_string() {
    let directory = TestDirectory::new("empty output");
    let output = run_source(&directory, br#"fn main() { print(""); }"#);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn reports_string_lengths_as_bytes_without_strlen() {
    let directory = TestDirectory::new("string length");
    let output = run_source(
        &directory,
        br#"fn main() {
            println("".len());
            println("abc".len());
            println("a\0z".len());
            println("a\n".len());
        }"#,
    );
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"0\n3\n3\n2\n");
}

#[test]
fn compares_and_indexes_strings() {
    let directory = TestDirectory::new("string values");
    let source = br#"fn main() {
        println("same" == "same");
        println("same" != "other");
        println("a" < "b");
        println("a" < "ab");
        println("" < "a");
        println("a\0z" < "b");
        println("abcd"[0] == 'a');
        println("abcd"[-1] == 'd');
        println("abcd"[-4] == 'a');
    }"#;
    let output = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\n");
}

#[test]
fn constructs_compares_projects_and_narrows_tuples() {
    let directory = TestDirectory::new("tuple values");
    let source = br#"
        type Pair(int, int);
        type Nested(Pair, bool);
        type Choice(Pair | int);
        fn identity(value Pair) Pair { value }
        fn main() {
            pair := Pair(20, 22);
            nested := Nested(pair, true);
            println(nested.0.0);
            println(pair == identity(pair));
            choice := Choice(pair);
            if choice is Pair: println(choice.1);
        }
    "#;
    let output = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"20\ntrue\n22\n");
}

#[test]
fn runs_inline_and_referenced_struct_graphs() {
    let directory = TestDirectory::new("struct graph");
    let source = br#"
        type Leaf(value int);
        type Pair(left Leaf, right Leaf, shared &Leaf);
        type Byte(value char);
        type Packed(prefix char, first Byte, second Byte);
        type Wrapper(packed Packed);
        type Numbers(int, int);
        type Choice(int | bool);
        type Carrier(numbers Numbers, choice Choice);
        fn inspect(leaf Leaf) int { leaf.value }
        fn main() {
            old := Leaf(value = 1);
            var pair := Pair(
                left = Leaf(value = 2),
                right = Leaf(value = 3),
                shared = old
            );
            left_alias := pair.left;
            right_alias := pair.right;
            println(left_alias == pair.left);
            println(left_alias == right_alias);
            pair.left = Leaf(value = 9);
            println(left_alias.value);
            pair.shared = Leaf(value = 4);
            println(old.value);
            println(pair.shared.value);
            println(inspect(left_alias));
            wrapper := Wrapper(packed = Packed(
                prefix = 'x',
                first = Byte(value = 'a'),
                second = Byte(value = 'b')
            ));
            first_byte := wrapper.packed.first;
            second_byte := wrapper.packed.second;
            println(first_byte == wrapper.packed.first);
            println(first_byte == second_byte);
            println(first_byte.value);
            carrier := Carrier(numbers = Numbers(5, 6), choice = Choice(7));
            println(carrier.numbers.1);
            choice := carrier.choice;
            if choice is int: println(choice);
        }
    "#;
    let output = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"true\nfalse\n9\n1\n4\n9\ntrue\nfalse\na\n6\n7\n");
}

#[test]
fn runs_struct_returns_and_control_flow() {
    let directory = TestDirectory::new("struct returns");
    let source = br#"
        type Node(value int);
        fn make(value int) Node { Node(value = value) }
        fn main() {
            var total := 0;
            var index := 0;
            while index < 3 {
                node := make(index + 10);
                total += node.value;
                index += 1;
            }
            println(total);
        }
    "#;
    let output = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"33\n");
}

#[test]
fn runs_integrated_gc_graphs_through_the_public_pipeline() {
    let directory = TestDirectory::new("integrated gc graphs");
    let output = run_fixture(
        &directory,
        "integrated gc graphs.sao2",
        include_bytes!("fixtures/gc_integrated.sao2"),
    );
    assert!(
        !compiler_is_missing(&output),
        "native GC integration tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"8\ntrue\ntrue\nfalse\n30\n2\n3\n40\n110\n900\ntrue\n20\n11\n21\ntrue\n11\n11\ntrue\n30\n11\n93\n11\n20\n"
    );
}

#[test]
fn runs_sustained_production_heap_pressure_with_live_roots() {
    let directory = TestDirectory::new("production gc threshold");
    let output = run_fixture(
        &directory,
        "production gc threshold.sao2",
        include_bytes!("fixtures/gc_threshold.sao2"),
    );
    assert!(
        !compiler_is_missing(&output),
        "native GC threshold tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"777\n");
}

#[test]
fn emits_integrated_gc_c_deterministically() {
    let directory = TestDirectory::new("integrated gc determinism");
    let source_path = directory.write_source(
        "integrated gc determinism.sao2",
        include_bytes!("fixtures/gc_integrated.sao2"),
    );
    let first = sao2(
        &directory.0,
        &[OsStr::new("build"), source_path.as_os_str()],
    );
    assert!(
        !compiler_is_missing(&first),
        "native GC determinism tests require a supported C compiler: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let generated = fs::read(directory.0.join("build/program.c")).unwrap();

    let second = sao2(
        &directory.0,
        &[OsStr::new("build"), source_path.as_os_str()],
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read(directory.0.join("build/program.c")).unwrap(), generated);
}

#[test]
fn rejects_invalid_struct_programs_before_c_emission() {
    let cases: &[(&str, &[u8], &str)] = &[
        (
            "immutable member",
            b"type Point(value int); fn main() { point := Point(value = 1); point.value = 2; }",
            "must be declared 'var'",
        ),
        (
            "incompatible field",
            b"type Point(value int); fn main() { Point(value = true); }",
            "expression type does not match expected type",
        ),
        (
            "inline cycle",
            b"type Loop(child Loop); fn main() {}",
            "infinite",
        ),
    ];
    for (label, source, expected) in cases {
        let directory = TestDirectory::new(label);
        let source_path = directory.write_source("invalid struct.sao2", source);
        let output = sao2(&directory.0, &[OsStr::new("build"), source_path.as_os_str()]);
        assert_eq!(output.status.code(), Some(1), "{label}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{label}: {stderr}");
        assert!(!directory.0.join("build/program.c").exists(), "{label}");
    }
}

#[test]
fn reports_string_index_failures() {
    let directory = TestDirectory::new("string index failure");
    let output = run_source(&directory, b"fn main() { \"x\"[1]; }");
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("sao2: panic: string index out of range"), "{stderr}");
    assert!(stderr.contains("program with spaces.sao2:1:"), "{stderr}");
}

#[test]
fn runs_primitive_computation_mutation_and_shadowing() {
    let directory = TestDirectory::new("primitive computation");
    let source = br#"fn main() {
        base := 6;
        var value := base * 7;
        value += 1;
        print("value=");
        println(value);
        println(value > 40);
        {
            value := value >> 1;
            print("shadow=");
            println(value);
        }
        println(value);
    }"#;
    let output = run_source(&directory, source);
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"value=43\ntrue\nshadow=21\n43\n");
}

#[test]
fn returns_integer_main_result() {
    let directory = TestDirectory::new("integer result");
    let output = run_source(&directory, b"fn main() int { 23 }");
    assert!(
        !compiler_is_missing(&output),
        "native end-to-end tests require a supported C compiler: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn fuzzes_safe_primitive_programs_with_reproducible_seeds() {
    let directory = TestDirectory::new("primitive fuzz");
    for seed in fuzz_seeds() {
        eprintln!("SAO2_FUZZ_SEED={seed}");
        let (source, expected) = fuzzy_primitive_program(seed);
        let output = run_source(&directory, source.as_bytes());
        assert!(
            !compiler_is_missing(&output),
            "native fuzz tests require a supported C compiler: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "SAO2_FUZZ_SEED={seed}\nsource:\n{source}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.stdout,
            expected,
            "SAO2_FUZZ_SEED={seed}\nsource:\n{source}"
        );
    }
}
