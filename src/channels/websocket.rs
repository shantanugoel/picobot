use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMessage {
    Chat {
        session_id: Option<String>,
        user_id: Option<String>,
        message: String,
        model: Option<String>,
    },
    PermissionDecision {
        request_id: String,
        decision: PermissionDecisionChoice,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMessage {
    Session {
        session_id: String,
    },
    WhatsappQr {
        code: String,
    },
    Token {
        token: String,
    },
    Done {
        response: String,
        session_id: String,
    },
    Error {
        error: String,
    },
    PermissionRequired {
        tool: String,
        permissions: Vec<String>,
        request_id: String,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionChoice {
    Once,
    Session,
    Deny,
}
