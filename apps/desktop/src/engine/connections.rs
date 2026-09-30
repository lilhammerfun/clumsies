//! Local setup uses the same daemon-owned bindings and host adapters as macOS.
use super::*;
use clumsiesd::{
    DaemonAgentAdapterSettings, DaemonProjectAgentAdapterListRequest,
    DaemonProjectAgentAdapterRemoveRequest, DaemonProjectBinding, DaemonProjectBindingListRequest,
    DaemonProjectBindingRemoveRequest, DaemonProjectBindingReplaceRequest,
    DaemonSetAgentAdapterRequest, ProjectAgentAdapterKind,
};
use std::path::{Path, PathBuf};

pub struct Connections {
    pub bindings: Vec<DaemonProjectBinding>,
}

pub fn connections(project: &str) -> Result<Connections, String> {
    let bindings = client()
        .list_project_bindings(DaemonProjectBindingListRequest {
            project_id: project.into(),
        })
        .map_err(|e| e.to_string())?
        .items;
    Ok(Connections { bindings })
}

pub fn agent_settings() -> Result<DaemonAgentAdapterSettings, String> {
    client()
        .call(DaemonIpcRequest::new(
            "agent_adapter_settings",
            serde_json::json!({}),
        ))
        .map_err(|e| e.to_string())?
        .into_payload()
        .map_err(|e| e.to_string())
}

