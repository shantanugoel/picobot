use crate::kernel::permissions::{CapabilitySet, Permission, PermissionTier};

#[derive(Debug, Clone)]
pub struct ChannelPermissionProfile {
    pub tier: PermissionTier,
    pub pre_authorized: Vec<Permission>,
    pub max_allowed: Vec<Permission>,
    pub allow_user_prompts: bool,
    pub prompt_timeout_secs: u32,
}

impl ChannelPermissionProfile {
    pub fn grants(&self) -> CapabilitySet {
        CapabilitySet::from_permissions(&self.pre_authorized)
    }

    pub fn max_capabilities(&self) -> CapabilitySet {
        CapabilitySet::from_permissions(&self.max_allowed)
    }
}
