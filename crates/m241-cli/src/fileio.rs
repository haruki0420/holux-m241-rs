//! Reading and writing the files a command works on.
//!
//! Every failure names the file, because `i/o: permission denied` without a path
//! is not actionable. Parent directories are created on write: the documented
//! workflow writes to `backup/stock.bin`, and a user who has not made `backup/`
//! first should not have to learn that from an errno.
//!
//! Refusing to overwrite is separate and explicit: [`ensure_free`] is an error
//! value, not a prompt. A dump is the only way back from a bad flash, and a tool
//! that silently replaces it has removed the user's recovery route.

use std::io::Read as _;
use std::path::Path;

use m241_core::{Error, Result};

/// Read a file whole.
///
/// # Errors
///
/// [`Error::Io`] naming the path.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::io(path.display().to_string(), source))
}

/// Read a file as UTF-8 text.
///
/// # Errors
///
/// [`Error::Io`] naming the path, or [`Error::State`] when the bytes are not
/// UTF-8 — an NMEA capture is ASCII, and decoding lossily would silently replace
/// the bytes this tool is supposed to pass through unchanged.
pub fn read_text(path: &Path) -> Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|_| {
        Error::State(format!(
            "{} is not UTF-8 text; an NMEA capture is ASCII",
            path.display()
        ))
    })
}

/// Write a file whole, creating its parent directories.
///
/// # Errors
///
/// [`Error::Io`] naming the path.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    create_parent(path)?;
    std::fs::write(path, bytes).map_err(|source| Error::io(path.display().to_string(), source))
}

/// Write text, creating the parent directories.
///
/// # Errors
///
/// [`Error::Io`] naming the path.
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    write(path, text.as_bytes())
}

/// Refuse to overwrite an existing file unless `force`.
///
/// # Errors
///
/// [`Error::Safety`] when the file exists and `force` is false. The exit code is
/// 5: this is a refusal to destroy something the user may need, not a parse
/// failure.
pub fn ensure_free(path: &Path, force: bool) -> Result<()> {
    if force || !path.exists() {
        return Ok(());
    }
    Err(Error::Safety {
        reason: format!(
            "{} already exists; pass --force to overwrite it, or choose another path",
            path.display()
        ),
    })
}

/// Refuse an output path that is the input path.
///
/// `m241 fw patch stock.bin -o stock.bin` would rewrite the backup in place, and
/// that file is the only copy of the stock image. Two spellings of the same file
/// (`./stock.bin`, `stock.bin`) canonicalise to the same path, so the check is
/// not a string comparison.
///
/// # Errors
///
/// [`Error::Safety`] when the two paths name the same file.
pub fn ensure_distinct(input: &Path, output: &Path) -> Result<()> {
    let same = match (std::fs::canonicalize(input), std::fs::canonicalize(output)) {
        (Ok(input), Ok(output)) => input == output,
        // The output need not exist yet; the lexical comparison still catches
        // the obvious `-o` mistake.
        _ => input == output,
    };
    if same {
        return Err(Error::Safety {
            reason: format!(
                "the output {} is also the input; the source image must be kept as it is",
                output.display()
            ),
        });
    }
    Ok(())
}

/// Read standard input as UTF-8 text.
///
/// # Errors
///
/// [`Error::Io`] when reading fails, [`Error::State`] when the bytes are not
/// UTF-8.
pub fn read_stdin_text() -> Result<String> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|source| Error::io("<stdin>", source))?;
    Ok(text)
}

fn create_parent(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(parent)
        .map_err(|source| Error::io(parent.display().to_string(), source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch directory per test, removed by the guard.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("m241-cli-fileio-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_write_creates_the_parent_directories() {
        let scratch = Scratch::new("parents");
        let path = scratch.join("backup/stock.bin");
        write(&path, b"bytes").expect("write");
        assert_eq!(read(&path).expect("read"), b"bytes");
    }

    #[test]
    fn a_read_failure_names_the_file() {
        let scratch = Scratch::new("missing");
        let path = scratch.join("nope.bin");
        let err = read(&path).expect_err("no such file");
        assert!(format!("{err}").contains("nope.bin"), "got {err}");
    }

    #[test]
    fn an_existing_file_is_only_overwritten_on_request() {
        let scratch = Scratch::new("clobber");
        let path = scratch.join("stock.bin");
        write(&path, b"first").expect("write");
        let err = ensure_free(&path, false).expect_err("must refuse");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        ensure_free(&path, true).expect("--force allows it");
        ensure_free(&scratch.join("other.bin"), false).expect("a new path is free");
    }

    #[test]
    fn the_input_must_not_also_be_the_output() {
        let scratch = Scratch::new("distinct");
        let source = scratch.join("stock.bin");
        write(&source, b"image").expect("write");
        let err = ensure_distinct(&source, &source).expect_err("same file");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");

        // The same file spelled differently canonicalises to the same path.
        let spelled = scratch.join("./stock.bin");
        let err = ensure_distinct(&source, &spelled).expect_err("same file, other spelling");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");

        ensure_distinct(&source, &scratch.join("patched.bin")).expect("different files");
    }

    #[test]
    fn text_that_is_not_utf8_is_refused_rather_than_replaced() {
        let scratch = Scratch::new("utf8");
        let path = scratch.join("capture.nmea");
        write(&path, &[0xFF, 0xFE, 0x00]).expect("write");
        let err = read_text(&path).expect_err("not UTF-8");
        assert!(format!("{err}").contains("UTF-8"), "got {err}");
    }
}