pub fn bind_workspace(project: &str, path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("Choose an existing work folder.".into());
    }
    let path = path.to_str().ok_or("The folder path is not valid UTF-8.")?;
    client()
        .replace_project_binding(DaemonProjectBindingReplaceRequest {
            workspace_root: path.into(),
            project_id: project.into(),
            expected_revision: None,
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn unbind_workspace(binding: &DaemonProjectBinding) -> Result<(), String> {
    let adapters = client()
        .list_project_agent_adapters(DaemonProjectAgentAdapterListRequest {
            project_id: binding.project_id.clone(),
        })
        .map_err(|e| e.to_string())?;
    for adapter in adapters
        .items
        .into_iter()
        .filter(|a| a.workspace_root == binding.workspace_root)
    {
        client()
            .remove_project_agent_adapter(DaemonProjectAgentAdapterRemoveRequest {
                workspace_root: adapter.workspace_root,
                adapter: adapter.adapter,
                expected_revision: adapter.revision,
            })
            .map_err(|e| e.to_string())?;
    }
    client()
        .remove_project_binding(DaemonProjectBindingRemoveRequest {
            workspace_root: binding.workspace_root.clone(),
            expected_revision: binding.revision,
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn agent_runtime() -> Result<PathBuf, String> {
    let path = if let Some(path) = std::env::var_os("CLUMSIES_AGENT_RUNTIME_BINARY") {
        PathBuf::from(path)
    } else {
        std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(if cfg!(windows) {
                "clumsiesd.exe"
            } else {
                "clumsiesd"
            })
    };
    if !path.is_file() {
        return Err("The bundled Clumsies engine is missing. Reinstall the client.".into());
    }
    std::fs::canonicalize(path).map_err(|e| e.to_string())
}

fn codex_binary() -> Option<String> {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

pub fn isolated_connections() -> bool {
    std::env::var_os("CLUMSIES_DAEMON_ROOT").is_some_and(|root| {
        Path::new(&root)
            .components()
            .any(|part| part.as_os_str() == "ai.clumsies.dev")
    })
}

pub fn configure_agent(adapter: ProjectAgentAdapterKind, enabled: bool) -> Result<(), String> {
    if isolated_connections() {
        return Err(
            "Agent installation is unavailable in this isolated development instance.".into(),
        );
    }
    let runtime = agent_runtime()?
        .to_str()
        .ok_or("Invalid engine path")?
        .to_owned();
    let host = if adapter == ProjectAgentAdapterKind::Codex {
        codex_binary()
    } else {
        None
    };
    let can_install = adapter != ProjectAgentAdapterKind::Codex || host.is_some();
    let request = DaemonSetAgentAdapterRequest {
        adapter,
        enabled,
        runtime_binary_path: runtime,
        host_binary_path: host,
    };
    let result: DaemonAgentAdapterSettings = client()
        .with_timeout(Duration::from_secs(90))
        .call(DaemonIpcRequest::new(
            "set_agent_adapter",
            serde_json::to_value(request).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?
        .into_payload()
        .map_err(|e| e.to_string())?;
    let setting = result
        .items
        .iter()
        .find(|s| s.adapter == adapter)
        .ok_or("Missing integration status")?;
    if enabled && can_install && !setting.installed {
        return Err("The integration was not installed. Retry after opening the AI tool.".into());
    }
    Ok(())
}

pub fn codex_plugin_status() -> Result<clumsiesd::DaemonCodexPluginStatus, String> {
    let runtime_binary_path = agent_runtime()?.to_string_lossy().into_owned();
    client()
        .with_timeout(Duration::from_secs(45))
        .inspect_codex_plugin(clumsiesd::DaemonCodexPluginRequest {
            runtime_binary_path,
            host_binary_path: codex_binary(),
        })
        .map_err(|e| e.to_string())
}

/// Portable stdio configuration for clients without a managed adapter.
#[cfg(test)]
fn mcp_configuration() -> Result<String, String> {
    let mut entry = serde_json::json!({"command": agent_runtime()?, "args":["mcp","serve"]});
    if let Some(root) = std::env::var_os("CLUMSIES_DAEMON_ROOT") {
        entry["env"] = serde_json::json!({"CLUMSIES_DAEMON_ROOT":root.to_string_lossy()});
    }
    serde_json::to_string_pretty(&serde_json::json!({"mcpServers":{"clumsies":entry}}))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clumsiesd::DaemonProjectBindingResolveRequest;

    #[test]
    #[ignore]
    fn local_connection_routes_real_mcp_and_unlinks() {
        assert_eq!(std::env::var("CLUMSIES_MEMORY_UI_TEST").as_deref(), Ok("1"));
        let health = client().health().unwrap();
        assert_eq!(
            reqwest::Url::parse(&health.server_url).unwrap().host_str(),
            Some("127.0.0.1")
        );
        let projects = projects().unwrap();
        let project = projects
            .iter()
            .find(|p| p.name == "Memory UI 验收")
            .unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let agent = DaemonIpcClient::for_agent_runtime(DAEMON_SERVICE, health.agent_runtime);
        let resolve = || {
            agent.resolve_project_binding(DaemonProjectBindingResolveRequest {
                workspace_path: workspace.path().to_string_lossy().into_owned(),
                required_adapter: None,
            })
        };
        assert!(
            resolve()
                .unwrap_err()
                .to_string()
                .contains("project_binding_not_found")
        );
        bind_workspace(&project.project_id, workspace.path()).unwrap();
        let binding = connections(&project.project_id)
            .unwrap()
            .bindings
            .into_iter()
            .find(|b| Path::new(&b.workspace_root) == workspace.path())
            .unwrap();
        let result = std::panic::catch_unwind(|| {
            assert_eq!(resolve().unwrap().project_id, project.project_id);
            if let Some(other) = projects.iter().find(|p| p.project_id != project.project_id) {
                assert!(bind_workspace(&other.project_id, workspace.path()).is_err());
                assert_eq!(resolve().unwrap().project_id, project.project_id);
            }
            let config: serde_json::Value =
                serde_json::from_str(&mcp_configuration().unwrap()).unwrap();
            let entry = &config["mcpServers"]["clumsies"];
            let mut command = std::process::Command::new(entry["command"].as_str().unwrap());
            command
                .args(["mcp", "serve"])
                .current_dir(workspace.path())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if let Some(env) = entry["env"].as_object() {
                for (key, value) in env {
                    command.env(key, value.as_str().unwrap());
                }
            }
            let mut child = command.spawn().unwrap();
            use std::io::Write;
            let mut stdin = child.stdin.take().unwrap();
            for value in [
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
                serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory","arguments":{"op":{"activate":{"query":"中文文件名 欢迎"}}}}}),
                serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memory","arguments":{"op":{"load":{"ids":["01-目录层级/欢迎.md"]}}}}}),
            ] {
                writeln!(stdin, "{value}").unwrap();
            }
            drop(stdin);
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let answers: Vec<serde_json::Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            for id in [2, 3] {
                let answer = answers.iter().find(|a| a["id"] == id).unwrap();
                assert!(answer.get("error").is_none(), "{answer}");
                assert_eq!(answer["result"]["isError"], false);
            }
            let loaded = answers.iter().find(|a| a["id"] == 3).unwrap();
            assert_eq!(
                loaded["result"]["structuredContent"]["resources"][0]["path"],
                "01-目录层级/欢迎.md"
            );
        });
        unbind_workspace(&binding).unwrap();
        assert!(
            resolve()
                .unwrap_err()
                .to_string()
                .contains("project_binding_not_found")
        );
        result.unwrap();
    }
}
