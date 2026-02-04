use std::sync::Arc;

use tokio_stream::StreamExt;

use crate::channels::adapter::{InboundAdapter, OutboundMessage};
use crate::channels::permissions::ChannelPermissionProfile;
use crate::delivery::queue::DeliveryQueue;
use crate::kernel::agent::Kernel;
use crate::kernel::agent_loop::{
    PermissionDecision, run_agent_loop_streamed_with_permissions_limit,
};
use crate::models::router::ModelRegistry;
use crate::session::adapter::{append_user_message, session_from_state, state_from_session};
use crate::session::manager::Session;
use crate::session::persistent_manager::PersistentSessionManager;

pub async fn run_adapter_loop(
    inbound: Arc<dyn InboundAdapter>,
    delivery_queue: DeliveryQueue,
    sessions: Arc<PersistentSessionManager>,
    kernel: Arc<Kernel>,
    models: Arc<ModelRegistry>,
    profile: ChannelPermissionProfile,
    max_tool_rounds: usize,
) {
    let mut stream = inbound.subscribe().await;
    while let Some(message) = stream.next().await {
        let (_session_id, mut session) = match load_or_create_session(
            &sessions,
            message.channel_id.clone(),
            message.user_id.clone(),
            inbound.channel_type(),
            &profile,
        ) {
            Ok(result) => result,
            Err(_) => continue,
        };
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
        let scoped_kernel =
            kernel.clone_with_context(Some(message.user_id.clone()), Some(session.id.clone()));
        let mut on_debug = |line: &str| {
            eprintln!("Adapter debug: {line}");
        };
        let result = run_agent_loop_streamed_with_permissions_limit(
            &scoped_kernel,
            model.as_ref(),
            &mut convo_state,
            message.text,
            &mut on_token,
            &mut on_permission,
            &mut on_debug,
            max_tool_rounds,
        )
        .await;
        match result {
            Ok(text) => {
                if response_text.is_empty() {
                    response_text = text;
                }
                session_from_state(&mut session, &convo_state);
                let _ = sessions.update_session(&session);
                let outbound_message = OutboundMessage {
                    channel_id: message.channel_id,
                    user_id: message.user_id,
                    text: response_text,
                };
                let _delivery_id = delivery_queue.enqueue(outbound_message).await;
            }
            Err(err) => {
                let outbound_message = OutboundMessage {
                    channel_id: message.channel_id,
                    user_id: message.user_id,
                    text: format!("Sorry, I hit an upstream error. Please try again. ({err})"),
                };
                let _delivery_id = delivery_queue.enqueue(outbound_message).await;
            }
        }
    }
}

fn load_or_create_session(
    sessions: &PersistentSessionManager,
    channel_id: String,
    user_id: String,
    channel_type: crate::channels::adapter::ChannelType,
    profile: &ChannelPermissionProfile,
) -> Result<(String, Session), String> {
    let session_id = format!("{}:{}", channel_id, user_id);
    if let Ok(Some(session)) = sessions.get_session(&session_id) {
        return Ok((session_id, session));
    }
    let session = sessions
        .create_session(
            session_id.clone(),
            channel_type,
            channel_id,
            user_id,
            profile,
        )
        .map_err(|err| err.to_string())?;
    Ok((session_id, session))
}
