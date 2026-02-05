use std::path::Path;

use crate::config::{ChannelConfig, ChannelsConfig};
use crate::kernel::permissions::{
    parse_permission_with_base, CapabilitySet, ChannelPermissionProfile, MemoryScope, Permission,
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
        if let Ok(permission) = parse_permission_with_base(entry, base_dir) {
            set.insert(permission);
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

pub fn channel_config<'a>(
    config: &'a ChannelsConfig,
    channel_id: &str,
) -> Option<&'a ChannelConfig> {
    config.profiles.get(channel_id)
}
