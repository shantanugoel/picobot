use std::path::Path;

use crate::config::ChannelsConfig;
use crate::kernel::permissions::{
    CapabilitySet, ChannelPermissionProfile, MemoryScope, Permission, parse_permission_with_base,
};

pub fn channel_profile(
    config: &ChannelsConfig,
    channel_id: &str,
    base_dir: &Path,
) -> ChannelPermissionProfile {
    let mut profile = ChannelPermissionProfile::default();
    let Some(channel) = config.profiles.get(channel_id) else {
        return profile;
    };
    if channel.pre_authorized.is_none() && channel.max_allowed.is_none() {
        profile.pre_authorized = default_pre_authorized();
        profile.max_allowed = profile.pre_authorized.clone();
    } else {
        profile.pre_authorized = parse_permissions(channel.pre_authorized.as_ref(), base_dir);
        profile.max_allowed = parse_permissions(channel.max_allowed.as_ref(), base_dir);
        if profile.max_allowed.permissions().next().is_none() {
            profile.max_allowed = profile.pre_authorized.clone();
        }
    }
    profile.allow_user_prompts = channel.allow_user_prompts();
    profile.prompt_timeout_secs = channel.prompt_timeout_secs();
    profile
}

fn parse_permissions(entries: Option<&Vec<String>>, base_dir: &Path) -> CapabilitySet {
    let mut set = CapabilitySet::empty();
    let Some(entries) = entries else {
        return set;
    };
    for entry in entries {
        match parse_permission_with_base(entry, base_dir) {
            Ok(permission) => {
                set.insert(permission);
            }
            Err(err) => {
                tracing::warn!(permission = %entry, error = %err, "invalid channel permission");
            }
        }
    }
    set
}

fn default_pre_authorized() -> CapabilitySet {
    let mut set = CapabilitySet::empty();
    set.insert(Permission::MemoryRead {
        scope: MemoryScope::Session,
    });
    set.insert(Permission::MemoryWrite {
        scope: MemoryScope::Session,
    });
    set
}

#[cfg(test)]
mod tests {
    use super::channel_profile;
    use crate::config::{ChannelConfig, ChannelsConfig};
    use crate::kernel::permissions::{MemoryScope, Permission};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn channel_profile_default_for_missing_channel() {
        let config = ChannelsConfig::default();
        let profile = channel_profile(&config, "unknown", PathBuf::from("/tmp").as_path());
        assert!(profile.pre_authorized.permissions().next().is_none());
    }

    #[test]
    fn channel_profile_defaults_to_session_memory_when_permissions_missing() {
        let mut profiles = HashMap::new();
        profiles.insert("repl".to_string(), ChannelConfig::default());
        let config = ChannelsConfig { profiles };
        let profile = channel_profile(&config, "repl", PathBuf::from("/tmp").as_path());
        let required = Permission::MemoryRead {
            scope: MemoryScope::Session,
        };
        assert!(profile.pre_authorized.allows(&required));
        assert!(profile.max_allowed.allows(&required));
    }

    #[test]
    fn channel_profile_parses_configured_permissions() {
        let mut channel = ChannelConfig::default();
        channel.pre_authorized = Some(vec!["filesystem:read:/tmp/**".to_string()]);
        channel.max_allowed = Some(vec!["filesystem:read:/tmp/**".to_string()]);
        let mut profiles = HashMap::new();
        profiles.insert("api".to_string(), channel);
        let config = ChannelsConfig { profiles };

        let profile = channel_profile(&config, "api", PathBuf::from("/tmp").as_path());
        let required = Permission::FileRead {
            path: crate::kernel::permissions::PathPattern("/tmp/**".to_string()),
        };
        assert!(profile.pre_authorized.allows(&required));
    }

    #[test]
    fn channel_profile_max_allowed_inherits_pre_authorized() {
        let mut channel = ChannelConfig::default();
        channel.pre_authorized = Some(vec!["filesystem:read:/tmp/**".to_string()]);
        channel.max_allowed = Some(vec![]);
        let mut profiles = HashMap::new();
        profiles.insert("api".to_string(), channel);
        let config = ChannelsConfig { profiles };

        let profile = channel_profile(&config, "api", PathBuf::from("/tmp").as_path());
        let required = Permission::FileRead {
            path: crate::kernel::permissions::PathPattern("/tmp/**".to_string()),
        };
        assert!(profile.max_allowed.allows(&required));
    }
}
