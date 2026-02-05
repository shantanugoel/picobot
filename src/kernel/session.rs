use crate::kernel::permissions::{CapabilitySet, Permission};

#[derive(Debug, Default, Clone)]
pub struct SessionGrants {
    grants: CapabilitySet,
}

impl SessionGrants {
    pub fn new() -> Self {
        Self {
            grants: CapabilitySet::empty(),
        }
    }

    pub fn allows_all(&self, required: &[Permission]) -> bool {
        self.grants.allows_all(required)
    }

    pub fn grant(&mut self, permissions: &[Permission]) {
        for permission in permissions {
            self.grants.insert(permission.clone());
        }
    }

    pub fn as_capabilities(&self) -> &CapabilitySet {
        &self.grants
    }
}
