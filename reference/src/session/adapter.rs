use crate::kernel::agent_loop::ConversationState;
use crate::models::types::Message;
use crate::session::manager::Session;

pub fn state_from_session(session: &Session) -> ConversationState {
    let mut state = ConversationState::new();
    for message in &session.conversation {
        state.push(message.clone());
    }
    state.set_session_grants(session.permissions.clone());
    state
}

pub fn session_from_state(session: &mut Session, state: &ConversationState) {
    session.conversation = state.messages().to_vec();
    session.permissions = state.session_grants().clone();
}

pub fn append_user_message(session: &mut Session, content: String) {
    session.conversation.push(Message::user(content));
}
