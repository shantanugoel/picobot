use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("Permission denied for {permission:?}")]
    Denied { permission: Permission },
}

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
        self.permissions.contains(required)
    }

    pub fn allows_all(&self, required: &[Permission]) -> bool {
        required.iter().all(|permission| self.allows(permission))
    }
}
