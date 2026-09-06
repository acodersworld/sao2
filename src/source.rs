use std::fs;
use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;

#[derive(Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

impl SourceFile {
    pub fn load(path: &Path) -> Result<Self, Diagnostic> {
        let metadata = fs::metadata(path).map_err(|error| {
            Diagnostic::input(format!("cannot access '{}': {error}", path.display()))
        })?;
        if !metadata.is_file() {
            return Err(Diagnostic::input(format!(
                "source path '{}' is not a regular file",
                path.display()
            )));
        }

        let bytes = fs::read(path).map_err(|error| {
            Diagnostic::input(format!("cannot read '{}': {error}", path.display()))
        })?;
        let text = String::from_utf8(bytes).map_err(|error| {
            Diagnostic::input(format!(
                "source file '{}' is not valid UTF-8 at byte {}",
                path.display(),
                error.utf8_error().valid_up_to()
            ))
        })?;

        Ok(Self {
            path: path.to_path_buf(),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sao2-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn loads_utf8_and_preserves_path() {
        let path = temporary_path("valid");
        fs::write(&path, "print(\"hello\");").unwrap();

        let source = SourceFile::load(&path).unwrap();
        assert_eq!(source.path, path);
        assert_eq!(source.text, "print(\"hello\");");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_missing_files_and_directories() {
        let missing = temporary_path("missing");
        assert!(SourceFile::load(&missing).is_err());
        assert!(SourceFile::load(std::env::temp_dir().as_path()).is_err());
    }

    #[test]
    fn rejects_invalid_utf8() {
        let path = temporary_path("invalid-utf8");
        fs::write(&path, [0xff, 0xfe]).unwrap();

        let error = SourceFile::load(&path).unwrap_err();
        assert!(error.to_string().contains("not valid UTF-8 at byte 0"));

        fs::remove_file(path).unwrap();
    }
}
