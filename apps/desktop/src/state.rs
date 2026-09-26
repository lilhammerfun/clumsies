//! What the window remembers between runs.
//!
//! Which documents a reader had open is a fact about this window, not about the
//! Project: the Memory of a Project belongs to the daemon, and macOS keeps the
//! same thing in its workspace model. So it lives in a file of its own, under
//! the state directory rather than the config directory, because a client that
//! loses it loses nothing a reader typed.
//!
//! Anything unreadable is simply no memory at all: a client that refused to
//! start over a file it wrote itself would be worse than one that forgets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The documents the reader had open, and which one was in front.
#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WindowState {
    #[serde(default)]
    pub project_id: Option<String>,
    /// The paths of the open documents, in the order the strip showed them.
    #[serde(default)]
    pub open: Vec<String>,
    /// The path of the document that was in front.
    #[serde(default)]
    pub active: Option<String>,
}

/// Where the state lives: one small file under `XDG_STATE_HOME`, or the
/// directory that stands in for it.
fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("clumsies").join("desktop.json"))
}

/// Reads what the last run left. Nothing there, or nothing readable, is an
/// empty state.
pub fn load() -> WindowState {
    path().map(|path| read(&path)).unwrap_or_default()
}

/// Writes what this run has open. A state that cannot be written is reported
/// and otherwise ignored: it is not the work a reader did.
pub fn save(state: &WindowState) {
    let Some(path) = path() else {
        return;
    };
    if let Err(error) = write(&path, state) {
        crate::logging::error(&format!("could not write {}: {error}", path.display()));
    }
}

fn read(path: &Path) -> WindowState {
    let Ok(text) = std::fs::read_to_string(path) else {
        return WindowState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write(path: &Path, state: &WindowState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    // Only what the tests use: the component library exports a `test` macro of
    // its own, and a glob import would shadow the built-in attribute with it.
    use super::{WindowState, read, write};

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("clumsies-state-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn what_was_written_is_what_is_read() {
        let path = scratch("roundtrip");
        let _ = std::fs::remove_file(&path);
        let state = WindowState {
            project_id: Some("prj_one".to_owned()),
            open: vec!["a.md".to_owned(), "b/c.md".to_owned()],
            active: Some("b/c.md".to_owned()),
        };
        write(&path, &state).expect("the state should be writable");
        assert_eq!(read(&path), state);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nothing_readable_is_no_memory_at_all() {
        let path = scratch("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(read(&path), WindowState::default());

        std::fs::write(&path, "not json").expect("the file should be writable");
        assert_eq!(read(&path), WindowState::default());
        let _ = std::fs::remove_file(&path);
    }
}
