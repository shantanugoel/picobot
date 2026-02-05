use crate::channels::permissions::ChannelPermissionProfile;
use crate::config::ChannelConfig;
use crate::kernel::permissions::{Permission, PermissionTier};

pub fn profile_from_config(
    config: Option<&ChannelConfig>,
    default_tier: PermissionTier,
) -> Result<ChannelPermissionProfile, String> {
    let config = config.cloned().unwrap_or_default();
    let pre_authorized = parse_permissions(&config.pre_authorized)?;
    let max_allowed = parse_permissions(&config.max_allowed)?;
    Ok(ChannelPermissionProfile {
        tier: default_tier,
        pre_authorized,
        max_allowed,
        allow_user_prompts: config.allow_user_prompts.unwrap_or(true),
        prompt_timeout_secs: config.prompt_timeout_secs.unwrap_or(120),
    })
}

fn parse_permissions(values: &[String]) -> Result<Vec<Permission>, String> {
    values
        .iter()
        .map(|value| value.parse::<Permission>())
        .collect()
}
