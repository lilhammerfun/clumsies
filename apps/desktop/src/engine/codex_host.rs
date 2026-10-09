//! Locate the Codex CLI used to manage its desktop plugins.
use std::ffi::OsStr;
use std::path::PathBuf;

pub(super) fn codex_binary() -> Option<String> {
    #[cfg(windows)]
    let app_binary = installed_codex_app_cli().ok().flatten();
    #[cfg(not(windows))]
    let app_binary = None;
    codex_binary_from(app_binary, std::env::var_os("PATH").as_deref())
}

fn codex_binary_from(app_binary: Option<PathBuf>, search_path: Option<&OsStr>) -> Option<String> {
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    let path_binaries = search_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|path| path.join(name));
    app_binary
        .into_iter()
        .chain(path_binaries)
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn installed_codex_app_cli() -> windows::core::Result<Option<PathBuf>> {
    use windows::Management::Deployment::PackageManager;
    use windows::core::HSTRING;

    // An empty SID asks for this user's registered packages, including installs
    // on another drive. The desktop app does not add its CLI to the user's PATH.
    let packages = PackageManager::new()?.FindPackagesByUserSecurityId(&HSTRING::new())?;
    for package in packages {
        if package.Id()?.Name()? != "OpenAI.Codex" {
            continue;
        }
        let cli = PathBuf::from(package.InstalledLocation()?.Path()?.to_string_lossy())
            .join("app/resources/codex.exe");
        if cli.is_file() {
            return Ok(Some(cli));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_package_discovery_works_without_a_ui_thread() {
        std::thread::spawn(installed_codex_app_cli)
            .join()
            .unwrap()
            .unwrap();
    }

    #[test]
    fn installed_app_is_found_without_a_path_entry() {
        let root = tempfile::tempdir().unwrap();
        let cli = root.path().join("应用 Codex/app/resources/codex.exe");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, b"app CLI").unwrap();
        assert_eq!(
            codex_binary_from(Some(cli.clone()), None),
            Some(cli.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn installed_app_takes_precedence_over_a_separate_path_cli() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app-codex.exe");
        std::fs::write(&app, b"app CLI").unwrap();
        let cli = root
            .path()
            .join(if cfg!(windows) { "codex.exe" } else { "codex" });
        std::fs::write(cli, b"other CLI").unwrap();
        let paths = std::env::join_paths([root.path()]).unwrap();
        assert_eq!(
            codex_binary_from(Some(app.clone()), Some(&paths)),
            Some(app.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn missing_app_falls_back_to_a_path_cli() {
        let root = tempfile::tempdir().unwrap();
        let cli = root
            .path()
            .join(if cfg!(windows) { "codex.exe" } else { "codex" });
        std::fs::write(&cli, b"CLI").unwrap();
        let paths = std::env::join_paths([root.path()]).unwrap();
        assert_eq!(
            codex_binary_from(Some(root.path().join("missing")), Some(&paths)),
            Some(cli.to_string_lossy().into_owned())
        );
        assert_eq!(codex_binary_from(None, None), None);
    }
}
