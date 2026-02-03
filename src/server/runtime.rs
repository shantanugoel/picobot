use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::channels::adapter::{InboundAdapter, OutboundMessage, OutboundSender};
use crate::channels::permissions::ChannelPermissionProfile;
use crate::kernel::agent::Kernel;
use crate::kernel::agent_loop::{
    PermissionDecision, run_agent_loop_streamed_with_permissions_limit,
};
use crate::models::router::ModelRegistry;
use crate::session::adapter::{append_user_message, session_from_state, state_from_session};
use crate::session::manager::{Session, SessionManager};

pub async fn run_adapter_loop(
    inbound: Arc<dyn InboundAdapter>,
    outbound: Arc<dyn OutboundSender>,
    sessions: Arc<SessionManager>,
    kernel: Arc<Kernel>,
    models: Arc<ModelRegistry>,
    profile: ChannelPermissionProfile,
    max_tool_rounds: usize,
) {
    let mut stream = inbound.subscribe().await;
    while let Some(message) = stream.next().await {
        let (_session_id, mut session) = load_or_create_session(
            &sessions,
            message.channel_id.clone(),
            message.user_id.clone(),
            inbound.channel_type(),
            &profile,
        );
        append_user_message(&mut session, message.text.clone());
        let mut convo_state = state_from_session(&session);
        let model = models.default_model_arc();
        let mut response_text = String::new();
        let mut on_token = |token: &str| {
            response_text.push_str(token);
        };
        let mut on_permission = |_: &str, required: &[crate::kernel::permissions::Permission]| {
            if !profile.allow_user_prompts {
                return PermissionDecision::Deny;
            }
            if !profile.max_capabilities().allows_all(required) {
                return PermissionDecision::Deny;
            }
            PermissionDecision::Session
        };
        let result = run_agent_loop_streamed_with_permissions_limit(
            kernel.as_ref(),
            model.as_ref(),
            &mut convo_state,
            message.text,
            &mut on_token,
            &mut on_permission,
            &mut |_| {},
            max_tool_rounds,
        )
        .await;
        if let Ok(text) = result {
            if response_text.is_empty() {
                response_text = text;
            }
            session_from_state(&mut session, &convo_state);
            sessions.update_session(session);
            let _ = outbound
                .send(OutboundMessage {
                    channel_id: message.channel_id,
                    user_id: message.user_id,
                    text: response_text,
                })
                .await;
        }
    }
}

fn load_or_create_session(
    sessions: &SessionManager,
    channel_id: String,
    user_id: String,
    channel_type: crate::channels::adapter::ChannelType,
    profile: &ChannelPermissionProfile,
) -> (String, Session) {
    let session_id = format!("{}:{}", channel_id, user_id);
    if let Some(session) = sessions.get_session(&session_id) {
        return (session_id, session);
    }
    let session = sessions.create_session(
        session_id.clone(),
        channel_type,
        channel_id,
        user_id,
        profile,
    );
    (session_id, session)
}
