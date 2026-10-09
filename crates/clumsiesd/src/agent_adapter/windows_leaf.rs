//! Windows leaf operations for the existing durable Adapter journal and compare/capture/publish logic.

use std::ffi::CStr;
use std::io::{Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use super::*;

/// Pins the managed namespace against directory deletion or replacement during a transaction.
pub(super) struct ManagedLeafDirectory {
    /// Existing path validation reused by both platform implementations.
    guard: ManagedPathGuard,
    /// Directory containing the leaf and private journal files.
    parent: PathBuf,
    /// Single-component target name.
    pub(super) target: std::ffi::CString,
    /// Handles deny delete sharing until all operations have finished.
    _pins: Vec<fs::File>,
}

impl ManagedLeafDirectory {
    /// Creates validated parent directories and pins every managed ancestor.
    ///
    /// # Errors
    /// Rejects linked, replaced, or inaccessible namespaces.
    pub(super) fn open(path: &Path) -> Result<Self, DaemonError> {
        let initial = ManagedPathGuard::capture_inferred(path)?;
        initial.create_parent_directories()?;
        let guard = ManagedPathGuard::capture_under(&initial.anchor, path)?;
        let mut pins = Vec::new();
        for directory in &guard.directories {
            let file = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&directory.path)?;
            validate_directory_metadata(&directory.path, &file.metadata()?)?;
            pins.push(file);
        }
        guard.revalidate()?;
        let parent = path
            .parent()
            .ok_or_else(|| adapter_conflict("Adapter leaf has no parent"))?
            .to_path_buf();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| adapter_conflict("Adapter leaf name is not UTF-8"))?;
        let target = std::ffi::CString::new(name)
            .map_err(|_| adapter_conflict("Adapter leaf name contains NUL"))?;
        Ok(Self {
            guard,
            parent,
            target,
            _pins: pins,
        })
    }

    /// Rechecks path guards while namespace handles are pinned.
    ///
    /// # Errors
    /// Rejects a changed directory or leaf.
    pub(super) fn revalidate(&self) -> Result<(), DaemonError> {
        self.guard.revalidate()
    }

    /// File writes are synced before write-through publication; directory handles remain pinned.
    ///
    /// # Errors
    /// Rejects namespace drift.
    pub(super) fn sync(&self) -> Result<(), DaemonError> {
        self.revalidate()
    }

    /// Resolves only one normal filename inside the pinned directory.
    ///
    /// # Errors
    /// Rejects separators, parent traversal, or non-UTF-8 names.
    fn path(&self, name: &CStr) -> Result<PathBuf, DaemonError> {
        let name = name
            .to_str()
            .map_err(|_| adapter_conflict("Adapter filename is not UTF-8"))?;
        let path = Path::new(name);
        if path.components().count() != 1
            || !matches!(path.components().next(), Some(Component::Normal(_)))
        {
            return Err(adapter_conflict("Adapter filename escaped its directory"));
        }
        Ok(self.parent.join(path))
    }
}

/// Reads a bounded, regular leaf without following a reparse point.
///
/// # Errors
/// Rejects linked, oversized, or inaccessible files.
pub(super) fn file_snapshot_at(
    directory: &ManagedLeafDirectory,
    name: &CStr,
) -> Result<FileSnapshot, DaemonError> {
    let path = directory.path(name)?;
    let mut file = match fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(file_snapshot_from_journal(&None, None));
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    validate_managed_leaf(&path)?;
    if !metadata.is_file() || metadata.len() > MAX_ADAPTER_FS_CONTENT_BYTES as u64 {
        return Err(adapter_conflict(
            "Adapter leaf is not a bounded regular file",
        ));
    }
    let mut content = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_ADAPTER_FS_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut content)?;
    if content.len() > MAX_ADAPTER_FS_CONTENT_BYTES {
        return Err(adapter_conflict("Adapter leaf exceeded its content limit"));
    }
    Ok(file_snapshot_from_journal(&Some(content), None))
}

/// Publishes within one directory without replacing a destination created by another writer.
///
/// # Errors
/// Returns collisions, unsupported filesystems, or transport errors.
pub(super) fn rename_noreplace_at(
    directory: &ManagedLeafDirectory,
    source: &CStr,
    destination: &CStr,
) -> std::io::Result<()> {
    let encode = |name| -> std::io::Result<Vec<u16>> {
        let path = directory.path(name).map_err(std::io::Error::other)?;
        Ok(path.as_os_str().encode_wide().chain(Some(0)).collect())
    };
    let source = encode(source)?;
    let destination = encode(destination)?;
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Removes only validated private journal files; absence permits crash recovery.
///
/// # Errors
/// Rejects reparse points or filesystem failures.
pub(super) fn remove_file_at(
    directory: &ManagedLeafDirectory,
    name: &CStr,
) -> Result<(), DaemonError> {
    let path = directory.path(name)?;
    validate_managed_leaf(&path)?;
    match fs::remove_file(path) {
        Ok(()) => directory.sync(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Rebuilds attributable partial staging files and publishes only the journaled bytes.
///
/// # Errors
/// Preserves foreign collisions rather than overwriting them.
pub(super) fn create_staged_file_at(
    directory: &ManagedLeafDirectory,
    name: &CStr,
    stage: &CStr,
    content: &[u8],
    mode: u32,
) -> Result<(), DaemonError> {
    let expected = file_snapshot_from_journal(&Some(content.to_vec()), Some(mode));
    let current = file_snapshot_at(directory, name)?;
    if current.content.is_some() {
        return if snapshots_match(&current, &expected) {
            Ok(())
        } else {
            Err(adapter_conflict("Private Adapter staging file changed"))
        };
    }
    // The random stage name is owned by the persisted journal, including a partial crash write.
    if file_snapshot_at(directory, stage)?.content.is_some() {
        remove_file_at(directory, stage)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.path(stage)?)?;
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    match rename_noreplace_at(directory, stage, name) {
        Ok(()) => directory.sync(),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if snapshots_match(&file_snapshot_at(directory, name)?, &expected) {
                remove_file_at(directory, stage)
            } else {
                Err(adapter_conflict(
                    "Private Adapter staging destination changed",
                ))
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_replace_publication_preserves_foreign_file_and_rebuilds_partial_stage() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode.json");
        let directory = ManagedLeafDirectory::open(&path).unwrap();
        let source = c"private.new";
        let stage = c"private.stage";
        fs::write(directory.path(stage).unwrap(), b"partial").unwrap();
        create_staged_file_at(&directory, source, stage, b"owned", 0o644).unwrap();
        fs::write(&path, b"foreign").unwrap();
        assert!(rename_noreplace_at(&directory, source, &directory.target).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"foreign");
        assert_eq!(
            file_snapshot_at(&directory, source)
                .unwrap()
                .content
                .unwrap(),
            b"owned"
        );
    }
}
