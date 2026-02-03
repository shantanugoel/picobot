use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::PermissionsConfig;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionTier {
    UserGrantable,
    AdminOnly,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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

    pub fn from_config(config: &PermissionsConfig) -> Self {
        let mut set = CapabilitySet::empty();

        if let Some(filesystem) = &config.filesystem {
            for path in &filesystem.read_paths {
                set.insert(Permission::FileRead {
                    path: PathPattern(path.clone()),
                });
            }
            for path in &filesystem.write_paths {
                set.insert(Permission::FileWrite {
                    path: PathPattern(path.clone()),
                });
            }
        }

        if let Some(network) = &config.network {
            for domain in &network.allowed_domains {
                set.insert(Permission::NetAccess {
                    domain: DomainPattern(domain.clone()),
                });
            }
        }

        if let Some(shell) = &config.shell
            && !shell.allowed_commands.is_empty()
        {
            set.insert(Permission::ShellExec {
                allowed_commands: Some(shell.allowed_commands.clone()),
            });
        }

        set
    }

    pub fn from_permissions(permissions: &[Permission]) -> Self {
        let mut set = CapabilitySet::empty();
        for permission in permissions {
            set.insert(permission.clone());
        }
        set
    }

    pub fn permissions(&self) -> impl Iterator<Item = &Permission> {
        self.permissions.iter()
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
    if (value == "~" || value.starts_with("~/"))
        && let Some(home) = dirs::home_dir()
    {
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

impl std::str::FromStr for Permission {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(path) = value.strip_prefix("filesystem:read:") {
            return Ok(Permission::FileRead {
                path: PathPattern(path.to_string()),
            });
        }
        if let Some(path) = value.strip_prefix("filesystem:write:") {
            return Ok(Permission::FileWrite {
                path: PathPattern(path.to_string()),
            });
        }
        if let Some(domain) = value.strip_prefix("net:") {
            return Ok(Permission::NetAccess {
                domain: DomainPattern(domain.to_string()),
            });
        }
        if value == "shell:*" {
            return Ok(Permission::ShellExec {
                allowed_commands: None,
            });
        }
        if let Some(list) = value.strip_prefix("shell:") {
            let commands = list
                .split(',')
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>();
            if commands.is_empty() {
                return Err("shell permissions require at least one command or '*'".to_string());
            }
            return Ok(Permission::ShellExec {
                allowed_commands: Some(commands),
            });
        }
        Err(format!("invalid permission '{value}'"))
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilitySet, DomainPattern, PathPattern, Permission};
    use std::str::FromStr;

    #[test]
    fn capability_set_allows_globbed_paths() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::FileRead {
            path: PathPattern("/tmp/**".to_string()),
        });

        let required = Permission::FileRead {
            path: PathPattern("/tmp/example.txt".to_string()),
        };

        assert!(set.allows(&required));
    }

    #[test]
    fn domain_pattern_matches_host() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::NetAccess {
            domain: DomainPattern("api.github.com".to_string()),
        });

        let required = Permission::NetAccess {
            domain: DomainPattern("api.github.com".to_string()),
        };

        assert!(set.allows(&required));
    }

    #[test]
    fn shell_exec_none_covers_all() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::ShellExec {
            allowed_commands: None,
        });

        let required = Permission::ShellExec {
            allowed_commands: Some(vec!["git".to_string()]),
        };

        assert!(set.allows(&required));
    }

    #[test]
    fn permission_from_str_parses_filesystem() {
        let permission = Permission::from_str("filesystem:read:/tmp/**").unwrap();
        assert!(matches!(permission, Permission::FileRead { .. }));

        let permission = Permission::from_str("filesystem:write:/tmp/**").unwrap();
        assert!(matches!(permission, Permission::FileWrite { .. }));
    }

    #[test]
    fn permission_from_str_parses_network() {
        let permission = Permission::from_str("net:api.github.com").unwrap();
        assert!(matches!(permission, Permission::NetAccess { .. }));
    }

    #[test]
    fn permission_from_str_parses_shell() {
        let permission = Permission::from_str("shell:git,rg").unwrap();
        assert!(matches!(permission, Permission::ShellExec { .. }));

        let permission = Permission::from_str("shell:*").unwrap();
        assert!(matches!(
            permission,
            Permission::ShellExec {
                allowed_commands: None
            }
        ));
    }
}
