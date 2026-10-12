//! Human presentation and bounded terminal paging, separate from protocol responses.

use std::io::{self, IsTerminal, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::Value;

/// One command's output destination; short output stays out of a pager.
pub(super) struct Output {
    /// Explicit machine-readable mode, independent of terminal detection.
    pub json: bool,
    /// Product operation used to select a human presentation.
    kind: &'static str,
    /// Include diagnostic fields in text mode.
    pub(super) verbose: bool,
    /// Whether this command can launch an interactive reader.
    paging: bool,
    /// Initial screenful, bounded before switching to streaming output.
    pending: Vec<u8>,
    /// Reader process and its input; never used for JSON or redirected output.
    reader: Option<(Child, ChildStdin)>,
    /// Number of lines that fit in the initial screen.
    screen_lines: usize,
    /// Whether the initial screen has already been flushed.
    started: bool,
}

impl Output {
    /// Chooses a presentation without spawning a reader or printing anything.
    pub fn new(
        json: bool,
        no_pager: bool,
        verbose: bool,
        kind: &'static str,
        readable: bool,
    ) -> Self {
        let paging = !json
            && !no_pager
            && readable
            && io::stdout().is_terminal()
            && io::stdin().is_terminal()
            && std::env::var("TERM").as_deref() != Ok("dumb");
        Self {
            json,
            kind,
            verbose,
            paging,
            pending: Vec::new(),
            reader: None,
            started: false,
            screen_lines: std::env::var("LINES")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| (5..=200).contains(n))
                .unwrap_or(24)
                .saturating_sub(2),
        }
    }

    /// Prints a response in explicit JSON mode or the command's human presentation.
    ///
    /// # Errors
    /// Returns serialization or output failures, including reader cancellation.
    pub fn value(
        &mut self,
        value: &impl serde::Serialize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(value)?;
        if self.json {
            self.text_raw(&format!("{}\n", serde_json::to_string_pretty(&value)?))?;
        } else {
            self.text(&render(self.kind, &value, self.verbose))?;
        }
        Ok(())
    }

    /// Renders one collection page without retaining preceding response objects.
    ///
    /// # Errors
    /// Rejects malformed collection shapes and propagates output failures.
    pub fn page(&mut self, value: &Value, first: bool) -> Result<(), Box<dyn std::error::Error>> {
        let items = value["items"]
            .as_array()
            .ok_or("List response requires items")?;
        if first {
            self.text(&list_header(self.kind, self.verbose))?;
        }
        for item in items {
            if self.verbose {
                let mut text = String::new();
                evidence(&mut text, item, 0);
                self.text(&text)?;
            } else {
                self.text(&list_item(self.kind, item))?;
            }
        }
        Ok(())
    }

    /// Escapes untrusted terminal control characters while preserving text layout.
    ///
    /// # Errors
    /// Returns output or reader cancellation errors.
    pub fn text(&mut self, text: &str) -> io::Result<()> {
        self.text_raw(&safe_text(text))
    }

    /// Writes bounded chunks, allowing pipe backpressure to limit producer read-ahead.
    ///
    /// # Errors
    /// Returns stream errors and notices an exited pager before writing more data.
    fn text_raw(&mut self, text: &str) -> io::Result<()> {
        for chunk in text.as_bytes().chunks(4096) {
            if !self.started && self.paging {
                self.pending.extend_from_slice(chunk);
                if self.pending.iter().filter(|b| **b == b'\n').count() < self.screen_lines
                    && self.pending.len() < 8192
                {
                    continue;
                }
                self.start();
                let pending = std::mem::take(&mut self.pending);
                self.write_chunk(&pending)?;
            } else {
                self.started = true;
                self.write_chunk(chunk)?;
            }
        }
        Ok(())
    }

    /// Starts a configured reader, falling back before any bytes have been sent.
    fn start(&mut self) {
        self.started = true;
        let selected = std::env::var("CLUMSIES_PAGER")
            .ok()
            .or_else(|| std::env::var("PAGER").ok());
        let mut command = if let Some(selected) = selected {
            if selected.trim().is_empty() || selected == "cat" {
                return;
            }
            let trimmed = selected.trim_start();
            let name = if trimmed.starts_with(['\'', '"']) {
                let quote = trimmed.chars().next().unwrap();
                trimmed[1..].split(quote).next().unwrap_or("")
            } else {
                trimmed.split_whitespace().next().unwrap_or("")
            };
            if executable(name).is_none() && !(cfg!(windows) && name.eq_ignore_ascii_case("type")) {
                eprintln!(
                    "Pager executable not found; displaying directly: {}",
                    safe_text(name)
                );
                return;
            }
            shell_reader(&selected)
        } else if let Some(path) = executable("less") {
            let mut cmd = Command::new(path);
            cmd.arg(if columns(self.kind).is_empty() {
                "-FRX"
            } else {
                "-FRXS"
            });
            cmd
        } else if cfg!(windows) {
            if executable("more").is_none() {
                return;
            }
            shell_reader("more")
        } else if let Some(path) = executable("more") {
            Command::new(path)
        } else {
            return;
        };
        match command.stdin(Stdio::piped()).spawn() {
            Ok(mut child) => {
                if let Some(input) = child.stdin.take() {
                    self.reader = Some((child, input));
                }
            }
            Err(error) => eprintln!("Could not start pager; displaying directly: {error}"),
        }
    }

    /// Flushes directly or through the selected reader without panicking on a closed pipe.
    ///
    /// # Errors
    /// Returns write failures or a closed-reader cancellation.
    fn write_chunk(&mut self, chunk: &[u8]) -> io::Result<()> {
        if let Some((child, input)) = &mut self.reader {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Err(io::ErrorKind::BrokenPipe.into())
                } else {
                    Err(io::Error::other(format!("Pager failed: {status}")))
                };
            }
            if let Err(error) = input.write_all(chunk) {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    let status = child.wait()?;
                    if !status.success() {
                        return Err(io::Error::other(format!("Pager failed: {status}")));
                    }
                }
                return Err(error);
            }
            input.flush()
        } else {
            let mut stdout = io::stdout().lock();
            stdout.write_all(chunk)?;
            stdout.flush()
        }
    }

    /// Flushes a short result and closes reader input before waiting for the reader.
    ///
    /// # Errors
    /// Returns output or process-wait failures.
    pub fn finish(&mut self) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            self.write_chunk(&pending)?;
        }
        if let Some((mut child, input)) = self.reader.take() {
            drop(input);
            let status = child.wait()?;
            if !status.success() {
                return Err(io::Error::other(format!("Pager failed: {status}")));
            }
        }
        Ok(())
    }
}

