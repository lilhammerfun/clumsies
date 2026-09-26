use std::env;
use std::path::{Path, PathBuf};

use crate::DaemonDraftResourceKind;
use crate::DaemonError;
use crate::DaemonTextReplacement;
use crate::MemoryKind;
use crate::config::ProjectConfig;

pub(crate) fn home_dir() -> Result<PathBuf, DaemonError> {
    env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        DaemonError::InvalidConfig(
            "HOME is required when daemon runtime paths are not configured".to_owned(),
        )
    })
}

pub(crate) fn non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(crate) fn canonical_server_url(server_url: &str) -> Result<String, DaemonError> {
    ProjectConfig {
        server_url: server_url.to_owned(),
        project_id: None,
        memory_guidelines_path: None,
    }
    .validate()?;
    Ok(server_url.trim().trim_end_matches('/').to_owned())
}

pub(crate) fn canonical_workspace_directory(path: &str) -> Result<PathBuf, DaemonError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(DaemonError::InvalidRequest(
            "workspace path must not be empty".to_owned(),
        ));
    }
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        DaemonError::InvalidRequest(format!("workspace path {path} cannot be resolved: {error}"))
    })?;
    if !canonical.is_dir() {
        return Err(DaemonError::InvalidRequest(format!(
            "workspace path {} is not a directory",
            canonical.display()
        )));
    }
    Ok(canonical)
}

/// Canonicalizes a stored binding root so a later workspace move or symlink
/// change (e.g. a repository migrated to an external volume) still matches a
/// canonical request path. Falls back to the raw path when the directory no
/// longer exists so stale bindings keep resolving.
pub(crate) fn canonical_binding_root(root: &str) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| Path::new(root).to_path_buf())
}

pub(crate) fn parse_bool_env(name: &str) -> Result<Option<bool>, DaemonError> {
    let Some(value) = env::var(name).ok() else {
        return Ok(None);
    };
    match value.as_str() {
        "1" | "true" | "TRUE" | "yes" | "YES" => Ok(Some(true)),
        "0" | "false" | "FALSE" | "no" | "NO" => Ok(Some(false)),
        _ => Err(DaemonError::InvalidConfig(format!(
            "{name} must be a boolean value"
        ))),
    }
}

pub(crate) fn parse_u64_env(name: &str) -> Result<Option<u64>, DaemonError> {
    let Some(value) = env::var(name).ok() else {
        return Ok(None);
    };
    value.parse::<u64>().map(Some).map_err(|error| {
        DaemonError::InvalidConfig(format!("{name} must be a positive integer: {error}"))
    })
}

pub(crate) fn is_normalized_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(is_portable_path_segment)
}

