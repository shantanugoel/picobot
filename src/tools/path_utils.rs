use std::path::{Path, PathBuf};

use crate::tools::traits::ToolError;

pub(crate) fn resolve_path(
    working_dir: &Path,
    jail_root: Option<&Path>,
    raw: &str,
) -> Result<PathBuf, ToolError> {
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

    let resolved = normalize_path(&resolved);
    if let Some(jail_root) = jail_root {
        let jail_root = jail_root
            .canonicalize()
            .map_err(|err| ToolError::new(format!("invalid jail_root: {err}")))?;
        let candidate = if resolved.exists() {
            resolved
                .canonicalize()
                .map_err(|err| ToolError::new(err.to_string()))?
        } else if let Some(parent) = resolved.parent() {
            let parent = parent
                .canonicalize()
                .map_err(|err| ToolError::new(err.to_string()))?;
            match resolved.file_name() {
                Some(name) => parent.join(name),
                None => parent,
            }
        } else {
            resolved.clone()
        };
        if !candidate.starts_with(&jail_root) {
            return Err(ToolError::new(format!(
                "path escapes jail_root: {}",
                candidate.display()
            )));
        }
    }

    Ok(resolved)
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
