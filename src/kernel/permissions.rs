use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::PermissionsConfig;
use crate::tools::traits::ToolContext;

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
    MemoryRead {
        scope: MemoryScope,
    },
    MemoryWrite {
        scope: MemoryScope,
    },
    Schedule {
        action: String,
    },
    Notify {
        channel: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PathPattern(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainPattern(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryScope {
    Session,
    User,
    Global,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    permissions: HashSet<Permission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPermissionProfile {
    pub pre_authorized: CapabilitySet,
    pub max_allowed: CapabilitySet,
    pub allow_user_prompts: bool,
    pub prompt_timeout_secs: u64,
}

impl Default for ChannelPermissionProfile {
    fn default() -> Self {
        Self {
            pre_authorized: CapabilitySet::empty(),
            max_allowed: CapabilitySet::empty(),
            allow_user_prompts: true,
            prompt_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptDecision {
    AllowOnce,
    AllowSession,
    Deny,
}

#[async_trait]
pub trait PermissionPrompter: Send + Sync {
    async fn prompt(
        &self,
        tool_name: &str,
        permissions: &[Permission],
        timeout_secs: u64,
    ) -> Option<PromptDecision>;

    async fn prompt_timeout_extension(
        &self,
        _tool_name: &str,
        _timeout: Duration,
        _extension: Duration,
        _timeout_secs: u64,
    ) -> Option<bool> {
        None
    }
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

    pub fn allows_any(&self, required: &[Permission]) -> bool {
        required.iter().any(|permission| self.allows(permission))
    }

    pub fn from_config_with_base(config: &PermissionsConfig, base_dir: &Path) -> Self {
        let mut set = CapabilitySet::empty();

        if let Some(filesystem) = &config.filesystem {
            for path in &filesystem.read_paths {
                set.insert(Permission::FileRead {
                    path: PathPattern(resolve_permission_path(base_dir, path)),
                });
            }
            for path in &filesystem.write_paths {
                set.insert(Permission::FileWrite {
                    path: PathPattern(resolve_permission_path(base_dir, path)),
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

        if let Some(schedule) = &config.schedule
            && !schedule.allowed_actions.is_empty()
        {
            for action in &schedule.allowed_actions {
                set.insert(Permission::Schedule {
                    action: action.clone(),
                });
            }
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

pub fn parse_permission_with_base(value: &str, base_dir: &Path) -> Result<Permission, String> {
    let mut permission = value.trim().parse::<Permission>()?;
    match &mut permission {
        Permission::FileRead { path } | Permission::FileWrite { path } => {
            path.0 = resolve_permission_path(base_dir, &path.0);
        }
        _ => {}
    }
    Ok(permission)
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

pub(crate) fn resolve_permission_path(base_dir: &Path, raw: &str) -> String {
    let expanded = if (raw == "~" || raw.starts_with("~/"))
        && let Some(home) = dirs::home_dir()
    {
        let trimmed = raw.trim_start_matches('~');
        home.join(trimmed.trim_start_matches('/'))
            .to_string_lossy()
            .to_string()
    } else {
        raw.to_string()
    };

    let path = Path::new(&expanded);
    let resolved = if path.is_absolute() {
        Path::new(&expanded).to_path_buf()
    } else {
        base_dir.join(path)
    };

    normalize_path(&resolved).to_string_lossy().to_string()
}

impl std::fmt::Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Permission::FileRead { path } => write!(f, "filesystem:read:{}", path.0),
            Permission::FileWrite { path } => write!(f, "filesystem:write:{}", path.0),
            Permission::NetAccess { domain } => write!(f, "net:{}", domain.0),
            Permission::ShellExec { allowed_commands } => match allowed_commands {
                None => write!(f, "shell:*"),
                Some(commands) => write!(f, "shell:{}", commands.join(",")),
            },
            Permission::MemoryRead { scope } => {
                write!(f, "memory:read:{}", memory_scope_label(*scope))
            }
            Permission::MemoryWrite { scope } => {
                write!(f, "memory:write:{}", memory_scope_label(*scope))
            }
            Permission::Schedule { action } => write!(f, "schedule:{}", action),
            Permission::Notify { channel } => write!(f, "notify:{}", channel),
        }
    }
}

fn normalize_path(path: &Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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
            (
                Permission::MemoryRead { scope: granted },
                Permission::MemoryRead { scope: needed },
            ) => granted.covers(*needed),
            (
                Permission::MemoryWrite { scope: granted },
                Permission::MemoryWrite { scope: needed },
            ) => granted.covers(*needed),
            (Permission::Schedule { action: granted }, Permission::Schedule { action: needed }) => {
                granted == "*" || granted == needed
            }
            (Permission::Notify { channel: granted }, Permission::Notify { channel: needed }) => {
                granted == "*" || granted == needed
            }
            _ => false,
        }
    }

    pub fn is_auto_granted(&self, ctx: &ToolContext) -> bool {
        match self {
            Permission::MemoryRead { scope } | Permission::MemoryWrite { scope } => match scope {
                MemoryScope::Session => ctx.session_id.is_some(),
                MemoryScope::User => ctx.user_id.is_some(),
                MemoryScope::Global => false,
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
        if let Some(scope) = value.strip_prefix("memory:read:") {
            return Ok(Permission::MemoryRead {
                scope: parse_memory_scope(scope)?,
            });
        }
        if let Some(scope) = value.strip_prefix("memory:write:") {
            return Ok(Permission::MemoryWrite {
                scope: parse_memory_scope(scope)?,
            });
        }
        if let Some(action) = value.strip_prefix("schedule:") {
            if action.is_empty() {
                return Err("schedule permission requires an action".to_string());
            }
            return Ok(Permission::Schedule {
                action: action.to_string(),
            });
        }
        if let Some(channel) = value.strip_prefix("notify:") {
            if channel.is_empty() {
                return Err("notify permission requires a channel".to_string());
            }
            return Ok(Permission::Notify {
                channel: channel.to_string(),
            });
        }
        Err(format!("invalid permission '{value}'"))
    }
}

fn parse_memory_scope(value: &str) -> Result<MemoryScope, String> {
    match value {
        "session" => Ok(MemoryScope::Session),
        "user" => Ok(MemoryScope::User),
        "global" => Ok(MemoryScope::Global),
        _ => Err(format!("invalid memory scope '{value}'")),
    }
}

fn memory_scope_label(scope: MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Session => "session",
        MemoryScope::User => "user",
        MemoryScope::Global => "global",
    }
}

impl MemoryScope {
    pub fn covers(self, required: MemoryScope) -> bool {
        matches!(
            (self, required),
            (MemoryScope::Global, _)
                | (MemoryScope::User, MemoryScope::User | MemoryScope::Session)
                | (MemoryScope::Session, MemoryScope::Session)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{CapabilitySet, DomainPattern, MemoryScope, PathPattern, Permission};
    use crate::config::{FilesystemPermissions, PermissionsConfig};
    use crate::tools::traits::ToolContext;
    use std::path::PathBuf;
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

    #[test]
    fn permission_from_str_parses_memory_scopes() {
        let permission = Permission::from_str("memory:read:session").unwrap();
        assert!(matches!(
            permission,
            Permission::MemoryRead {
                scope: MemoryScope::Session
            }
        ));

        let permission = Permission::from_str("memory:write:user").unwrap();
        assert!(matches!(
            permission,
            Permission::MemoryWrite {
                scope: MemoryScope::User
            }
        ));
    }

    #[test]
    fn permission_from_str_parses_schedule() {
        let permission = Permission::from_str("schedule:create").unwrap();
        assert!(matches!(permission, Permission::Schedule { .. }));
    }

    #[test]
    fn permission_from_str_parses_notify() {
        let permission = Permission::from_str("notify:whatsapp").unwrap();
        assert!(matches!(permission, Permission::Notify { .. }));
    }

    #[test]
    fn memory_scope_covers_global() {
        let global = Permission::MemoryRead {
            scope: MemoryScope::Global,
        };
        let needed = Permission::MemoryRead {
            scope: MemoryScope::Session,
        };
        assert!(global.covers(&needed));
    }

    #[test]
    fn memory_scope_user_covers_session() {
        let user = Permission::MemoryWrite {
            scope: MemoryScope::User,
        };
        let needed = Permission::MemoryWrite {
            scope: MemoryScope::Session,
        };
        assert!(user.covers(&needed));
    }

    #[test]
    fn from_config_with_base_resolves_relative_paths() {
        let config = PermissionsConfig {
            filesystem: Some(FilesystemPermissions {
                read_paths: vec!["data/**".to_string()],
                write_paths: vec![],
                jail_root: None,
            }),
            network: None,
            shell: None,
            schedule: None,
            tool_limits: None,
        };
        let base = PathBuf::from("/tmp/picobot");
        let set = CapabilitySet::from_config_with_base(&config, &base);
        assert!(set.allows(&Permission::FileRead {
            path: PathPattern("/tmp/picobot/data/**".to_string())
        }));
    }

    #[test]
    fn permission_cross_type_does_not_cover() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::FileRead {
            path: PathPattern("/tmp/**".to_string()),
        });
        let needed = Permission::FileWrite {
            path: PathPattern("/tmp/file.txt".to_string()),
        };
        assert!(!set.allows(&needed));
    }

    #[test]
    fn path_pattern_single_star_matches_nested() {
        let pattern = PathPattern("/tmp/*".to_string());
        assert!(pattern.matches(std::path::Path::new("/tmp/file.txt")));
        assert!(pattern.matches(std::path::Path::new("/tmp/nested/file.txt")));
    }

    #[test]
    fn path_pattern_exact_match_only() {
        let pattern = PathPattern("/tmp/file.txt".to_string());
        assert!(pattern.matches(std::path::Path::new("/tmp/file.txt")));
        assert!(!pattern.matches(std::path::Path::new("/tmp/file.txt.bak")));
    }

    #[test]
    fn domain_pattern_wildcard_matches_subdomain() {
        let pattern = DomainPattern("*.github.com".to_string());
        assert!(pattern.matches("api.github.com"));
        assert!(!pattern.matches("github.com"));
    }

    #[test]
    fn shell_exec_specific_does_not_cover_other() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::ShellExec {
            allowed_commands: Some(vec!["git".to_string()]),
        });
        let required = Permission::ShellExec {
            allowed_commands: Some(vec!["rm".to_string()]),
        };
        assert!(!set.allows(&required));
    }

    #[test]
    fn capability_set_allows_any_requires_one_match() {
        let mut set = CapabilitySet::empty();
        set.insert(Permission::FileRead {
            path: PathPattern("/tmp/one.txt".to_string()),
        });
        let required = vec![
            Permission::FileRead {
                path: PathPattern("/tmp/one.txt".to_string()),
            },
            Permission::FileRead {
                path: PathPattern("/tmp/two.txt".to_string()),
            },
        ];
        assert!(set.allows_any(&required));
    }

    #[test]
    fn memory_scope_session_does_not_cover_user() {
        let session = Permission::MemoryRead {
            scope: MemoryScope::Session,
        };
        let required = Permission::MemoryRead {
            scope: MemoryScope::User,
        };
        assert!(!session.covers(&required));
    }

    #[test]
    fn permission_display_roundtrip() {
        let permission = Permission::MemoryWrite {
            scope: MemoryScope::User,
        };
        let parsed = Permission::from_str(&permission.to_string()).unwrap();
        assert_eq!(permission, parsed);
    }

    #[test]
    fn notify_permission_display_roundtrip() {
        let permission = Permission::Notify {
            channel: "whatsapp".to_string(),
        };
        let parsed = Permission::from_str(&permission.to_string()).unwrap();
        assert_eq!(permission, parsed);
    }

    #[test]
    fn is_auto_granted_session_memory_requires_session_id() {
        let permission = Permission::MemoryRead {
            scope: MemoryScope::Session,
        };
        let ctx = ToolContext {
            capabilities: std::sync::Arc::new(CapabilitySet::empty()),
            user_id: None,
            session_id: Some("session".to_string()),
            channel_id: None,
            working_dir: PathBuf::from("/tmp"),
            jail_root: None,
            scheduler: None,
            notifications: None,
            notify_tool_used: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            execution_mode: crate::tools::traits::ExecutionMode::User,
            timezone_offset: "+00:00".to_string(),
            timezone_name: "UTC".to_string(),
            max_response_bytes: None,
            max_response_chars: None,
        };
        assert!(permission.is_auto_granted(&ctx));
    }
}
