//! Read published Memory from one immutable project snapshot.

use std::collections::BTreeMap;

use clap::Subcommand;
use clumsiesd::DaemonIpcClient;
use serde_json::{Value, json};

use super::{current_binding, identifier, output::Output, resolve_project, server};

/// Read-only operations on a project's published Memory, including selected organization Memory.
#[derive(Subcommand)]
pub(super) enum MemoryCommand {
    /// List published Memory paths and identifiers without opening a reader.
    List {
        /// Project ID or unique name; defaults to the current directory binding.
        #[arg(long)]
        project: Option<String>,
    },
    /// Read a published Memory by ID or exact path, or all published bodies.
    Show {
        /// Memory ID or exact path from memory list.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        target: Option<String>,
        /// Read all published Memory bodies from the same snapshot.
        #[arg(long)]
        all: bool,
        /// Disambiguate a path shared by project and organization Memory.
        #[arg(long, value_parser = ["project", "org"])]
        scope: Option<String>,
        /// Project ID or unique name; defaults to the current directory binding.
        #[arg(long)]
        project: Option<String>,
    },
}

/// Fetches a single authoritative snapshot and prints only Memory resources.
///
/// # Errors
/// Rejects inaccessible projects, incomplete snapshots, missing or ambiguous targets and output failures.
pub(super) fn run(
    client: &DaemonIpcClient,
    action: MemoryCommand,
    output: &mut Output,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = match &action {
        MemoryCommand::List { project } | MemoryCommand::Show { project, .. } => project,
    };
    let project_id = match project {
        Some(project) => resolve_project(client, project)?,
        None => current_binding(client)?.project_id,
    };
    let state = server(
        client,
        "GET",
        &format!("/api/v1/projects/{}/commit-state", identifier(&project_id)?),
        None,
        None,
    )?;
    if state["ref"]["scope"] != "project" || state["ref"]["project_id"] != project_id {
        return Err("Snapshot reference does not belong to the requested project".into());
    }
    let commit_id = state["ref"]["commit_id"].as_str();
    let snapshot = match commit_id {
        Some(id) => server(
            client,
            "GET",
            &format!("/api/v1/commits/{}", identifier(id)?),
            None,
            None,
        )?,
        None if state["ref"]["commit_id"].is_null() => json!({"tree":{"entries":[]},"blobs":[]}),
        None => return Err("Invalid snapshot reference".into()),
    };
    if let Some(id) = commit_id
        && (snapshot["commit"]["commit_id"] != id
            || snapshot["commit"]["scope"] != "project"
            || snapshot["commit"]["project_id"] != project_id)
    {
        return Err("Downloaded snapshot does not match the requested reference".into());
    }
    let mut items = documents(&snapshot)?;
    match action {
        MemoryCommand::List { .. } => {
            for item in &mut items {
                item.as_object_mut()
                    .expect("document object")
                    .remove("content");
            }
            output.value(&json!({"project_id":project_id,"commit_id":commit_id,"items":items}))
        }
        MemoryCommand::Show {
            target, scope, all, ..
        } => {
            let items = select(items, target.as_deref(), scope.as_deref())?;
            if output.json {
                return output
                    .value(&json!({"project_id":project_id,"commit_id":commit_id,"items":items}));
            }
            if output.verbose {
                output.text(&format!(
                    "Published Memory · {project_id}\nSnapshot: {}\n\n",
                    commit_id.unwrap_or("(not published)")
                ))?;
            }
            if items.is_empty() {
                output.text("No published Memory.\n")?;
            }
            let mut paths = BTreeMap::new();
            for item in &items {
                *paths
                    .entry(item["path"].as_str().expect("validated path"))
                    .or_insert(0) += 1;
            }
            for item in &items {
                let ambiguous = paths[item["path"].as_str().expect("validated path")] > 1;
                output.text(&document_text(item, all, ambiguous, output.verbose))?;
                if all {
                    output.text("\n\n")?;
                }
            }
            Ok(())
        }
    }
}

/// Keeps single-body output literal; collection headings identify files without diagnostic IDs.
fn document_text(item: &Value, all: bool, ambiguous: bool, verbose: bool) -> String {
    let mut text = String::new();
    if all || verbose {
        text.push_str(&format!(
            "File: {}",
            item["path"].as_str().expect("validated path")
        ));
        if ambiguous || verbose {
            text.push_str(&format!(
                " [{}]",
                item["scope"].as_str().expect("validated scope")
            ));
        }
        text.push('\n');
        if verbose {
            text.push_str(&format!(
                "ID: {}\n",
                item["memory_id"].as_str().expect("validated identity")
            ));
        }
        text.push('\n');
    }
    text.push_str(item["content"].as_str().expect("validated body"));
    text
}