/// Resolves an optional terminal reader using the current user's executable search path.
fn executable(name: &str) -> Option<std::path::PathBuf> {
    let explicit = std::path::Path::new(name);
    if explicit.is_absolute() || explicit.components().count() > 1 {
        return explicit.is_file().then(|| explicit.to_owned());
    }
    let suffixes: &[&str] = if cfg!(windows) {
        &["", ".exe", ".com", ".cmd", ".bat"]
    } else {
        &[""]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        suffixes
            .iter()
            .map(|suffix| dir.join(format!("{name}{suffix}")))
            .find(|path| path.is_file())
    })
}

/// Interprets only the user's explicitly configured pager command, never server content.
fn shell_reader(value: &str) -> Command {
    let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    command
        .arg(if cfg!(windows) { "/C" } else { "-c" })
        .arg(value);
    command
}

/// Renders terminal controls visibly so server text cannot issue terminal commands.
pub(super) fn safe_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            safe.push_str(&format!("\\u{{{:x}}}", c as u32));
        } else {
            safe.push(c);
        }
    }
    safe
}

/// Human title of the command's list result.
fn heading(kind: &str) -> &str {
    match kind {
        "projects" => "Projects",
        "drafts" | "drafts-global" => "Drafts",
        "reviews" => "Reviews",
        "comments" => "Comments",
        "members" => "Members",
        _ => "Results",
    }
}

/// Gets a scalar without JSON quoting; missing data is explicitly unavailable.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "—".to_owned(),
        Value::Array(values) => values.iter().map(scalar).collect::<Vec<_>>().join(", "),
        Value::Object(values) => values
            .iter()
            .map(|(k, v)| format!("{k}: {}", scalar(v)))
            .collect::<Vec<_>>()
            .join(", "),
        _ => value.to_string(),
    }
}

/// Fixed display widths keep columns aligned across streamed pages without buffering the list.
fn columns(kind: &str) -> Vec<(&'static str, usize)> {
    match kind {
        "memories" => vec![("ID", 36), ("SCOPE", 7), ("PATH", 0)],
        "projects" => vec![
            ("ID", 36),
            ("NAME", 24),
            ("YOUR ROLE", 10),
            ("OWNER", 24),
            ("MEMBERS", 7),
            ("UPDATED", 16),
            ("DESCRIPTION", 0),
        ],
        "drafts" => vec![
            ("ID", 40),
            ("STATUS", 10),
            ("SCOPE", 7),
            ("SYNC", 14),
            ("UPDATED", 16),
            ("PATH", 0),
        ],
        "drafts-global" => vec![
            ("ID", 40),
            ("PROJECT", 36),
            ("STATUS", 10),
            ("SCOPE", 7),
            ("SYNC", 14),
            ("UPDATED", 16),
            ("PATH", 0),
        ],
        "reviews" => vec![
            ("ID", 36),
            ("STATUS", 10),
            ("VERSION", 7),
            ("AUTHOR", 24),
            ("UPDATED", 16),
            ("TITLE", 0),
        ],
        "members" => vec![("ID", 36), ("ROLE", 10), ("NAME", 24), ("EMAIL", 0)],
        "comments" => vec![("ID", 36), ("AUTHOR", 24), ("CREATED", 16), ("BODY", 0)],
        _ => Vec::new(),
    }
}

/// Fits a single-line cell using terminal display width, including CJK and combining characters.
fn cell(value: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let value = safe_text(value).replace(['\n', '\r', '\t'], " ");
    if width == 0 {
        return value;
    }
    let mut value = value;
    if UnicodeWidthStr::width(value.as_str()) > width {
        let mut used = 0;
        value = value
            .chars()
            .take_while(|ch| {
                used += UnicodeWidthChar::width(*ch).unwrap_or(0);
                used < width
            })
            .collect();
        value.push('…');
    }
    let padding = width.saturating_sub(UnicodeWidthStr::width(value.as_str()));
    format!("{value}{}", " ".repeat(padding))
}

/// Formats one row with stable widths; the last column preserves the complete value.
fn row(kind: &str, values: &[String]) -> String {
    let columns = columns(kind);
    let cells: Vec<_> = columns
        .iter()
        .zip(values)
        .map(|((_, width), value)| cell(value, *width))
        .collect();
    format!("{}\n", cells.join("  "))
}

