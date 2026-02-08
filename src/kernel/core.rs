use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::Instant;

use crate::kernel::permissions::{CapabilitySet, ChannelPermissionProfile, PermissionPrompter};
use crate::scheduler::service::SchedulerService;
use crate::tools::registry::ToolRegistry;
use crate::tools::traits::{
    ExecutionMode,
    PreExecutionDecision,
    ToolContext,
    ToolError,
    ToolExecutor,
    ToolOutput,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftTimeoutPolicy {
    Prompt,
    AutoExtend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeoutExtensionDecision {
    Extended,
    Declined,
}

pub fn soft_timeout_duration(hard_timeout: Duration, ratio: f64) -> Duration {
    if hard_timeout.is_zero() || !ratio.is_finite() || ratio <= 0.0 {
        return Duration::ZERO;
    }
    let ratio = if ratio >= 1.0 { 1.0 } else { ratio };
    let secs = hard_timeout.as_secs_f64() * ratio;
    if secs <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(secs)
    }
}

#[cfg(test)]
mod timeout_extension_tests {
    use super::{SoftTimeoutPolicy, TimeoutExtensionDecision};
    use super::Kernel;
    use crate::kernel::permissions::{CapabilitySet, PermissionPrompter};
    use crate::tools::registry::ToolRegistry;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    struct AllowPrompter;

    #[async_trait]
    impl PermissionPrompter for AllowPrompter {
        async fn prompt(
            &self,
            _tool_name: &str,
            _permissions: &[crate::kernel::permissions::Permission],
            _timeout_secs: u64,
        ) -> Option<crate::kernel::permissions::PromptDecision> {
            None
        }

        async fn prompt_timeout_extension(
            &self,
            _tool_name: &str,
            _timeout: std::time::Duration,
            _extension: std::time::Duration,
            _timeout_secs: u64,
        ) -> Option<bool> {
            Some(true)
        }
    }

    #[tokio::test]
    async fn auto_extend_policy_skips_prompt() {
        let registry = Arc::new(ToolRegistry::new());
        let kernel = Kernel::new(registry)
            .with_capabilities(CapabilitySet::empty())
            .with_soft_timeouts(0.5, SoftTimeoutPolicy::AutoExtend, None);
        let decision = kernel
            .maybe_extend_timeout(
                "dummy",
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(5),
                kernel.context(),
            )
            .await;
        assert!(matches!(decision, TimeoutExtensionDecision::Extended));
    }

    #[tokio::test]
    async fn prompt_policy_allows_extension() {
        let registry = Arc::new(ToolRegistry::new());
        let kernel = Kernel::new(registry)
            .with_capabilities(CapabilitySet::empty())
            .with_prompter(Some(Arc::new(AllowPrompter)));
        let decision = kernel
            .maybe_extend_timeout(
                "dummy",
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(5),
                kernel.context(),
            )
            .await;
        assert!(matches!(decision, TimeoutExtensionDecision::Extended));
    }
}

#[cfg(test)]
mod soft_timeout_tests {
    use super::soft_timeout_duration;

    #[test]
    fn soft_timeout_zero_when_ratio_zero() {
        let hard = std::time::Duration::from_secs(60);
        assert!(soft_timeout_duration(hard, 0.0).is_zero());
    }

    #[test]
    fn soft_timeout_clamps_ratio_above_one() {
        let hard = std::time::Duration::from_secs(10);
        let soft = soft_timeout_duration(hard, 2.0);
        assert_eq!(soft, hard);
    }
}

#[derive(Debug, Clone, Copy)]
enum DecisionSource {
    Capabilities,
    ExtraGrants,
    PreAuthorized,
    SessionGrants,
    AutoGranted,
}

#[derive(Clone)]
pub struct Kernel {
    tool_registry: Arc<ToolRegistry>,
    context: ToolContext,
    prompt_profile: ChannelPermissionProfile,
    prompter: Option<Arc<dyn PermissionPrompter>>,
    session_grants: Arc<std::sync::RwLock<CapabilitySet>>,
    default_timeout: Duration,
    tool_timeouts: std::collections::HashMap<String, Duration>,
    soft_timeout_ratio: f64,
    soft_timeout_policy: SoftTimeoutPolicy,
    soft_timeout_extension: Option<Duration>,
}

impl Kernel {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self {
            tool_registry,
            context: ToolContext {
                capabilities: Arc::new(CapabilitySet::empty()),
                user_id: None,
                session_id: None,
                channel_id: None,
                working_dir: std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from(".")),
                jail_root: None,
                scheduler: None,
                notifications: None,
                notify_tool_used: Arc::new(AtomicBool::new(false)),
                execution_mode: ExecutionMode::User,
                timezone_offset: "+00:00".to_string(),
                timezone_name: "UTC".to_string(),
                max_response_bytes: None,
                max_response_chars: None,
            },
            prompt_profile: ChannelPermissionProfile::default(),
            prompter: None,
            session_grants: Arc::new(std::sync::RwLock::new(CapabilitySet::empty())),
            default_timeout: Duration::from_secs(60),
            tool_timeouts: std::collections::HashMap::new(),
            soft_timeout_ratio: 0.0,
            soft_timeout_policy: SoftTimeoutPolicy::Prompt,
            soft_timeout_extension: None,
        }
    }

    pub fn with_capabilities(mut self, capabilities: CapabilitySet) -> Self {
        self.context.capabilities = Arc::new(capabilities);
        self
    }

    pub fn with_prompt_profile(mut self, profile: ChannelPermissionProfile) -> Self {
        self.prompt_profile = profile;
        self
    }

    pub fn prompt_profile(&self) -> &ChannelPermissionProfile {
        &self.prompt_profile
    }

    pub fn with_prompter(mut self, prompter: Option<Arc<dyn PermissionPrompter>>) -> Self {
        self.prompter = prompter;
        self
    }

    pub fn with_working_dir(mut self, working_dir: std::path::PathBuf) -> Self {
        self.context.working_dir = working_dir;
        self
    }

    pub fn with_jail_root(mut self, jail_root: Option<std::path::PathBuf>) -> Self {
        self.context.jail_root = jail_root;
        self
    }

    pub fn with_scheduler(mut self, scheduler: Option<Arc<SchedulerService>>) -> Self {
        self.context.scheduler = scheduler;
        self
    }

    pub fn with_notifications(
        mut self,
        notifications: Option<Arc<crate::notifications::service::NotificationService>>,
    ) -> Self {
        self.context.notifications = notifications;
        self
    }

    pub fn with_channel_id(mut self, channel_id: Option<String>) -> Self {
        self.context.channel_id = channel_id;
        self
    }

    pub fn with_timezone(mut self, offset: String, name: String) -> Self {
        self.context.timezone_offset = offset;
        self.context.timezone_name = name;
        self
    }

    pub fn with_max_response_bytes(mut self, max_response_bytes: Option<u64>) -> Self {
        self.context.max_response_bytes = max_response_bytes;
        self
    }

    pub fn with_max_response_chars(mut self, max_response_chars: Option<usize>) -> Self {
        self.context.max_response_chars = max_response_chars;
        self
    }

    pub fn with_execution_mode(mut self, mode: ExecutionMode) -> Self {
        self.context.execution_mode = mode;
        self
    }

    pub fn with_tool_timeouts(
        mut self,
        default_timeout: Duration,
        tool_timeouts: std::collections::HashMap<String, Duration>,
    ) -> Self {
        self.default_timeout = default_timeout;
        self.tool_timeouts = tool_timeouts;
        self
    }

    pub fn with_soft_timeouts(
        mut self,
        ratio: f64,
        policy: SoftTimeoutPolicy,
        extension: Option<Duration>,
    ) -> Self {
        self.soft_timeout_ratio = ratio;
        self.soft_timeout_policy = policy;
        self.soft_timeout_extension = extension;
        self
    }

    pub fn clone_with_context(&self, user_id: Option<String>, session_id: Option<String>) -> Self {
        let mut context = self.context.clone();
        context.user_id = user_id;
        context.session_id = session_id;
        context.notify_tool_used = Arc::new(AtomicBool::new(false));
        Self {
            tool_registry: Arc::clone(&self.tool_registry),
            context,
            prompt_profile: self.prompt_profile.clone(),
            prompter: self.prompter.clone(),
            session_grants: Arc::new(std::sync::RwLock::new(CapabilitySet::empty())),
            default_timeout: self.default_timeout,
            tool_timeouts: self.tool_timeouts.clone(),
            soft_timeout_ratio: self.soft_timeout_ratio,
            soft_timeout_policy: self.soft_timeout_policy,
            soft_timeout_extension: self.soft_timeout_extension,
        }
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        self.tool_registry.as_ref()
    }

    pub fn context(&self) -> &ToolContext {
        &self.context
    }

    pub async fn invoke_tool(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        self.invoke_tool_with_grants(tool, input, None).await
    }

    pub async fn invoke_tool_with_prompt_by_name(
        &self,
        name: &str,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self
            .tool_registry
            .get(name)
            .ok_or_else(|| ToolError::new(format!("unknown tool '{name}'")))?;
        self.invoke_tool_with_prompt(tool.as_ref(), input).await
    }

    pub async fn invoke_tool_with_grants(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
        extra_grants: Option<&CapabilitySet>,
    ) -> Result<ToolOutput, ToolError> {
        if self.context.execution_mode.is_scheduled_job()
            && self
                .context
                .notify_tool_used
                .load(std::sync::atomic::Ordering::Relaxed)
            && tool.spec().name != "notify"
        {
            tracing::info!(
                event = "tool_skipped",
                tool = %tool.spec().name,
                user_id = ?self.context.user_id,
                session_id = ?self.context.session_id,
                channel_id = ?self.context.channel_id,
                scheduled = self.context.execution_mode.is_scheduled_job(),
                reason = "scheduled_job_already_notified",
                "scheduled job already notified; skipping tool call"
            );
            return Ok(json!({
                "status": "skipped",
                "reason": "scheduled job already notified"
            }));
        }
        let span = tracing::info_span!(
            "tool_invoke",
            tool = %tool.spec().name,
            user_id = ?self.context.user_id,
            session_id = ?self.context.session_id,
            channel_id = ?self.context.channel_id,
            scheduled = self.context.execution_mode.is_scheduled_job(),
        );
        let _enter = span.enter();
        self.tool_registry.validate_input(tool, &input)?;

        let required = self
            .tool_registry
            .required_permissions(tool, &self.context, &input)?;
        tracing::info!(
            event = "tool_usage",
            tool = %tool.spec().name,
            user_id = ?self.context.user_id,
            session_id = ?self.context.session_id,
            channel_id = ?self.context.channel_id,
            scheduled = self.context.execution_mode.is_scheduled_job(),
            "tool usage requested"
        );
        let any_mode = tool.spec().name.as_str() == "schedule";
        let decision_source = if any_mode {
            if self.context.capabilities.allows_any(&required) {
                Some(DecisionSource::Capabilities)
            } else if extra_grants
                .map(|grants| grants.allows_any(&required))
                .unwrap_or(false)
            {
                Some(DecisionSource::ExtraGrants)
            } else if self.prompt_profile.pre_authorized.allows_any(&required) {
                Some(DecisionSource::PreAuthorized)
            } else if self
                .session_grants
                .read()
                .map(|grants| grants.allows_any(&required))
                .unwrap_or(false)
            {
                Some(DecisionSource::SessionGrants)
            } else if required
                .iter()
                .all(|permission| permission.is_auto_granted(&self.context))
            {
                Some(DecisionSource::AutoGranted)
            } else {
                None
            }
        } else if self.context.capabilities.allows_all(&required) {
            Some(DecisionSource::Capabilities)
        } else if extra_grants
            .map(|grants| grants.allows_all(&required))
            .unwrap_or(false)
        {
            Some(DecisionSource::ExtraGrants)
        } else if self.prompt_profile.pre_authorized.allows_all(&required) {
            Some(DecisionSource::PreAuthorized)
        } else if self
            .session_grants
            .read()
            .map(|grants| grants.allows_all(&required))
            .unwrap_or(false)
        {
            Some(DecisionSource::SessionGrants)
        } else if required
            .iter()
            .all(|permission| permission.is_auto_granted(&self.context))
        {
            Some(DecisionSource::AutoGranted)
        } else {
            None
        };
        if let Some(source) = decision_source {
            tracing::info!(
                event = "tool_decision",
                tool = %tool.spec().name,
                user_id = ?self.context.user_id,
                session_id = ?self.context.session_id,
                channel_id = ?self.context.channel_id,
                scheduled = self.context.execution_mode.is_scheduled_job(),
                decision = "allowed",
                decision_source = ?source,
                permissions = ?required,
                "tool permission granted"
            );
        } else {
            tracing::warn!(
                event = "tool_decision",
                tool = %tool.spec().name,
                user_id = ?self.context.user_id,
                session_id = ?self.context.session_id,
                channel_id = ?self.context.channel_id,
                scheduled = self.context.execution_mode.is_scheduled_job(),
                decision = "denied",
                decision_source = "none",
                permissions = ?required,
                "tool permission denied"
            );
            return Err(ToolError::permission_denied(
                format!("permission denied for tool '{}'", tool.spec().name),
                required,
            ));
        }
        if let Some(policy) = tool.pre_execution_policy(&self.context, &input)? {
            let policy_reason = policy.reason.as_deref().unwrap_or("unspecified");
            let policy_key = policy.policy_key.as_deref().unwrap_or("");
            match policy.decision {
                PreExecutionDecision::Allow => {
                    tracing::info!(
                        event = "tool_policy_decision",
                        tool = %tool.spec().name,
                        user_id = ?self.context.user_id,
                        session_id = ?self.context.session_id,
                        channel_id = ?self.context.channel_id,
                        scheduled = self.context.execution_mode.is_scheduled_job(),
                        decision = "allow",
                        reason = %policy_reason,
                        policy_key = %policy_key,
                        "tool policy decision"
                    );
                }
                PreExecutionDecision::RequireApproval => {
                    tracing::warn!(
                        event = "tool_policy_decision",
                        tool = %tool.spec().name,
                        user_id = ?self.context.user_id,
                        session_id = ?self.context.session_id,
                        channel_id = ?self.context.channel_id,
                        scheduled = self.context.execution_mode.is_scheduled_job(),
                        decision = "requires_approval",
                        reason = %policy_reason,
                        policy_key = %policy_key,
                        "tool policy requires approval"
                    );
                    return Err(ToolError::permission_denied(
                        format!("tool '{}' requires approval", tool.spec().name),
                        required,
                    ));
                }
                PreExecutionDecision::Deny => {
                    tracing::warn!(
                        event = "tool_policy_decision",
                        tool = %tool.spec().name,
                        user_id = ?self.context.user_id,
                        session_id = ?self.context.session_id,
                        channel_id = ?self.context.channel_id,
                        scheduled = self.context.execution_mode.is_scheduled_job(),
                        decision = "deny",
                        reason = %policy_reason,
                        policy_key = %policy_key,
                        "tool policy denied"
                    );
                    return Err(ToolError::new(format!(
                        "tool '{}' denied by policy",
                        tool.spec().name
                    )));
                }
            }
        }
        if let Some(grants) = extra_grants {
            let mut merged = self.context.capabilities.as_ref().clone();
            for permission in grants.permissions() {
                merged.insert(permission.clone());
            }
            let mut scoped = self.context.clone();
            scoped.capabilities = Arc::new(merged);
            let output = self.execute_with_timeout(tool, &scoped, input).await;
            match &output {
                Ok(_) => tracing::info!(
                    event = "tool_outcome",
                    tool = %tool.spec().name,
                    user_id = ?self.context.user_id,
                    session_id = ?self.context.session_id,
                    channel_id = ?self.context.channel_id,
                    scheduled = self.context.execution_mode.is_scheduled_job(),
                    outcome = "success",
                    "tool execution succeeded"
                ),
                Err(err) => tracing::error!(
                    event = "tool_outcome",
                    tool = %tool.spec().name,
                    user_id = ?self.context.user_id,
                    session_id = ?self.context.session_id,
                    channel_id = ?self.context.channel_id,
                    scheduled = self.context.execution_mode.is_scheduled_job(),
                    outcome = "error",
                    timed_out = err.is_timeout(),
                    error = %err,
                    "tool execution failed"
                ),
            }
            output
        } else {
            let output = self.execute_with_timeout(tool, &self.context, input).await;
            match &output {
                Ok(_) => tracing::info!(
                    event = "tool_outcome",
                    tool = %tool.spec().name,
                    user_id = ?self.context.user_id,
                    session_id = ?self.context.session_id,
                    channel_id = ?self.context.channel_id,
                    scheduled = self.context.execution_mode.is_scheduled_job(),
                    outcome = "success",
                    "tool execution succeeded"
                ),
                Err(err) => tracing::error!(
                    event = "tool_outcome",
                    tool = %tool.spec().name,
                    user_id = ?self.context.user_id,
                    session_id = ?self.context.session_id,
                    channel_id = ?self.context.channel_id,
                    scheduled = self.context.execution_mode.is_scheduled_job(),
                    outcome = "error",
                    timed_out = err.is_timeout(),
                    error = %err,
                    "tool execution failed"
                ),
            }
            output
        }
    }

    async fn execute_with_timeout(
        &self,
        tool: &dyn ToolExecutor,
        ctx: &ToolContext,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        let timeout = self
            .tool_timeouts
            .get(tool.spec().name.as_str())
            .copied()
            .unwrap_or(self.default_timeout);
        let mut task = Box::pin(tool.execute(ctx, input));
        let ratio = self.soft_timeout_ratio;
        let soft_timeout = soft_timeout_duration(timeout, ratio);
        if soft_timeout.is_zero() {
            return match tokio::time::timeout(timeout, task).await {
                Ok(result) => result,
                Err(_) => Err(ToolError::timeout(format!(
                    "tool '{}' timed out after {:?}",
                    tool.spec().name, timeout
                ))),
            };
        }

        let start = Instant::now();
        let soft_deadline = start + soft_timeout;
        let hard_deadline = start + timeout;
        let result = tokio::time::timeout_at(soft_deadline, &mut task).await;
        if let Ok(result) = result {
            return result;
        }

        let extension = self.soft_timeout_extension.unwrap_or(timeout);
        if extension.is_zero() {
            return match tokio::time::timeout_at(hard_deadline, &mut task).await {
                Ok(result) => result,
                Err(_) => Err(ToolError::timeout(format!(
                    "tool '{}' timed out after {:?}",
                    tool.spec().name, timeout
                ))),
            };
        }
        let decision = self
            .maybe_extend_timeout(
                tool.spec().name.as_str(),
                timeout,
                soft_timeout,
                extension,
                ctx,
            )
            .await;
        let total_timeout = match decision {
            TimeoutExtensionDecision::Extended => timeout.saturating_add(extension),
            TimeoutExtensionDecision::Declined => timeout,
        };
        let deadline = match decision {
            TimeoutExtensionDecision::Extended => hard_deadline + extension,
            TimeoutExtensionDecision::Declined => hard_deadline,
        };
        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        if remaining.is_zero() {
            return Err(ToolError::timeout(format!(
                "tool '{}' timed out after {:?}",
                tool.spec().name, total_timeout
            )));
        }
        match tokio::time::timeout_at(now + remaining, task).await {
            Ok(result) => result,
            Err(_) => Err(ToolError::timeout(format!(
                "tool '{}' timed out after {:?}",
                tool.spec().name, total_timeout
            ))),
        }
    }

    async fn maybe_extend_timeout(
        &self,
        tool_name: &str,
        hard_timeout: Duration,
        soft_timeout: Duration,
        extension: Duration,
        ctx: &ToolContext,
    ) -> TimeoutExtensionDecision {
        if ctx.execution_mode.is_scheduled_job() {
            return TimeoutExtensionDecision::Declined;
        }
        match self.soft_timeout_policy {
            SoftTimeoutPolicy::AutoExtend => {
                tracing::info!(
                    event = "timeout_extension",
                    tool = %tool_name,
                    decision = "auto_extend",
                    soft_timeout_secs = soft_timeout.as_secs_f64(),
                    extension_secs = extension.as_secs_f64(),
                    hard_timeout_secs = hard_timeout.as_secs_f64(),
                    "tool timeout auto-extended"
                );
                TimeoutExtensionDecision::Extended
            }
            SoftTimeoutPolicy::Prompt => {
                let Some(prompter) = self.prompter.as_ref() else {
                    tracing::info!(
                        event = "timeout_extension",
                        tool = %tool_name,
                        decision = "no_prompter",
                        "tool timeout extension skipped"
                    );
                    return TimeoutExtensionDecision::Declined;
                };
                tracing::info!(
                    event = "timeout_extension_prompt",
                    tool = %tool_name,
                    soft_timeout_secs = soft_timeout.as_secs_f64(),
                    extension_secs = extension.as_secs_f64(),
                    hard_timeout_secs = hard_timeout.as_secs_f64(),
                    prompt_timeout_secs = self.prompt_profile.prompt_timeout_secs,
                    "tool timeout extension prompt"
                );
                let decision = tokio::time::timeout(
                    Duration::from_secs(self.prompt_profile.prompt_timeout_secs),
                    prompter.prompt_timeout_extension(
                        tool_name,
                        soft_timeout,
                        extension,
                        self.prompt_profile.prompt_timeout_secs,
                    ),
                )
                .await
                .ok()
                .flatten();
                let approved = matches!(decision, Some(true));
                tracing::info!(
                    event = "timeout_extension_decision",
                    tool = %tool_name,
                    decision = if approved { "extended" } else { "declined" },
                    "tool timeout extension decision"
                );
                if approved {
                    TimeoutExtensionDecision::Extended
                } else {
                    TimeoutExtensionDecision::Declined
                }
            }
        }
    }

    pub async fn invoke_tool_with_prompt(
        &self,
        tool: &dyn ToolExecutor,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        match self.invoke_tool(tool, input.clone()).await {
            Ok(output) => Ok(output),
            Err(err) => {
                let required = match err.required_permissions() {
                    Some(required) => required,
                    None => return Err(err),
                };
                if !self.prompt_profile.allow_user_prompts
                    || self.context.execution_mode.is_scheduled_job()
                    || required
                        .iter()
                        .all(|permission| permission.is_auto_granted(&self.context))
                {
                    let reason = if self.context.execution_mode.is_scheduled_job() {
                        "scheduled_job"
                    } else if !self.prompt_profile.allow_user_prompts {
                        "prompts_disabled"
                    } else {
                        "auto_granted"
                    };
                    tracing::debug!(
                        event = "prompt_skipped",
                        reason,
                        tool = %tool.spec().name,
                        user_id = ?self.context.user_id,
                        session_id = ?self.context.session_id,
                        channel_id = ?self.context.channel_id,
                        permissions = ?required,
                        "prompt skipped"
                    );
                    return Err(err);
                }
                let promptable = match tool.spec().name.as_str() {
                    "schedule" => self.prompt_profile.max_allowed.allows_any(required),
                    _ => self.prompt_profile.max_allowed.allows_all(required),
                };
                if !promptable {
                    tracing::debug!(
                        event = "prompt_skipped",
                        reason = "not_in_max_allowed",
                        tool = %tool.spec().name,
                        user_id = ?self.context.user_id,
                        session_id = ?self.context.session_id,
                        channel_id = ?self.context.channel_id,
                        permissions = ?required,
                        "prompt skipped"
                    );
                    return Err(err);
                }
                let prompter = match self.prompter.as_ref() {
                    Some(prompter) => prompter.clone(),
                    None => {
                        tracing::debug!(
                            event = "prompt_skipped",
                            reason = "no_prompter",
                            tool = %tool.spec().name,
                            user_id = ?self.context.user_id,
                            session_id = ?self.context.session_id,
                            channel_id = ?self.context.channel_id,
                            permissions = ?required,
                            "prompt skipped"
                        );
                        return Err(err);
                    }
                };
                tracing::info!(
                    event = "prompt_issued",
                    tool = %tool.spec().name,
                    user_id = ?self.context.user_id,
                    session_id = ?self.context.session_id,
                    channel_id = ?self.context.channel_id,
                    permissions = ?required,
                    "prompt issued"
                );
                let decision = prompter
                    .prompt(
                        tool.spec().name.as_str(),
                        required,
                        self.prompt_profile.prompt_timeout_secs,
                    )
                    .await;
                match decision {
                    Some(crate::kernel::permissions::PromptDecision::AllowOnce) => {
                        tracing::info!(
                            event = "prompt_decision",
                            tool = %tool.spec().name,
                            user_id = ?self.context.user_id,
                            session_id = ?self.context.session_id,
                            channel_id = ?self.context.channel_id,
                            decision = "allow_once",
                            "prompt decision"
                        );
                        let mut grants = CapabilitySet::from_permissions(required);
                        for permission in self.prompt_profile.pre_authorized.permissions() {
                            grants.insert(permission.clone());
                        }
                        self.invoke_tool_with_grants(tool, input, Some(&grants))
                            .await
                    }
                    Some(crate::kernel::permissions::PromptDecision::AllowSession) => {
                        tracing::info!(
                            event = "prompt_decision",
                            tool = %tool.spec().name,
                            user_id = ?self.context.user_id,
                            session_id = ?self.context.session_id,
                            channel_id = ?self.context.channel_id,
                            decision = "allow_session",
                            "prompt decision"
                        );
                        if let Ok(mut session_grants) = self.session_grants.write() {
                            for permission in required {
                                session_grants.insert(permission.clone());
                            }
                        }
                        self.invoke_tool(tool, input).await
                    }
                    Some(crate::kernel::permissions::PromptDecision::Deny) => {
                        tracing::info!(
                            event = "prompt_decision",
                            tool = %tool.spec().name,
                            user_id = ?self.context.user_id,
                            session_id = ?self.context.session_id,
                            channel_id = ?self.context.channel_id,
                            decision = "deny",
                            "prompt decision"
                        );
                        Err(err)
                    }
                    None => {
                        tracing::info!(
                            event = "prompt_decision",
                            tool = %tool.spec().name,
                            user_id = ?self.context.user_id,
                            session_id = ?self.context.session_id,
                            channel_id = ?self.context.channel_id,
                            decision = "timeout",
                            "prompt decision"
                        );
                        Err(err)
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::Kernel;
    use crate::kernel::permissions::{
        CapabilitySet, ChannelPermissionProfile, PathPattern, Permission, PermissionPrompter,
        PromptDecision,
    };
    use crate::tools::registry::ToolRegistry;
    use crate::tools::traits::{
        PreExecutionPolicy,
        ToolContext,
        ToolError,
        ToolExecutor,
        ToolOutput,
        ToolSpec,
    };

    #[derive(Debug)]
    struct DummyTool {
        spec: ToolSpec,
    }

    impl DummyTool {
        fn new() -> Self {
            Self {
                spec: ToolSpec {
                    name: "dummy".to_string(),
                    description: "dummy tool".to_string(),
                    schema: json!({"type": "object"}),
                },
            }
        }
    }

    #[derive(Debug)]
    struct StaticTool {
        spec: ToolSpec,
        required: Vec<Permission>,
        output: ToolOutput,
    }

    impl StaticTool {
        fn new(name: &str, schema: serde_json::Value, required: Vec<Permission>) -> Self {
            Self {
                spec: ToolSpec {
                    name: name.to_string(),
                    description: "static tool".to_string(),
                    schema,
                },
                required,
                output: json!({"status": "ok"}),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for StaticTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        fn required_permissions(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Vec<Permission>, ToolError> {
            Ok(self.required.clone())
        }

        fn pre_execution_policy(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Option<PreExecutionPolicy>, ToolError> {
            Ok(None)
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(self.output.clone())
        }
    }

    #[derive(Clone)]
    struct MockPrompter {
        decision: Option<PromptDecision>,
        calls: Arc<AtomicUsize>,
    }

    impl MockPrompter {
        fn new(decision: Option<PromptDecision>) -> Self {
            Self {
                decision,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl PermissionPrompter for MockPrompter {
        async fn prompt(
            &self,
            _tool_name: &str,
            _permissions: &[Permission],
            _timeout_secs: u64,
        ) -> Option<PromptDecision> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decision
        }
    }

    fn read_permission() -> Permission {
        Permission::FileRead {
            path: PathPattern("/tmp/allowed.txt".to_string()),
        }
    }

    fn prompt_profile_for(required: &[Permission]) -> ChannelPermissionProfile {
        ChannelPermissionProfile {
            pre_authorized: CapabilitySet::empty(),
            max_allowed: CapabilitySet::from_permissions(required),
            allow_user_prompts: true,
            prompt_timeout_secs: 30,
        }
    }

    #[async_trait]
    impl ToolExecutor for DummyTool {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        fn required_permissions(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Vec<Permission>, ToolError> {
            Ok(vec![Permission::FileRead {
                path: PathPattern("/tmp/allowed.txt".to_string()),
            }])
        }

        fn pre_execution_policy(
            &self,
            _ctx: &ToolContext,
            _input: &serde_json::Value,
        ) -> Result<Option<PreExecutionPolicy>, ToolError> {
            Ok(None)
        }

        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> Result<ToolOutput, ToolError> {
            Ok(json!({"status": "ok"}))
        }
    }

    #[test]
    fn invoke_tool_denies_without_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new())).unwrap();
        let registry = Arc::new(registry);

        let kernel = Kernel::new(Arc::clone(&registry));
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(kernel.invoke_tool(tool.as_ref(), json!({})));

        assert!(result.is_err());
    }

    #[test]
    fn invoke_tool_allows_with_permission() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool::new())).unwrap();
        let registry = Arc::new(registry);

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert(Permission::FileRead {
            path: PathPattern("/tmp/allowed.txt".to_string()),
        });

        let kernel = Kernel::new(Arc::clone(&registry)).with_capabilities(capabilities);
        let tool = kernel.tool_registry().get("dummy").unwrap();
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(kernel.invoke_tool(tool.as_ref(), json!({})));

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invoke_tool_with_prompt_allow_once_does_not_persist() {
        let required = vec![read_permission()];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::AllowOnce)));
        let kernel = Kernel::new(Arc::clone(&registry))
            .with_prompt_profile(prompt_profile_for(&required))
            .with_prompter(Some(prompter));

        let output = kernel
            .invoke_tool_with_prompt_by_name("dummy", json!({}))
            .await;
        assert!(output.is_ok());

        let tool = kernel.tool_registry().get("dummy").unwrap();
        let second = kernel.invoke_tool(tool.as_ref(), json!({})).await;
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn invoke_tool_with_prompt_allow_session_persists() {
        let required = vec![read_permission()];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::AllowSession)));
        let kernel = Kernel::new(Arc::clone(&registry))
            .with_prompt_profile(prompt_profile_for(&required))
            .with_prompter(Some(prompter));

        let output = kernel
            .invoke_tool_with_prompt_by_name("dummy", json!({}))
            .await;
        assert!(output.is_ok());

        let kernel_no_prompt = kernel.clone().with_prompter(None);
        let tool = kernel_no_prompt.tool_registry().get("dummy").unwrap();
        let second = kernel_no_prompt.invoke_tool(tool.as_ref(), json!({})).await;
        assert!(second.is_ok());
    }

    #[tokio::test]
    async fn invoke_tool_with_prompt_deny_returns_error() {
        let required = vec![read_permission()];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::Deny)));
        let kernel = Kernel::new(Arc::clone(&registry))
            .with_prompt_profile(prompt_profile_for(&required))
            .with_prompter(Some(prompter));

        let result = kernel
            .invoke_tool_with_prompt_by_name("dummy", json!({}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.required_permissions().is_some());
    }

    #[tokio::test]
    async fn invoke_tool_with_prompt_disabled_when_scheduled_job() {
        let required = vec![read_permission()];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::AllowOnce)));
        let prompter_dyn: Arc<dyn PermissionPrompter> = prompter.clone();
        let kernel = Kernel::new(Arc::clone(&registry))
            .with_prompt_profile(prompt_profile_for(&required))
            .with_prompter(Some(prompter_dyn))
            .with_execution_mode(crate::tools::traits::ExecutionMode::ScheduledJob);

        let result = kernel
            .invoke_tool_with_prompt_by_name("dummy", json!({}))
            .await;
        assert!(result.is_err());
        assert_eq!(prompter.calls(), 0);
    }

    #[tokio::test]
    async fn invoke_tool_with_prompt_disabled_when_allow_user_prompts_false() {
        let required = vec![read_permission()];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let prompter = Arc::new(MockPrompter::new(Some(PromptDecision::AllowOnce)));
        let prompter_dyn: Arc<dyn PermissionPrompter> = prompter.clone();
        let mut profile = prompt_profile_for(&required);
        profile.allow_user_prompts = false;
        let kernel = Kernel::new(Arc::clone(&registry))
            .with_prompt_profile(profile)
            .with_prompter(Some(prompter_dyn));

        let result = kernel
            .invoke_tool_with_prompt_by_name("dummy", json!({}))
            .await;
        assert!(result.is_err());
        assert_eq!(prompter.calls(), 0);
    }

    #[tokio::test]
    async fn schedule_tool_allows_any_permission_match() {
        let required = vec![
            Permission::Schedule {
                action: "create".to_string(),
            },
            Permission::Schedule {
                action: "*".to_string(),
            },
        ];
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(StaticTool::new(
                "schedule",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        registry
            .register(Arc::new(StaticTool::new(
                "dummy2",
                json!({"type": "object"}),
                required.clone(),
            )))
            .unwrap();
        let registry = Arc::new(registry);

        let mut capabilities = CapabilitySet::empty();
        capabilities.insert(Permission::Schedule {
            action: "create".to_string(),
        });

        let kernel = Kernel::new(Arc::clone(&registry)).with_capabilities(capabilities);

        let schedule_tool = kernel.tool_registry().get("schedule").unwrap();
        let schedule_result = kernel.invoke_tool(schedule_tool.as_ref(), json!({})).await;
        assert!(schedule_result.is_ok());

        let dummy_tool = kernel.tool_registry().get("dummy2").unwrap();
        let dummy_result = kernel.invoke_tool(dummy_tool.as_ref(), json!({})).await;
        assert!(dummy_result.is_err());
    }
}
