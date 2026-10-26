//! Helpers for handling untrusted client input safely.

use std::path::{Component, Path, PathBuf};

use crate::tool::ToolError;

/// Resolves a client-provided relative path inside `root`, refusing anything
/// that could escape it.
///
/// Use it whenever a resource variable (typically `{+path}`) or a tool
/// argument names a file:
///
/// ```no_run
/// # use flymcprust_core::{ToolError, security::confine_path};
/// /// A file of the workspace.
/// // #[resource(template = "file:///{+path}")]
/// fn file(path: String) -> Result<String, ToolError> {
///     let full = confine_path("/srv/workspace", &path)?;
///     std::fs::read_to_string(full).map_err(|_| ToolError::not_found())
/// }
/// ```
///
/// Rejected with [`ToolError::not_found`] (which reveals nothing about the
/// file system):
///
/// - absolute paths, drive or UNC prefixes, and `..` components;
/// - empty paths and paths containing NUL or other control characters;
/// - paths that leave `root` after following symbolic links (checked on the
///   canonical path, so the target must exist).
///
/// `root` itself is trusted and must exist.
///
/// # Errors
///
/// Returns [`ToolError::not_found`] when the path is invalid, escapes
/// `root`, or does not exist.
pub fn confine_path(root: impl AsRef<Path>, relative: &str) -> Result<PathBuf, ToolError> {
    if relative.is_empty() || relative.chars().any(char::is_control) {
        return Err(ToolError::not_found());
    }
    // Both separators count on every platform, so a Windows-style `..\`
    // cannot slip through on Unix and vice versa.
    let normalized = relative.replace('\\', "/");
    let mut clean = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::not_found());
            }
        }
    }
    if clean.as_os_str().is_empty() || (cfg!(windows) && normalized.contains(':')) {
        // `C:` or alternate data streams (`file:stream`) on Windows.
        return Err(ToolError::not_found());
    }
    let root = root
        .as_ref()
        .canonicalize()
        .map_err(|_| ToolError::not_found())?;
    let resolved = root
        .join(&clean)
        .canonicalize()
        .map_err(|_| ToolError::not_found())?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(ToolError::not_found())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flymcprust-confine-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("a.txt"), "a").unwrap();
        dir
    }

    #[test]
    fn accepts_paths_inside_the_root() {
        let root = workspace();
        let p = confine_path(&root, "sub/a.txt").unwrap();
        assert!(p.ends_with("a.txt"));
        assert!(confine_path(&root, "./sub/./a.txt").is_ok());
        assert!(confine_path(&root, "sub\\a.txt").is_ok());
    }

    #[test]
    fn rejects_escapes() {
        let root = workspace();
        for bad in [
            "",
            "../outside",
            "sub/../../outside",
            "..\\outside",
            "/etc/passwd",
            "\\\\server\\share",
            "C:\\Windows\\win.ini",
            "sub/a.txt\0",
            "sub/\na.txt",
            "missing.txt",
        ] {
            assert!(
                confine_path(&root, bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_leaving_the_root() {
        let root = workspace();
        let link = root.join("escape");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("/", &link).unwrap();
        assert!(confine_path(&root, "escape/etc").is_err());
    }
}