/// Emits a header once per collection, including an empty collection.
fn list_header(kind: &str, verbose: bool) -> String {
    if verbose || columns(kind).is_empty() {
        return format!("{}\n", heading(kind));
    }
    row(
        kind,
        &columns(kind)
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
}

/// Chooses a human identity without expanding the entire account object.
fn person(value: &Value) -> String {
    if value.is_string() {
        return scalar(value);
    }
    ["display_name", "username", "email", "user_id"]
        .iter()
        .find_map(|key| value[*key].as_str().filter(|s| !s.trim().is_empty()))
        .unwrap_or("(unknown)")
        .to_owned()
}

/// Converts an RFC3339 timestamp to local wall time without displaying subsecond noise.
fn date(value: &Value) -> String {
    value
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| scalar(value))
}

/// Presents list records as tables; full account and synchronization evidence requires verbose mode.
fn list_item(kind: &str, item: &Value) -> String {
    let values = match kind {
        "memories" => vec![
            scalar(&item["memory_id"]),
            scalar(&item["scope"]),
            scalar(&item["path"]),
        ],
        "projects" => vec![
            scalar(&item["project_id"]),
            scalar(&item["name"]),
            item["your_role"].as_str().unwrap_or("n/a").to_owned(),
            item["owner"]["name"]
                .as_str()
                .unwrap_or(if item.get("owner").is_some() {
                    "(no owner)"
                } else {
                    "n/a"
                })
                .to_owned(),
            item.get("member_count")
                .filter(|v| !v.is_null())
                .map(scalar)
                .unwrap_or_else(|| "n/a".to_owned()),
            date(&item["updated_at"]),
            item["description"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("(not set)")
                .to_owned(),
        ],
        "drafts" | "drafts-global" => {
            let pending = item["pending_operation_count"].as_u64().unwrap_or(0);
            let failed = item["failed_operation_count"].as_u64().unwrap_or(0);
            let sync = if failed > 0 {
                format!("{failed} failed")
            } else if pending > 0 {
                format!("{pending} pending")
            } else {
                "synced".to_owned()
            };
            let mut values = vec![scalar(&item["draft_id"])];
            if kind == "drafts-global" {
                values.push(scalar(&item["project_id"]));
            }
            values.extend([
                scalar(&item["status"]),
                scalar(&item["scope"]),
                sync,
                date(&item["updated_at"]),
                item["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("(path unavailable; use draft show)")
                    .to_owned(),
            ]);
            values
        }
        "reviews" => vec![
            scalar(&item["review_id"]),
            scalar(&item["status"]),
            scalar(&item["version"]),
            person(&item["author"]),
            date(&item["updated_at"]),
            scalar(&item["title"]),
        ],
        "members" => vec![
            scalar(&item["user"]["user_id"]),
            scalar(&item["role"]),
            person(&item["user"]),
            scalar(&item["user"]["email"]),
        ],
        "comments" => vec![
            scalar(&item["comment_id"]),
            person(&item["author"]),
            date(&item["created_at"]),
            scalar(&item["body"]),
        ],
        _ => return String::new(),
    };
    row(kind, &values)
}

/// Appends only selected public fields, keeping long or multi-line values readable.
fn fields_text(text: &mut String, value: &Value, fields: &[&str]) {
    for key in fields {
        if let Some(value) = value
            .get(key)
            .filter(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
        {
            if value.as_str().is_some_and(|s| s.trim().is_empty()) {
                continue;
            }
            let shown = if matches!(*key, "author" | "decided_by") {
                person(value)
            } else if key.ends_with("_at") {
                date(value)
            } else {
                scalar(value)
            };
            text.push_str(&format!("{}: {}\n", key.replace('_', " "), shown));
        }
    }
}

/// Renders nested operation evidence without JSON punctuation or dropping semantic fields.
fn evidence(text: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                if matches!(
                    key.as_str(),
                    "access_token" | "refresh_token" | "password" | "token"
                ) {
                    continue;
                }
                text.push_str(&format!("{}{}:", " ".repeat(indent), key.replace('_', " ")));
                if value.is_object() || value.is_array() {
                    text.push('\n');
                    evidence(text, value, indent + 2);
                } else {
                    text.push_str(&format!(" {}\n", scalar(value)));
                }
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                text.push_str(&format!("{}[{}]\n", " ".repeat(indent), index + 1));
                evidence(text, value, indent + 2);
            }
        }
        _ => text.push_str(&format!("{}{}\n", " ".repeat(indent), scalar(value))),
    }
}