/// Selects one unambiguous published identity or a complete scoped collection.
///
/// # Errors
/// Rejects missing targets and paths shared across authority scopes.
fn select(
    mut items: Vec<Value>,
    target: Option<&str>,
    scope: Option<&str>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    items.retain(|item| scope.is_none_or(|scope| item["scope"] == scope));
    if let Some(target) = target {
        items.retain(|item| item["memory_id"] == target || item["path"] == target);
        match items.len() {
            0 => {
                return Err(
                    "Published Memory not found; use clumsies memory list to inspect IDs and paths"
                        .into(),
                );
            }
            1 => {}
            _ => {
                return Err(
                    "Memory path is ambiguous; use a Memory ID or --scope project|org".into(),
                );
            }
        }
    }
    Ok(items)
}

/// Resolves referenced bodies once and excludes directories and selection configuration.
///
/// # Errors
/// Rejects invalid resource metadata, duplicate identities and missing or duplicate blobs.
fn documents(snapshot: &Value) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut blobs = BTreeMap::new();
    for blob in snapshot["blobs"]
        .as_array()
        .ok_or("Snapshot has no blobs")?
    {
        let id = blob["blob_id"].as_str().ok_or("Blob has no identity")?;
        let content = blob["content"].as_str().ok_or("Blob has no text content")?;
        if blobs.insert(id, content).is_some() {
            return Err("Snapshot has duplicate blobs".into());
        }
    }
    let mut identities = std::collections::BTreeSet::new();
    let mut items = Vec::new();
    for entry in snapshot["tree"]["entries"]
        .as_array()
        .ok_or("Snapshot has no entries")?
    {
        if entry["type"] != "memory" || entry["is_directory"] == true {
            continue;
        }
        let id = entry["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Memory has no identity")?;
        let scope = entry["scope"]
            .as_str()
            .filter(|s| matches!(*s, "project" | "org"))
            .ok_or("Memory has invalid scope")?;
        let path = entry["path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Memory has no path")?;
        if !identities.insert((scope, id)) {
            return Err("Snapshot has duplicate Memory identities".into());
        }
        let body = entry["blob_id"]
            .as_str()
            .and_then(|id| blobs.get(id))
            .ok_or("Snapshot is missing a Memory body")?;
        items.push(json!({"memory_id":id,"scope":scope,"path":path,"content":body}));
    }
    items.sort_by(|a, b| {
        a["path"]
            .as_str()
            .cmp(&b["path"].as_str())
            .then(a["scope"].as_str().cmp(&b["scope"].as_str()))
    });
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_output_only_labels_collections_and_opt_in_diagnostics() {
        let item = json!({"memory_id":"mem_private", "scope":"project", "path":"guide.md", "content":"# Meaningful body\n"});
        assert_eq!(
            document_text(&item, false, false, false),
            "# Meaningful body\n"
        );
        assert_eq!(
            document_text(&item, true, false, false),
            "File: guide.md\n\n# Meaningful body\n"
        );
        assert_eq!(
            document_text(&item, true, true, false),
            "File: guide.md [project]\n\n# Meaningful body\n"
        );
        assert!(document_text(&item, false, false, true).contains("ID: mem_private"));
    }

    #[test]
    fn selection_requires_an_unambiguous_identity() {
        let items = vec![
            json!({"memory_id":"m1","scope":"project","path":"README.md"}),
            json!({"memory_id":"m2","scope":"org","path":"README.md"}),
        ];
        assert!(select(items.clone(), Some("README.md"), None).is_err());
        assert_eq!(
            select(items.clone(), Some("README.md"), Some("org")).unwrap()[0]["memory_id"],
            "m2"
        );
        assert_eq!(select(items.clone(), Some("m1"), None).unwrap().len(), 1);
        assert!(select(items.clone(), Some("missing"), None).is_err());
        assert_eq!(select(items, None, None).unwrap().len(), 2);
        assert!(select(vec![], None, None).unwrap().is_empty());
    }

    #[test]
    fn snapshot_bodies_include_selected_org_and_exclude_directories_and_config() {
        let snapshot = json!({"tree":{"entries":[
            {"id":"m2","type":"memory","scope":"project","path":"z.md","blob_id":"b"},
            {"id":"m1","type":"memory","scope":"org","path":"a.md","blob_id":"b"},
            {"id":"dir","type":"memory","is_directory":true},
            {"type":"project_org_selection"}
        ]},"blobs":[{"blob_id":"b","content":"# Body\n"}]});
        let items = documents(&snapshot).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["scope"], "org");
        assert_eq!(items[1]["content"], "# Body\n");
        let mut missing = snapshot.clone();
        missing["blobs"] = json!([]);
        assert!(documents(&missing).is_err());
        let mut duplicate = snapshot.clone();
        duplicate["tree"]["entries"]
            .as_array_mut()
            .unwrap()
            .push(snapshot["tree"]["entries"][0].clone());
        assert!(documents(&duplicate).is_err());
    }
}
