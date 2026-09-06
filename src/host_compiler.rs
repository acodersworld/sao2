use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flavor {
    Msvc,
    GnuLike,
}

pub fn compile(generated_c: &Path) -> Result<PathBuf, Diagnostic> {
    let compiler = find_compiler()?;
    eprintln!("using C compiler: {}", compiler.display());
    let flavor = compiler_flavor(&compiler);
    let executable = generated_c.with_file_name(if cfg!(windows) {
        "program.exe"
    } else {
        "program"
    });

    remove_stale_executable(&executable)?;
    let arguments = compiler_arguments(flavor, generated_c, &executable);
    let output = Command::new(&compiler)
        .args(&arguments)
        .output()
        .map_err(|error| {
            Diagnostic::compiler(format!(
                "failed to start C compiler '{}': {error}",
                compiler.display()
            ))
        })?;

    if !output.status.success() {
        let mut details = String::new();
        append_output(&mut details, "stdout", &output.stdout);
        append_output(&mut details, "stderr", &output.stderr);
        return Err(Diagnostic::compiler(format!(
            "C compiler '{}' failed with status {}{}",
            compiler.display(),
            output.status,
            details
        )));
    }
    if !executable.is_file() {
        return Err(Diagnostic::compiler(format!(
            "C compiler '{}' succeeded but did not create '{}'",
            compiler.display(),
            executable.display()
        )));
    }

    Ok(executable)
}

fn find_compiler() -> Result<PathBuf, Diagnostic> {
    if let Some(configured) = env::var_os("SAO2_CC") {
        return find_program(&configured).ok_or_else(|| {
            Diagnostic::compiler(format!(
                "configured C compiler '{}' was not found",
                configured.to_string_lossy()
            ))
        });
    }

    let candidates: &[&str] = if cfg!(windows) {
        &["cl", "clang", "gcc", "cc"]
    } else {
        &["cc", "clang", "gcc"]
    };
    candidates
        .iter()
        .find_map(|candidate| find_program(OsStr::new(candidate)))
        .ok_or_else(|| {
            Diagnostic::compiler(
                "no supported C compiler found; install cl, clang, gcc, or cc, or set SAO2_CC",
            )
        })
}

fn find_program(program: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }

    let search_path = env::var_os("PATH")?;
    for directory in env::split_paths(&search_path) {
        for candidate in executable_names(program) {
            let path = directory.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn executable_names(program: &OsStr) -> Vec<OsString> {
    if !cfg!(windows) || Path::new(program).extension().is_some() {
        return vec![program.to_os_string()];
    }
    let extensions = env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    extensions
        .to_string_lossy()
        .split(';')
        .map(|extension| {
            let mut name = program.to_os_string();
            name.push(extension.to_ascii_lowercase());
            name
        })
        .collect()
}

fn compiler_flavor(compiler: &Path) -> Flavor {
    let stem = compiler
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if stem == "cl" {
        Flavor::Msvc
    } else {
        Flavor::GnuLike
    }
}

fn compiler_arguments(flavor: Flavor, input: &Path, output: &Path) -> Vec<OsString> {
    match flavor {
        Flavor::Msvc => vec![
            "/nologo".into(),
            "/TC".into(),
            "/W4".into(),
            input.as_os_str().to_owned(),
            format!("/Fe:{}", output.display()).into(),
            format!("/Fo:{}", input.with_extension("obj").display()).into(),
        ],
        Flavor::GnuLike => vec![
            "-std=c11".into(),
            "-Wall".into(),
            "-Wextra".into(),
            "-pedantic".into(),
            input.as_os_str().to_owned(),
            "-o".into(),
            output.as_os_str().to_owned(),
        ],
    }
}

fn remove_stale_executable(path: &Path) -> Result<(), Diagnostic> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Diagnostic::compiler(format!(
            "cannot replace executable '{}': {error}",
            path.display()
        ))),
    }
}

fn append_output(message: &mut String, label: &str, bytes: &[u8]) {
    if !bytes.is_empty() {
        message.push_str(&format!(
            "\n{label}:\n{}",
            String::from_utf8_lossy(bytes).trim_end()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_compiler_flavor_from_executable_name() {
        assert_eq!(compiler_flavor(Path::new("cl.exe")), Flavor::Msvc);
        assert_eq!(compiler_flavor(Path::new("clang.exe")), Flavor::GnuLike);
        assert_eq!(compiler_flavor(Path::new("gcc")), Flavor::GnuLike);
    }

    #[test]
    fn creates_gnu_arguments_without_shell_quoting() {
        let arguments = compiler_arguments(
            Flavor::GnuLike,
            Path::new("build/a file.c"),
            Path::new("build/a file.exe"),
        );
        assert_eq!(arguments[4], OsString::from("build/a file.c"));
        assert_eq!(arguments[5], OsString::from("-o"));
        assert_eq!(arguments[6], OsString::from("build/a file.exe"));
    }

    #[test]
    fn creates_msvc_arguments() {
        let arguments = compiler_arguments(
            Flavor::Msvc,
            Path::new("build/program.c"),
            Path::new("build/program.exe"),
        );
        assert!(arguments.contains(&OsString::from("/TC")));
        assert!(arguments.contains(&OsString::from("/Fe:build/program.exe")));
        assert!(arguments.contains(&OsString::from("/Fo:build/program.obj")));
    }
}
