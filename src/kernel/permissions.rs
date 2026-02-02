use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    FileRead {
        path: PathPattern,
    },
    FileWrite {
        path: PathPattern,
    },
    NetAccess {
        domain: DomainPattern,
    },
    ShellExec {
        allowed_commands: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainPattern(pub String);

#[derive(Debug, Default, Clone)]
pub struct CapabilitySet {
    permissions: HashSet<Permission>,
}

impl CapabilitySet {
    pub fn empty() -> Self {
        Self {
            permissions: HashSet::new(),
        }
    }

    pub fn insert(&mut self, permission: Permission) {
        self.permissions.insert(permission);
    }

    pub fn allows(&self, required: &Permission) -> bool {
        self.permissions
            .iter()
            .any(|permission| permission.covers(required))
    }

    pub fn allows_all(&self, required: &[Permission]) -> bool {
        required.iter().all(|permission| self.allows(permission))
    }
}

impl PathPattern {
    pub fn matches(&self, path: &Path) -> bool {
        let value = path.to_string_lossy();
        let pattern_value = expand_tilde(&self.0);
        glob::Pattern::new(&pattern_value)
            .map(|pattern| pattern.matches(&value))
            .unwrap_or(false)
    }
}

impl DomainPattern {
    pub fn matches(&self, domain: &str) -> bool {
        glob::Pattern::new(&self.0)
            .map(|pattern| pattern.matches(domain))
            .unwrap_or(false)
    }
}

fn expand_tilde(value: &str) -> String {
    if (value == "~" || value.starts_with("~/")) && let Some(home) = dirs::home_dir() {
        let trimmed = value.trim_start_matches('~');
        return home
            .join(trimmed.trim_start_matches('/'))
            .to_string_lossy()
            .to_string();
    }
    value.to_string()
}

impl Permission {
    pub fn covers(&self, required: &Permission) -> bool {
        match (self, required) {
            (Permission::FileRead { path: granted }, Permission::FileRead { path: needed }) => {
                granted.matches(Path::new(&needed.0))
            }
            (Permission::FileWrite { path: granted }, Permission::FileWrite { path: needed }) => {
                granted.matches(Path::new(&needed.0))
            }
            (
                Permission::NetAccess { domain: granted },
                Permission::NetAccess { domain: needed },
            ) => granted.matches(&needed.0),
            (
                Permission::ShellExec {
                    allowed_commands: granted,
                },
                Permission::ShellExec {
                    allowed_commands: needed,
                },
            ) => match (granted, needed) {
                (None, _) => true,
                (Some(granted), Some(needed)) => needed.iter().all(|cmd| granted.contains(cmd)),
                (Some(_), None) => false,
            },
            _ => false,
        }
    }
}
