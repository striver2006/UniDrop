use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum PathSecurityError {
    AbsolutePathForbidden,
    ParentTraversalForbidden,
    ReservedWindowsName,
    InvalidCharacter,
}

impl std::fmt::Display for PathSecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AbsolutePathForbidden => write!(f, "Absolute paths are strictly forbidden"),
            Self::ParentTraversalForbidden => write!(f, "Parent directory traversal (..) is forbidden"),
            Self::ReservedWindowsName => write!(f, "Reserved Windows device name detected"),
            Self::InvalidCharacter => write!(f, "Path contains invalid or dangerous characters"),
        }
    }
}

impl std::error::Error for PathSecurityError {}

pub struct PathGuard;

impl PathGuard {
    /// Sanitizes an untrusted relative path and resolves it safely within base_cache_dir.
    pub fn sanitize_and_resolve(base_cache_dir: &Path, untrusted_path: &str) -> Result<PathBuf, PathSecurityError> {
        let path = Path::new(untrusted_path);

        // 1. Disallow absolute paths
        if path.is_absolute() {
            return Err(PathSecurityError::AbsolutePathForbidden);
        }

        let mut sanitized_path = PathBuf::new();

        for component in path.components() {
            match component {
                Component::Normal(segment) => {
                    let seg_str = segment.to_str().ok_or(PathSecurityError::InvalidCharacter)?;

                    // 2. Reject Windows reserved names
                    if Self::is_windows_reserved_name(seg_str) {
                        return Err(PathSecurityError::ReservedWindowsName);
                    }

                    // 3. Trim dangerous trailing/leading dots and spaces
                    let trimmed = seg_str.trim_matches(|c| c == ' ' || c == '.');
                    if trimmed.is_empty() {
                        return Err(PathSecurityError::InvalidCharacter);
                    }

                    sanitized_path.push(trimmed);
                }
                // 4. Reject parent directory escape
                Component::ParentDir => return Err(PathSecurityError::ParentTraversalForbidden),
                Component::RootDir | Component::Prefix(_) => return Err(PathSecurityError::AbsolutePathForbidden),
                Component::CurDir => continue, // ignore "."
            }
        }

        if sanitized_path.as_os_str().is_empty() {
            return Err(PathSecurityError::InvalidCharacter);
        }

        Ok(base_cache_dir.join(sanitized_path))
    }

    fn is_windows_reserved_name(name: &str) -> bool {
        let upper = name.to_ascii_uppercase();
        let stem = upper.split('.').next().unwrap_or("");
        matches!(
            stem,
            "CON" | "PRN" | "AUX" | "NUL"
                | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
                | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_paths() {
        let base = Path::new("/tmp/unidrop_cache");
        let p = PathGuard::sanitize_and_resolve(base, "docs/report.pdf").unwrap();
        assert_eq!(p, base.join("docs").join("report.pdf"));

        let p2 = PathGuard::sanitize_and_resolve(base, "nested/folder/file.txt").unwrap();
        assert_eq!(p2, base.join("nested").join("folder").join("file.txt"));
    }

    #[test]
    fn test_reject_parent_traversal() {
        let base = Path::new("/tmp/unidrop_cache");
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "../../etc/passwd"),
            Err(PathSecurityError::ParentTraversalForbidden)
        );
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "docs/../../evil.sh"),
            Err(PathSecurityError::ParentTraversalForbidden)
        );
    }

    #[test]
    fn test_reject_absolute_paths() {
        let base = Path::new("/tmp/unidrop_cache");
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "/etc/shadow"),
            Err(PathSecurityError::AbsolutePathForbidden)
        );
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "C:\\Windows\\System32"),
            Err(PathSecurityError::AbsolutePathForbidden)
        );
    }

    #[test]
    fn test_reject_windows_reserved() {
        let base = Path::new("/tmp/unidrop_cache");
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "CON.txt"),
            Err(PathSecurityError::ReservedWindowsName)
        );
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "docs/NUL"),
            Err(PathSecurityError::ReservedWindowsName)
        );
        assert_eq!(
            PathGuard::sanitize_and_resolve(base, "folder/com1.log"),
            Err(PathSecurityError::ReservedWindowsName)
        );
    }
}
