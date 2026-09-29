//! Path operations shared by drag-and-drop, menus and rename dialogs.
use std::collections::{BTreeMap, BTreeSet};

pub fn valid(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.contains(':'))
}

pub fn relocate(
    entries: &[(String, bool)],
    selected: &[String],
    destination: &str,
) -> Result<Vec<(String, String)>, String> {
    if !destination.is_empty() && !valid(destination) {
        return Err("Invalid destination".into());
    }
    let roots: BTreeSet<_> = selected
        .iter()
        .filter(|path| {
            !selected
                .iter()
                .any(|other| other != *path && path.starts_with(&format!("{other}/")))
        })
        .cloned()
        .collect();
    if roots.is_empty() {
        return Err("Select a file or folder".into());
    }
    let mut changes = Vec::new();
    for root in roots {
        if !valid(&root) {
            return Err("Invalid source path".into());
        }
        if destination == root || destination.starts_with(&format!("{root}/")) {
            return Err("A folder cannot be moved into itself".into());
        }
        let name = root.rsplit('/').next().unwrap();
        let target = if destination.is_empty() {
            name.to_owned()
        } else {
            format!("{destination}/{name}")
        };
        for (path, _) in entries
            .iter()
            .filter(|(path, _)| path == &root || path.starts_with(&format!("{root}/")))
        {
            let next = format!("{target}{}", &path[root.len()..]);
            if next != *path {
                changes.push((path.clone(), next));
            }
        }
    }
    validate_changes(entries, &changes)?;
    if changes.is_empty() {
        return Err("The selection is already in this folder".into());
    }
    Ok(changes)
}

pub fn validate_changes(
    entries: &[(String, bool)],
    changes: &[(String, String)],
) -> Result<(), String> {
    let remap: BTreeMap<_, _> = changes
        .iter()
        .map(|(old, new)| (old.as_str(), new.as_str()))
        .collect();
    let mut final_paths = BTreeMap::new();
    for (path, directory) in entries {
        let target = remap.get(path.as_str()).copied().unwrap_or(path);
        if !valid(target) {
            return Err(format!("Invalid path: {target}"));
        }
        if final_paths.insert(target, *directory).is_some() {
            return Err(format!("A file or folder already exists at {target}"));
        }
    }
    for path in final_paths.keys() {
        let mut parent = *path;
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if final_paths.get(prefix) == Some(&false) {
                return Err(format!("A file blocks the destination: {prefix}"));
            }
            parent = prefix;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entries() -> Vec<(String, bool)> {
        [
            ("a", true),
            ("a/x.md", false),
            ("a/deep/y.md", false),
            ("empty", true),
            ("b/x.md", false),
        ]
        .into_iter()
        .map(|(p, d)| (p.into(), d))
        .collect()
    }
    #[test]
    fn preserves_relative_paths_and_deduplicates_nested_selection() {
        let result = relocate(&entries(), &["a".into(), "a/x.md".into()], "dest").unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains(&("a/deep/y.md".into(), "dest/a/deep/y.md".into())));
    }
    #[test]
    fn moves_empty_folders_and_rejects_self_collision_and_traversal() {
        assert_eq!(
            relocate(&entries(), &["empty".into()], "b").unwrap(),
            vec![("empty".into(), "b/empty".into())]
        );
        assert!(relocate(&entries(), &["a".into()], "a/deep").is_err());
        assert!(relocate(&entries(), &["a/x.md".into()], "b").is_err());
        assert!(relocate(&entries(), &["a".into()], "../outside").is_err());
        assert!(relocate(&entries(), &["a".into()], "b/x.md").is_err());
    }
}
