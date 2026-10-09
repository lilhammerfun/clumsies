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
    verbose: bool,
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
            self.text(&format!("{}\n", heading(self.kind)))?;
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
            cmd.arg("-FRX");
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
        &[".exe", ".com", ".cmd", ".bat"]
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
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                format!("\\u{{{:x}}}", c as u32).chars().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Human title of the command's list result.
fn heading(kind: &str) -> &str {
    match kind {
        "projects" => "Projects",
        "drafts" => "Drafts",
        "reviews" => "Reviews",
        "comments" => "Comments",
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

/// Compact multi-line list records keep IDs and paths copyable even in narrow terminals.
fn list_item(kind: &str, item: &Value) -> String {
    match kind {
        "projects" => format!(
            "{}  {}\n",
            scalar(&item["name"]),
            scalar(&item["project_id"])
        ),
        "drafts" => format!(
            "{}  {}  {}\n  project: {}  scope: {}  sync pending/failed: {}/{}  updated: {}\n",
            scalar(&item["draft_id"]),
            scalar(&item["status"]),
            scalar(&item["path"]),
            scalar(&item["project_id"]),
            scalar(&item["scope"]),
            scalar(&item["pending_operation_count"]),
            scalar(&item["failed_operation_count"]),
            scalar(&item["updated_at"])
        ),
        "reviews" => format!(
            "{}  {}  v{}  {}\n  project: {}  author: {}  updated: {}\n",
            scalar(&item["review_id"]),
            scalar(&item["status"]),
            scalar(&item["version"]),
            scalar(&item["title"]),
            scalar(&item["project_id"]),
            scalar(&item["author"]),
            scalar(&item["updated_at"])
        ),
        "comments" => format!(
            "{}  {}  {}\n{}\n\n",
            scalar(&item["comment_id"]),
            scalar(&item["author"]),
            scalar(&item["created_at"]),
            scalar(&item["body"])
        ),
        _ => String::new(),
    }
}

/// Appends only selected public fields, keeping long or multi-line values readable.
fn fields_text(text: &mut String, value: &Value, fields: &[&str]) {
    for key in fields {
        if let Some(value) = value
            .get(key)
            .filter(|v| !v.is_null() && !v.as_array().is_some_and(Vec::is_empty))
        {
            text.push_str(&format!("{}: {}\n", key.replace('_', " "), scalar(value)));
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

/// Command-specific summaries retain decision evidence while hiding transport internals.
fn render(kind: &str, value: &Value, verbose: bool) -> String {
    let mut text = String::new();
    if verbose {
        evidence(&mut text, value, 0);
        return text;
    }
    if let Some(items) = value["items"].as_array() {
        text.push_str(&format!("{}\n", heading(kind)));
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
        "review" | "diff" => {
            let review = value.get("review").unwrap_or(value);
            fields_text(
                &mut text,
                review,
                &[
                    "review_id",
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
                for draft in drafts {
                    evidence(&mut text, draft, 2);
                }
            }
            for field in ["decisions", "comments", "changes"] {
                if let Some(data) = value.get(field) {
                    text.push_str(&format!("{field}:\n"));
                    evidence(&mut text, data, 2);
                }
            }
            fields_text(&mut text, value, &["commit_id", "applied_operation_count"]);
        }
        "draft" => {
            let draft = value.get("draft").unwrap_or(value);
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
                    "freshness",
                    "reconciliation",
                    "current_commit_id",
                    "pending_operation_count",
                    "failed_operation_count",
                ],
            );
            if let Some(operations) = value.get("operations") {
                text.push_str("Operations:\n");
                evidence(&mut text, operations, 2);
            }
        }
        "plan" => {
            text.push_str("Reconciliation plan (no changes applied):\n");
            evidence(&mut text, value, 0);
            text.push_str("Use --json to export an editable machine-readable plan.\n");
        }
        "agent" => {
            text.push_str("Agent integrations:\n");
            evidence(&mut text, value, 0);
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
        let value = json!({"review":{"review_id":"r","version":4,"scope":"org","coordination":{"current_commit_id":"c","freshness":"behind"}},"drafts":[],"changes":[{"description":"changed","source":"new"}]});
        let text = render("review", &value, false);
        for expected in [
            "version: 4",
            "scope: org",
            "current commit id: c",
            "freshness: behind",
            "description: changed",
            "source: new",
        ] {
            assert!(text.contains(expected));
        }
        assert_eq!(
            safe_text("a\u{1b}]52;secret\u{7}\n中"),
            "a\\u{1b}]52;secret\\u{7}\n中"
        );
    }
}