/// Describes ordered mutations and non-text metadata without repeating the diff's text body.
fn operation_summary(text: &mut String, operations: &Value) {
    if let Some(operations) = operations.as_array() {
        for (index, operation) in operations.iter().enumerate() {
            text.push_str(&format!("Operation {}:\n", index + 1));
            fields_text(text, operation, &["action", "new_path"]);
            evidence(text, &operation["resource"], 2);
            if let Some(metadata) = operation["content"].as_object() {
                let metadata = Value::Object(
                    metadata
                        .iter()
                        .filter(|(key, _)| key.as_str() != "content")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                evidence(text, &metadata, 2);
            }
        }
    }
}

/// Quotes a displayed command argument without allowing shell expansion.
fn command_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Shows field-level conflicts and the exact guards needed for a later mutation.
fn plan_summary(text: &mut String, value: &Value) {
    let plan = value.get("plan").unwrap_or(value);
    let review = &plan["detail"]["review"];
    if let Some(id) = review["review_id"].as_str() {
        fields_text(text, review, &["review_id", "version", "status"]);
        let reference = plan["candidates"]
            .as_array()
            .and_then(|cs| cs.first())
            .and_then(|c| c["current_commit_id"].as_str())
            .unwrap_or("ref-none");
        text.push_str(&format!("Resolve after inspection: clumsies review update {} --resolve --version {} --reference {}\n", command_arg(id), review["version"], command_arg(reference)));
    }
    let candidates: Vec<&Value> = match plan["candidates"].as_array() {
        Some(candidates) => candidates.iter().collect(),
        None if plan.get("candidate_id").is_some() => vec![plan],
        _ => Vec::new(),
    };
    if candidates.is_empty() {
        text.push_str("No reconciliation candidates. Use --json for the complete plan.\n");
    }
    for candidate in candidates {
        text.push_str("\nProposal:\n");
        fields_text(
            text,
            candidate,
            &[
                "draft_id",
                "candidate_id",
                "draft_version",
                "status",
                "valid",
                "base_commit_id",
                "current_commit_id",
            ],
        );
        for (label, field) in [
            ("Ancestor", "base_state"),
            ("Upstream", "current_state"),
            ("Proposal", "draft_state"),
        ] {
            let state = &candidate[field];
            text.push_str(&format!(
                "{label}: exists={} path={}\n",
                state["exists"],
                state["resource"]["path"].as_str().unwrap_or("(none)")
            ));
        }
        let id = candidate["draft_id"].as_str().unwrap_or("DRAFT_ID");
        fields_text(
            text,
            &candidate["draft_state"]["resource"],
            &["id", "scope"],
        );
        let existence_target = if value.get("local_draft_id").is_some() {
            String::new()
        } else {
            command_arg(id)
        };
        if let Some(conflicts) = candidate["conflicts"].as_array() {
            for conflict in conflicts {
                let field = conflict["field"].as_str().unwrap_or("unknown");
                text.push_str(&format!(
                    "Conflict: {} / {field}\n",
                    conflict["kind"].as_str().unwrap_or("unknown")
                ));
                if conflict["kind"] == "content" && field == "content" {
                    for (label, side) in [("upstream", "current"), ("proposal", "draft")] {
                        text.push_str(
                            &similar::TextDiff::from_lines(
                                conflict["base"].as_str().unwrap_or(""),
                                conflict[side].as_str().unwrap_or(""),
                            )
                            .unified_diff()
                            .header("ancestor", label)
                            .to_string(),
                        );
                        text.push('\n');
                    }
                    text.push_str("Remove every generated conflict marker before saving.\n");
                } else {
                    fields_text(text, conflict, &["base", "current", "draft"]);
                    if conflict["kind"] == "existence" {
                        text.push_str(&format!(
                            "Explicit choice: --keep {} or --delete {}\n",
                            existence_target, existence_target
                        ));
                    } else if conflict["kind"] == "path" || conflict["kind"] == "path_occupied" {
                        text.push_str(&format!(
                            "Explicit choice: --path {}\n",
                            command_arg(&if value.get("local_draft_id").is_some() {
                                "FINAL_PATH".to_owned()
                            } else {
                                format!("{id}=FINAL_PATH")
                            })
                        ));
                    }
                }
            }
        }
        if let Some(local) = value["local_draft_id"].as_str() {
            let reference = candidate["current_commit_id"]
                .as_str()
                .unwrap_or("ref-none");
            text.push_str(&format!("Apply after inspection: clumsies draft rebase {} --candidate {} --version {} --reference {}{}\n", command_arg(local), command_arg(candidate["candidate_id"].as_str().unwrap_or("CANDIDATE")), candidate["draft_version"], command_arg(reference), if candidate["status"] == "conflicts" { " --edit" } else { "" }));
        }
    }
    if value.get("local_draft_id").is_some() {
        text.push_str("For deletion choices, omit --edit and supply --keep or --delete; use --path for path conflicts.\n");
    }
    text.push_str("Use --json to export full candidate evidence; --verbose shows all fields.\n");
}

/// Shows the inspected identity and semantic changes without dumping storage or account metadata.
fn diff_summary(text: &mut String, value: &Value) {
    let entity = value.get("review").or_else(|| value.get("draft"));
    if let Some(entity) = entity {
        if let Some(local) = value["local_draft_id"].as_str() {
            text.push_str(&format!("Local draft: {local}\n"));
        }
        fields_text(
            text,
            entity,
            &[
                "review_id",
                "draft_id",
                "version",
                "status",
                "author",
                "updated_at",
            ],
        );
        if entity.get("review_id").is_some() {
            fields_text(text, entity, &["title", "description"]);
        }
        fields_text(
            text,
            &entity["coordination"],
            &["freshness", "reconciliation", "current_commit_id"],
        );
        text.push_str("\nText diff: + added, - removed, space unchanged; --- before, +++ after.\n");
    }
    if let Some(changes) = value["changes"].as_array() {
        for change in changes {
            let before = scalar(&change["before_path"]);
            let after = scalar(&change["after_path"]);
            text.push_str(&format!(
                "\nFile: {}\n",
                if before == after {
                    after.clone()
                } else {
                    format!("{before} → {after}")
                }
            ));
            let operations = change["operations"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut actions = Vec::new();
            for operation in operations {
                if let Some(action) = operation["action"].as_str()
                    && actions.last().copied() != Some(action)
                {
                    actions.push(action);
                }
            }
            text.push_str(&format!("Change: {}\n", actions.join(" → ")));
            let scope = operations
                .first()
                .and_then(|op| op["resource"]["scope"].as_str())
                .or_else(|| change["base_resource"]["scope"].as_str());
            if let Some(scope) = scope {
                text.push_str(&format!("Scope: {scope}\n"));
            }
            let mut metadata = change["base_resource"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if let Some(kind) = metadata.get("type").cloned() {
                metadata.insert("kind".to_owned(), kind);
            }
            for (index, operation) in operations.iter().enumerate() {
                if let Some(content) = operation["content"].as_object() {
                    let mut content = content.clone();
                    // Updates replace the description; the Server omits None during serialization.
                    content.entry("description").or_insert(Value::Null);
                    for (key, next) in content.iter().filter(|(key, _)| key.as_str() != "content") {
                        let previous = metadata.get(key).cloned().unwrap_or(Value::Null);
                        let empty =
                            |v: &Value| v.is_null() || v.as_str().is_some_and(str::is_empty);
                        if previous != *next && !(empty(&previous) && empty(next)) {
                            text.push_str(&format!(
                                "Metadata (operation {}), {}: {} → {}\n",
                                index + 1,
                                key.replace('_', " "),
                                if empty(&previous) {
                                    "(empty)".to_owned()
                                } else {
                                    scalar(&previous)
                                },
                                if empty(next) {
                                    "(empty)".to_owned()
                                } else {
                                    scalar(next)
                                }
                            ));
                        }
                        metadata.insert(key.clone(), next.clone());
                    }
                }
            }
            let diff = change["unified_diff"].as_str().unwrap_or("");
            if diff.is_empty() {
                text.push_str("No text changes.\n");
            } else {
                text.push('\n');
                text.push_str(diff);
                if !diff.ends_with('\n') {
                    text.push('\n');
                }
            }
        }
    }
}

/// Command-specific summaries retain decision evidence while hiding transport internals.
fn render(kind: &str, value: &Value, verbose: bool) -> String {
    let mut text = String::new();
    if verbose {
        evidence(&mut text, value, 0);
        return text;
    }
    if let Some(items) = value["items"].as_array() {
        text.push_str(&list_header(kind, false));
        for item in items {
            text.push_str(&list_item(kind, item));
        }
        if items.is_empty() {
            text.push_str("No results.\n");
        }
        return text;
    }
    match kind {
        "status" => {
            text.push_str(&format!(
                "Daemon: {}\nAuthentication: {}\nSetup: {}\n",
                scalar(&value["health"]["daemon_version"]),
                if value["session"]["has_access_token"] == true
                    || value["session"]["has_refresh_token"] == true
                {
                    "signed in"
                } else {
                    "not signed in; run clumsies login"
                },
                if value["session"]["ready"] == true {
                    "ready"
                } else {
                    "incomplete"
                }
            ));
            fields_text(
                &mut text,
                &value["session"],
                &["server_url", "project_id", "missing_fields"],
            );
            fields_text(
                &mut text,
                &value["sync"],
                &[
                    "pending_operation_count",
                    "failed_operation_count",
                    "behind_draft_count",
                    "reconciliation_conflict_count",
                    "last_success_at",
                ],
            );
            for channel in ["draft_sync", "commit_sync"] {
                if let Some(state) = value["sync"].get(channel) {
                    text.push_str(&format!("{}:\n", channel.replace('_', " ")));
                    fields_text(&mut text, state, &["state", "last_error"]);
                }
            }
            if let Some(projects) = value["sync"]["unavailable_projects"]
                .as_array()
                .filter(|p| !p.is_empty())
            {
                text.push_str(&format!(
                    "Unavailable projects: {} (local drafts retained; use --verbose for details)\n",
                    projects.len()
                ));
            }
            if !value["retrieval"].is_null() {
                text.push_str("Retrieval readiness:\n");
                evidence(&mut text, &value["retrieval"], 2);
            }
        }
        "diff" => diff_summary(&mut text, value),
        "review" => {
            let review = value
                .get("review")
                .or_else(|| value.get("draft"))
                .unwrap_or(value);
            fields_text(
                &mut text,
                review,
                &[
                    "review_id",
                    "draft_id",
                    "title",
                    "description",
                    "project_id",
                    "scope",
                    "status",
                    "version",
                    "author",
                    "decision_body",
                    "decided_by",
                    "decided_at",
                    "org_contribution",
                    "project_source",
                ],
            );
            if !review["coordination"].is_null() {
                text.push_str("Coordination:\n");
                evidence(&mut text, &review["coordination"], 2);
            }
            if let Some(drafts) = value["drafts"].as_array() {
                text.push_str(&format!("Proposals: {}\n", drafts.len()));
                for entry in drafts {
                    fields_text(
                        &mut text,
                        &entry["draft"],
                        &[
                            "draft_id",
                            "title",
                            "version",
                            "status",
                            "project_id",
                            "base_commit_id",
                        ],
                    );
                    evidence(&mut text, &entry["draft"]["resource"], 2);
                    operation_summary(&mut text, &entry["operations"]);
                }
                if let Some(id) = review["review_id"].as_str() {
                    text.push_str(&format!("Read content: clumsies review diff {id}\n"));
                }
            }
            for field in ["decisions", "comments"] {
                if let Some(data) = value.get(field) {
                    text.push_str(&format!("{field}:\n"));
                    evidence(&mut text, data, 2);
                }
            }
            if let Some(changes) = value["changes"].as_array() {
                for change in changes {
                    fields_text(&mut text, change, &["before_path", "after_path"]);
                    text.push_str("Base resource metadata:\n");
                    evidence(&mut text, &change["base_resource"], 2);
                    operation_summary(&mut text, &change["operations"]);
                    if let Some(diff) = change["unified_diff"].as_str() {
                        text.push_str(diff);
                        text.push('\n');
                    }
                }
            }
            fields_text(&mut text, value, &["commit_id", "applied_operation_count"]);
        }
        "draft" => {
            let detail = value.get("draft").unwrap_or(value);
            let draft = detail.get("draft").unwrap_or(detail);
            fields_text(
                &mut text,
                draft,
                &[
                    "draft_id",
                    "server_draft_id",
                    "project_id",
                    "scope",
                    "path",
                    "status",
                    "server_version",
                    "version",
                    "freshness",
                    "reconciliation",
                    "current_commit_id",
                    "pending_operation_count",
                    "failed_operation_count",
                ],
            );
            fields_text(&mut text, &draft["resource"], &["id", "path", "scope"]);
            fields_text(
                &mut text,
                &draft["coordination"],
                &["current_commit_id", "freshness", "reconciliation"],
            );
            fields_text(
                &mut text,
                value,
                &["rebase_id", "previous_revision_id", "approval_invalidated"],
            );
            if let Some(operations) = value.get("operations").or_else(|| detail.get("operations")) {
                text.push_str("Operations:\n");
                if detail.get("draft").is_some() {
                    operation_summary(&mut text, operations);
                } else {
                    evidence(&mut text, operations, 2);
                }
            }
        }
        "plan" => {
            text.push_str("Reconciliation plan (no changes applied):\n");
            plan_summary(&mut text, value);
        }
        "agent" => {
            text.push_str("Agent integrations:\n");
            evidence(&mut text, value, 0);
        }
        "current" => {
            let project = &value["project"];
            for (label, field) in [
                ("Project", "name"),
                ("ID", "project_id"),
                ("Description", "description"),
            ] {
                let shown = project[field]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("(not set)");
                text.push_str(&format!("{label:12} {shown}\n"));
            }
            text.push_str(&format!(
                "{:<12} {}\n",
                "Updated",
                date(&project["updated_at"])
            ));
            for (label, shown) in [
                ("Workspace", &value["binding"]["workspace_root"]),
                ("Server", &value["binding"]["server_url"]),
                ("Your role", &value["your_role"]),
                ("Members", &value["member_count"]),
            ] {
                text.push_str(&format!("{label:12} {}\n", scalar(shown)));
            }
        }
        "binding" => {
            if value.is_array() {
                evidence(&mut text, value, 0);
                return text;
            }
            fields_text(
                &mut text,
                value,
                &[
                    "project_id",
                    "workspace_root",
                    "server_url",
                    "revision",
                    "removed",
                ],
            );
            if let Some(bindings) = value.get("bindings") {
                evidence(&mut text, bindings, 0);
            }
        }
        "project" => {
            let project = value.get("project").unwrap_or(value);
            fields_text(
                &mut text,
                project,
                &[
                    "name",
                    "project_id",
                    "description",
                    "updated_at",
                    "server_url",
                    "ready",
                    "missing_fields",
                ],
            );
        }
        "comment" => {
            text.push_str("Comment recorded.\n");
            fields_text(
                &mut text,
                value,
                &["comment_id", "review_id", "review_version", "body"],
            );
        }
        "logout" => text.push_str("Signed out. Local drafts and bindings are retained.\n"),
        "daemon" => fields_text(&mut text, value, &["daemon_version", "stopped"]),
        "login" => {
            text.push_str("Signed in.\n");
            fields_text(&mut text, value, &["server_url", "project_id"]);
            text.push_str("Next: clumsies project list\n");
        }
        _ => {
            evidence(&mut text, value, 0);
        }
    }
    if text.is_empty() {
        text.push_str("Done.\n");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_summary_preserves_semantics_without_internal_metadata() {
        let value = json!({"local_draft_id":"local", "draft":{"draft_id":"remote","version":2,"status":"open","updated_at":"2026-10-10T00:00:00Z","author":{"display_name":"Dylan","avatar_url":"private-avatar","email":"private-email"},"coordination":{"freshness":"behind","reconciliation":"conflicts","current_commit_id":"head","candidate_id":"internal-candidate"}},"changes":[{"before_path":"old.md","after_path":"new.md","base_resource":{"blob_id":"internal-blob","scope":"org","description":"Old","type":"memory"},"operations":[{"action":"update","resource":{"id":"internal-id","scope":"org"},"content":{"kind":"memory","description":"New","content":"not repeated"}},{"action":"update","content":{"kind":"memory","description":"New","content":"not repeated"}},{"action":"rename","new_path":"new.md"}],"unified_diff":"--- old.md\n+++ new.md\n@@ -1 +1 @@\n-old\n+new\n"}]});
        let text = render("diff", &value, false);
        for expected in [
            "Local draft: local",
            "draft id: remote",
            "version: 2",
            "Dylan",
            "freshness: behind",
            "reconciliation: conflicts",
            "current commit id: head",
            "old.md → new.md",
            "Change: update → rename",
            "Scope: org",
            "description: Old → New",
            "+new",
        ] {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        for hidden in [
            "private-avatar",
            "private-email",
            "internal-blob",
            "internal-id",
            "internal-candidate",
            "not repeated",
            "Base resource metadata",
            "Operation 1",
        ] {
            assert!(!text.contains(hidden), "{hidden}: {text}");
        }
        assert_eq!(text.matches("description: Old → New").count(), 1);
        assert!(render("diff", &value, true).contains("internal-blob"));
        let cleared = render(
            "diff",
            &json!({"changes":[{"before_path":"same.md","after_path":"same.md","base_resource":{"description":"Old"},"operations":[{"action":"update","content":{"content":"unchanged"}}],"unified_diff":""}]}),
            false,
        );
        assert!(
            cleared.contains("description: Old → (empty)") && cleared.contains("No text changes.")
        );
        for (before, after, action) in [
            ("/dev/null", "created.md", "create"),
            ("deleted.md", "/dev/null", "delete"),
            ("old.md", "renamed.md", "rename"),
        ] {
            let text = render(
                "diff",
                &json!({"changes":[{"before_path":before,"after_path":after,"operations":[{"action":action}],"unified_diff":""}]}),
                false,
            );
            assert!(
                text.contains(before)
                    && text.contains(after)
                    && text.contains(action)
                    && text.contains("No text changes.")
            );
        }
    }

    #[test]
    fn timestamps_are_consistent_in_project_review_lists_and_details() {
        let updated = json!("2026-10-10T00:00:00.123456Z");
        let expected = date(&updated);
        assert_eq!(expected.len(), 16);
        for kind in ["projects", "reviews", "drafts"] {
            let text = render(kind, &json!({"items":[{"updated_at":updated}]}), false);
            assert!(
                text.contains("UPDATED") && text.contains(&expected),
                "{kind}: {text}"
            );
            assert!(!text.contains("123456"));
        }
        let text = render("current", &json!({"project":{"updated_at":updated}}), false);
        assert!(text.contains("Updated") && text.contains(&expected));
        let text = render("project", &json!({"updated_at":updated}), false);
        assert!(text.contains(&expected));
    }

    #[test]
    fn project_table_displays_batch_fields_and_marks_missing_server_data() {
        let projects = json!({"items":[{"project_id":"p","name":"中文项目","your_role":"member","owner":{"user_id":"u","name":"Owner"},"member_count":250,"description":"Purpose"}]});
        let text = render("projects", &projects, false);
        for expected in [
            "YOUR ROLE",
            "OWNER",
            "MEMBERS",
            "DESCRIPTION",
            "Owner",
            "member",
            "250",
            "Purpose",
        ] {
            assert!(text.contains(expected));
        }
        let old = render(
            "projects",
            &json!({"items":[{"project_id":"p","name":"Old Server"}]}),
            false,
        );
        assert!(old.contains("n/a"));
        assert!(!old.contains("(no owner)"));
        let missing_owner = render(
            "projects",
            &json!({"items":[{"project_id":"p","owner":null,"member_count":1}]}),
            false,
        );
        assert!(missing_owner.contains("(no owner)"));
    }

    #[test]
    fn tables_align_unicode_and_hide_account_objects() {
        use unicode_width::UnicodeWidthStr;
        assert_eq!(UnicodeWidthStr::width(cell("中文e\u{301}", 8).as_str()), 8);
        assert_eq!(
            UnicodeWidthStr::width(cell("中文很长的名字", 6).as_str()),
            6
        );
        assert!(!cell("a\nb\tc", 10).contains(['\n', '\t']));
        let page = json!({"items":[{"review_id":"rev_test","status":"open","version":2,"title":"标题","author":{"display_name":"中文作者","avatar_url":"private-url","role":"owner","user_id":"u"}}]});
        let text = render("reviews", &page, false);
        for field in ["ID", "STATUS", "VERSION", "AUTHOR", "TITLE", "中文作者"] {
            assert!(text.contains(field));
        }
        assert!(!text.contains("private-url") && !text.contains("owner"));
        assert!(render("members", &json!({"items":[]}), false).contains("No results."));
        let current = json!({"project":{"name":"AgentOS","project_id":"p","description":""},"binding":{"workspace_root":"/repo","server_url":"https://example.test","revision":2},"member_count":250,"your_role":"member"});
        let text = render("current", &current, false);
        for value in ["AgentOS", "(not set)", "/repo", "member", "250"] {
            assert!(text.contains(value));
        }
        assert!(!text.contains("revision"));
        assert!(render("current", &current, true).contains("revision"));
    }

    #[test]
    fn rebase_receipt_unwraps_server_detail_without_losing_approval_state() {
        let value = json!({"rebase_id":"reb", "previous_revision_id":"previous", "approval_invalidated":true, "draft":{"draft":{"draft_id":"remote", "version":3, "resource":{"path":"final.md", "scope":"org"}}, "operations":[{"action":"update", "content":{"content":"private body", "is_directory":false}}]}});
        let text = render("draft", &value, false);
        for expected in [
            "draft id: remote",
            "version: 3",
            "path: final.md",
            "scope: org",
            "approval invalidated: true",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(!text.contains("private body"));
        assert!(
            render(
                "draft",
                &json!({"draft":{"draft_id":"local","server_version":2},"operations":[{"last_error":"upload denied"}]}),
                false
            )
            .contains("server version: 2")
        );
        assert!(render("draft", &json!({"draft":{"draft_id":"local"},"operations":[{"last_error":"upload denied"}]}), false).contains("upload denied"));
    }

    #[test]
    fn reconciliation_summary_exposes_guards_and_each_conflict_dimension() {
        let candidate = json!({"draft_id":"d", "candidate_id":"c", "draft_version":3, "status":"conflicts", "valid":true, "current_commit_id":"head", "base_state":{"exists":true,"resource":{"path":"old.md"}}, "current_state":{"exists":true,"resource":{"path":"upstream.md"}}, "draft_state":{"exists":true,"resource":{"path":"local.md"}}, "conflicts":[{"kind":"content","field":"content","base":"old\n","current":"upstream\n","draft":"local\n"},{"kind":"path","field":"path","base":"old.md","current":"upstream.md","draft":"local.md"},{"kind":"existence","field":"exists","base":"true","current":"false","draft":"true"}]});
        let text = render(
            "plan",
            &json!({"plan":{"detail":{"review":{"review_id":"r","version":7}}, "candidates":[candidate.clone()]}}),
            false,
        );
        for expected in [
            "--resolve --version 7",
            "candidate id: c",
            "draft version: 3",
            "+upstream",
            "+local",
            "d=FINAL_PATH",
            "--keep 'd'",
            "--delete 'd'",
        ] {
            assert!(text.contains(expected), "{text}");
        }
        let mut local = candidate;
        local["local_draft_id"] = json!("local");
        let text = render("plan", &local, false);
        assert!(
            text.contains("draft rebase 'local' --candidate 'c' --version 3 --reference 'head'")
        );
        assert!(!text.contains("d=FINAL_PATH"));
        assert_eq!(command_arg("a'$b"), "'a'\"'\"'$b'");
    }
    use serde_json::json;

    /// Runs the real output producer in a subprocess with isolated pager configuration.
    #[test]
    #[ignore]
    fn pager_producer_fixture() {
        if std::env::var_os("CLUMSIES_PAGER_CAPTURE").is_none() {
            return;
        }
        let mode = std::env::var("CLUMSIES_PAGER_MODE").unwrap();
        let mut output = Output::new(mode == "json", mode == "disabled", false, "projects", true);
        if mode == "stream" || mode == "cancel" || mode == "missing" {
            output.paging = true;
        }
        let args = super::super::pagination::PageArgs {
            limit: 1,
            cursor: None,
            all: false,
        };
        let mut fetched = 0;
        let result = super::super::pagination::print(&args, &mut output, |_| {
            fetched += 1;
            Ok(
                json!({"items":[{"project_id":format!("p{fetched}"),"name":"x".repeat(4096)}],"next_cursor": if fetched < 200 { Some(fetched.to_string()) } else { None }}),
            )
        });
        if mode == "cancel" {
            assert!(
                matches!(result.unwrap_err().downcast_ref::<io::Error>(), Some(e) if e.kind() == io::ErrorKind::BrokenPipe)
            );
            assert!(fetched < 200, "Reader exit did not stop requests");
        } else {
            result.unwrap();
        }
        output.finish().unwrap();
    }

    /// An actual reader process captures producer bytes or exits without reading.
    #[test]
    #[ignore]
    fn pager_reader_fixture() {
        let Some(path) = std::env::var_os("CLUMSIES_PAGER_CAPTURE") else {
            return;
        };
        if std::env::var("CLUMSIES_PAGER_MODE").as_deref() == Ok("cancel") {
            std::fs::write(path, b"reader exited").unwrap();
            return;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut io::stdin(), &mut bytes).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn reader_streams_cancels_and_is_skipped_for_json_and_redirects() {
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let pager = format!(
            "\"{}\" --ignored --exact output::tests::pager_reader_fixture --nocapture",
            executable.display()
        );
        for mode in [
            "stream",
            "cancel",
            "json",
            "disabled",
            "redirected",
            "missing",
        ] {
            let capture = root.path().join(mode);
            let stdout = std::fs::File::create(root.path().join(format!("{mode}.stdout"))).unwrap();
            let status = Command::new(&executable)
                .args([
                    "--ignored",
                    "--exact",
                    "output::tests::pager_producer_fixture",
                    "--nocapture",
                ])
                .env("CLUMSIES_PAGER_CAPTURE", &capture)
                .env("CLUMSIES_PAGER_MODE", mode)
                .env(
                    "CLUMSIES_PAGER",
                    if mode == "missing" {
                        "clumsies-nonexistent-reader"
                    } else {
                        &pager
                    },
                )
                .stdin(Stdio::null())
                .stdout(stdout)
                .status()
                .unwrap();
            assert!(status.success(), "Producer failed in {mode} mode");
            if mode == "stream" {
                let text = std::fs::read_to_string(capture).unwrap();
                assert!(text.contains("p1") && text.contains("p200"));
            } else if mode == "cancel" {
                assert_eq!(std::fs::read_to_string(capture).unwrap(), "reader exited");
            } else {
                assert!(!capture.exists(), "Reader launched for {mode}");
            }
        }
    }

    #[test]
    fn status_hides_transport_details_and_keeps_recovery_information() {
        let value = json!({"health":{"daemon_version":"1", "local_db":{"path":"private"}}, "session":{"ready":true,"has_access_token":true}, "sync":{"failed_operation_count":2,"unavailable_projects":[{}]}});
        let text = render("status", &value, false);
        assert!(text.contains("failed operation count: 2"));
        assert!(text.contains("local drafts retained"));
        assert!(!text.contains("private") && !text.contains("has access token"));
    }

    #[test]
    fn decision_evidence_and_terminal_controls_remain_visible() {
        let value = json!({"review":{"review_id":"r","version":4,"scope":"org","coordination":{"current_commit_id":"c","freshness":"behind"}},"drafts":[],"changes":[{"operations":[{"action":"update","content":{"description":"changed","org_source":"new","content":"not repeated"}}],"unified_diff":"+changed body"}]});
        let text = render("review", &value, false);
        for expected in [
            "version: 4",
            "scope: org",
            "current commit id: c",
            "freshness: behind",
            "description: changed",
            "org source: new",
        ] {
            assert!(text.contains(expected));
        }
        assert!(!text.contains("not repeated"));
        assert_eq!(
            safe_text("a\u{1b}]52;secret\u{7}\n中"),
            "a\\u{1b}]52;secret\\u{7}\n中"
        );
    }
}