fn is_portable_path_segment(segment: &str) -> bool {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.trim() != segment
        || segment.ends_with('.')
        || segment.chars().any(|character| {
            character.is_control()
                || matches!(character, '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !reserved_numbered_name(&stem, "COM")
        && !reserved_numbered_name(&stem, "LPT")
}

fn reserved_numbered_name(stem: &str, prefix: &str) -> bool {
    stem.strip_prefix(prefix)
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
}

pub(crate) fn validate_draft_resource_path(
    _resource: DaemonDraftResourceKind,
    path: &str,
) -> Result<(), DaemonError> {
    if !is_normalized_relative_path(path) {
        return Err(DaemonError::InvalidRequest(format!(
            "resource path is not a portable normalized relative path: {path}"
        )));
    }
    Ok(())
}

/// If `path` is a git worktree (its `.git` is a file pointing at the main
/// repository gitdir), return the main repository root. `None` when `path`
/// has no `.git` entry, the entry is not a file, or the gitdir reference
/// cannot be resolved to a sibling root.
pub(crate) fn git_worktree_main_root(path: &Path) -> Option<PathBuf> {
    let git_entry = path.join(".git");
    let Ok(metadata) = std::fs::metadata(&git_entry) else {
        return None;
    };
    if !metadata.is_file() {
        return None;
    }
    let Ok(content) = std::fs::read_to_string(&git_entry) else {
        return None;
    };
    let gitdir = content.trim().strip_prefix("gitdir: ")?.trim();
    let gitdir_path = PathBuf::from(gitdir);
    // gitdir forms:
    //   /main/repo/.git/worktrees/<name>   -> main root = gitdir parent's parent
    //   /main/repo/.git                    -> main root = gitdir parent
    let root = if gitdir_path.ends_with(".git") {
        gitdir_path.parent()?
    } else if gitdir_path.components().count() >= 3
        && gitdir_path.file_name().is_some_and(|name| name != ".git")
    {
        // .git/worktrees/<name>: parent is worktrees/, grandparent is .git/
        let parent = gitdir_path.parent()?;
        if parent.file_name().is_some_and(|name| name == "worktrees") {
            parent.parent()?.parent()?
        } else {
            return None;
        }
    } else {
        return None;
    };
    std::fs::canonicalize(root).ok()
}

pub(crate) fn memory_kind_matches_resource(
    _kind: MemoryKind,
    _resource: DaemonDraftResourceKind,
) -> bool {
    // The unified Memory model has a single resource kind; the check is
    // retained for call sites while the legacy kind taxonomy is removed.
    true
}

pub(crate) fn apply_exact_text_replacements(
    source: &str,
    replacements: &[DaemonTextReplacement],
) -> Result<String, DaemonError> {
    struct ReplacementSpan<'a> {
        start: usize,
        end: usize,
        new_text: &'a str,
        input_index: usize,
    }

    let mut spans = Vec::with_capacity(replacements.len());
    for (index, replacement) in replacements.iter().enumerate() {
        let mut matches = source.match_indices(&replacement.old_text);
        let Some((start, _)) = matches.next() else {
            return Err(DaemonError::State {
                code: "text_replacement_not_found",
                message: format!(
                    "Text replacement {} did not match the current resource",
                    index + 1
                ),
            });
        };
        if matches.next().is_some() {
            return Err(DaemonError::State {
                code: "text_replacement_ambiguous",
                message: format!(
                    "Text replacement {} matched more than once; include more surrounding text",
                    index + 1
                ),
            });
        }
        spans.push(ReplacementSpan {
            start,
            end: start + replacement.old_text.len(),
            new_text: &replacement.new_text,
            input_index: index,
        });
    }
    spans.sort_by_key(|span| span.start);
    for pair in spans.windows(2) {
        if pair[1].start < pair[0].end {
            return Err(DaemonError::State {
                code: "text_replacement_overlap",
                message: format!(
                    "Text replacements {} and {} overlap",
                    pair[0].input_index + 1,
                    pair[1].input_index + 1
                ),
            });
        }
    }

    let mut result = source.to_owned();
    for span in spans.into_iter().rev() {
        result.replace_range(span.start..span.end, span.new_text);
    }
    if result == source {
        return Err(DaemonError::State {
            code: "text_replacement_no_change",
            message: "Text replacements did not change the resource".to_owned(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::canonical_binding_root;
    use super::git_worktree_main_root;

    #[test]
    fn git_worktree_main_root_resolves_worktree_gitdir_reference() {
        let root =
            std::env::temp_dir().join(format!("clumsies-worktree-root-{}", std::process::id()));
        let main = root.join("repo");
        let worktrees = root.join("worktrees");
        let wt = worktrees.join("feature");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        // Simulate a git worktree: .git is a file pointing at the main gitdir.
        std::fs::write(
            wt.join(".git"),
            format!("gitdir: {}/.git/worktrees/feature\n", main.display()),
        )
        .unwrap();

        let resolved = git_worktree_main_root(&wt).unwrap();
        assert_eq!(
            std::fs::canonicalize(&resolved).unwrap(),
            std::fs::canonicalize(&main).unwrap()
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn git_worktree_main_root_returns_none_without_git_file() {
        let dir =
            std::env::temp_dir().join(format!("clumsies-worktree-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(git_worktree_main_root(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_worktree_main_root_ignores_directory_git_entry() {
        let dir =
            std::env::temp_dir().join(format!("clumsies-worktree-dirgit-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(git_worktree_main_root(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn canonical_binding_root_follows_a_replaced_workspace_symlink() {
        use std::os::unix::fs::symlink;

        let root =
            std::env::temp_dir().join(format!("clumsies-canonical-root-{}", std::process::id()));
        let real = root.join("real");
        let link = root.join("link");
        std::fs::create_dir_all(&real).unwrap();
        symlink(&real, &link).unwrap();

        let canonical = canonical_binding_root(link.to_str().unwrap());
        assert_eq!(
            std::fs::canonicalize(&canonical).unwrap(),
            std::fs::canonicalize(&real).unwrap()
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn canonical_binding_root_falls_back_to_the_raw_path_when_missing() {
        let missing = std::env::temp_dir().join("clumsies-canonical-root-missing");
        let _ = std::fs::remove_dir_all(&missing);

        let canonical = canonical_binding_root(missing.to_str().unwrap());
        assert_eq!(canonical, missing);
    }
}
