use std::path::{Path, PathBuf};

use crate::tools::traits::ToolError;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPath {
    pub canonical: PathBuf,
}

pub(crate) fn resolve_path(
    working_dir: &Path,
    jail_root: Option<&Path>,
    raw: &str,
) -> Result<ResolvedPath, ToolError> {
    let expanded = if raw.starts_with('~') {
        if raw == "~" || raw.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                let trimmed = raw.trim_start_matches('~');
                home.join(trimmed.trim_start_matches('/'))
            } else {
                return Err(ToolError::new("home directory not found".to_string()));
            }
        } else {
            PathBuf::from(raw)
        }
    } else {
        PathBuf::from(raw)
    };

    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        working_dir.join(expanded)
    };

    let normalized = normalize_path(&resolved);
    let canonical = canonicalize_with_fallback(&normalized)?;
    if let Some(jail_root) = jail_root {
        let jail_root = jail_root
            .canonicalize()
            .map_err(|err| ToolError::new(format!("invalid jail_root: {err}")))?;
        if !canonical.starts_with(&jail_root) {
            return Err(ToolError::new(format!(
                "path escapes jail_root: {}",
                canonical.display()
            )));
        }
    }

    Ok(ResolvedPath { canonical })
}

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn canonicalize_with_fallback(path: &Path) -> Result<PathBuf, ToolError> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|err| ToolError::new(err.to_string()));
    }

    let mut current = path;
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    while !current.exists() {
        if let Some(name) = current.file_name() {
            remainder.push(name.to_os_string());
        } else {
            break;
        }
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }

    if current.exists() {
        let mut canonical = current
            .canonicalize()
            .map_err(|err| ToolError::new(err.to_string()))?;
        for part in remainder.iter().rev() {
            canonical.push(part);
        }
        return Ok(canonical);
    }

    Ok(path.to_path_buf())
}
