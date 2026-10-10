//! Bounded text input and editor drafts retained when submission fails.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Reads a text file or '-' stdin without retaining arbitrarily large input.
///
/// # Errors
/// Rejects unreadable, non-UTF-8 or oversized input.
pub(super) fn read(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let source: Box<dyn Read> = if path.as_os_str() == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(std::fs::File::open(path)?)
    };
    let mut text = String::new();
    source.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err("Input exceeds 4 MiB".into());
    }
    Ok(text)
}

/// Resolves one explicit source and submits it, retaining editor input on failure.
///
/// # Errors
/// Returns input, editor or submission failures without silently submitting a fallback.
pub(super) fn submit<T>(
    value: Option<String>,
    file: Option<PathBuf>,
    editor: bool,
    initial: &str,
    send: impl FnOnce(String) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    if editor {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return Err(
                "Editor input requires an interactive terminal; use a file or '-' stdin instead"
                    .into(),
            );
        }
        edit(initial, send)
    } else {
        send(match file {
            Some(path) => read(&path)?,
            None => value.unwrap_or_else(|| initial.to_owned()),
        })
    }
}

/// Opens a private editor buffer; successful exit is the operator's submission action.
///
/// # Errors
/// Keeps the buffer and reports its location on editor, validation or submission failure.
fn edit<T>(
    initial: &str,
    send: impl FnOnce(String) -> Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .ok_or("Set VISUAL or EDITOR, or provide a file/stdin input")?;
    let root = std::env::temp_dir().join(format!("clumsies-edit-{}", uuid::Uuid::new_v4()));
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let mut builder = builder;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;
    let file = root.join("input.txt");
    let result = (|| {
        std::fs::write(&file, initial)?;
        // Only user configuration is interpreted; the file path is a quoted environment value.
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        command.arg(if cfg!(windows) { "/C" } else { "-c" });
        command.arg(if cfg!(windows) {
            format!("{editor} \"%CLUMSIES_EDITOR_FILE%\"")
        } else {
            format!("{editor} \"$CLUMSIES_EDITOR_FILE\"")
        });
        let status = command.env("CLUMSIES_EDITOR_FILE", &file).status()?;
        if !status.success() {
            return Err(
                format!("Editor exited unsuccessfully: {status}; nothing submitted").into(),
            );
        }
        send(read(&file)?)
    })();
    if result.is_ok() {
        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("Could not remove editor buffer {}: {error}", file.display());
        }
    } else {
        eprintln!("Edited input retained at {}", file.display());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_input_preserves_multiline_text_and_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("input with spaces.txt");
        std::fs::write(&file, "first\n第二行\n").unwrap();
        let text = submit(None, Some(file.clone()), false, "unused", Ok).unwrap();
        assert_eq!(text, "first\n第二行\n");
        std::fs::write(&file, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
        assert!(read(&file).is_err());
    }

    #[test]
    fn editor_process_preserves_failed_input_and_cleans_success() {
        for mode in ["success", "reject", "cancel", "oversize", "missing"] {
            let root = tempfile::Builder::new()
                .prefix("editor home with spaces ")
                .tempdir()
                .unwrap();
            let exe = std::env::current_exe().unwrap();
            let mut command = Command::new(&exe);
            command
                .args([
                    "--exact",
                    "input::tests::editor_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .env("CLUMSIES_EDITOR_TEST", mode)
                .env(
                    "VISUAL",
                    format!(
                        "\"{}\" --exact input::tests::editor_child --ignored --nocapture --skip",
                        exe.display()
                    ),
                )
                .env_remove("EDITOR")
                .env("TMPDIR", root.path())
                .env("TEMP", root.path())
                .env("TMP", root.path());
            if mode == "missing" {
                command.env_remove("VISUAL");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{mode}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    #[ignore = "isolated editor driver invoked by parent test"]
    fn editor_fixture() {
        let mode = std::env::var("CLUMSIES_EDITOR_TEST").unwrap();
        let result = edit("original", |text| {
            assert_eq!(text, "edited\n第二行\n");
            if mode == "reject" {
                Err("Submission failed".into())
            } else {
                Ok(())
            }
        });
        let roots: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap().collect();
        if mode == "success" || mode == "missing" {
            assert!(roots.is_empty());
            assert_eq!(result.is_ok(), mode == "success");
        } else {
            assert!(result.is_err());
            assert_eq!(roots.len(), 1);
            let file = roots[0].as_ref().unwrap().path().join("input.txt");
            assert!(file.is_file());
            if mode == "reject" {
                assert_eq!(std::fs::read_to_string(file).unwrap(), "edited\n第二行\n");
            }
        }
    }

    #[test]
    #[ignore = "actual external editor executable invoked by fixture"]
    fn editor_child() {
        let file = std::env::var_os("CLUMSIES_EDITOR_FILE").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let parent = Path::new(&file).parent().unwrap();
            assert_eq!(
                std::fs::metadata(parent).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        let mode = std::env::var("CLUMSIES_EDITOR_TEST").unwrap();
        if mode == "oversize" {
            std::fs::write(file, vec![b'x'; 4 * 1024 * 1024 + 1]).unwrap();
        } else {
            std::fs::write(file, "edited\n第二行\n").unwrap();
        }
        if mode == "cancel" {
            std::process::exit(3);
        }
    }
}
