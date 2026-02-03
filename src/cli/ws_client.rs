use std::thread;

use futures::{SinkExt, StreamExt};
use http::HeaderValue;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use crate::channels::websocket::{WsClientMessage, WsServerMessage};

#[derive(Debug)]
pub enum WsUiMessage {
    Session(String),
    WhatsappQr(String),
    Token(String),
    Done(String),
    Error(String),
    PermissionRequired {
        tool: String,
        permissions: Vec<String>,
        request_id: String,
    },
}

pub struct WsClientHandle {
    pub outbound: mpsc::UnboundedSender<WsClientMessage>,
    pub inbound: std::sync::mpsc::Receiver<WsUiMessage>,
}

pub fn spawn_ws_client(url: String, api_key: Option<String>) -> WsClientHandle {
    let (outbound, mut outbound_rx) = mpsc::unbounded_channel::<WsClientMessage>();
    let (inbound_tx, inbound_rx) = std::sync::mpsc::channel::<WsUiMessage>();

    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        let Ok(runtime) = runtime else {
            let _ = inbound_tx.send(WsUiMessage::Error("failed to start runtime".to_string()));
            return;
        };

        runtime.block_on(async move {
            let Ok(url) = Url::parse(&url) else {
                let _ = inbound_tx.send(WsUiMessage::Error("invalid ws url".to_string()));
                return;
            };
            let mut request: http::Request<()> = match url.as_str().into_client_request() {
                Ok(request) => request,
                Err(err) => {
                    let _ = inbound_tx.send(WsUiMessage::Error(err.to_string()));
                    return;
                }
            };
            if let Some(api_key) = api_key
                && let Ok(value) = HeaderValue::from_str(&api_key) {
                request.headers_mut().insert("x-api-key", value);
            }
            let ws_stream = match connect_async(request).await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    let _ = inbound_tx.send(WsUiMessage::Error(err.to_string()));
                    return;
                }
            };
            let (mut write, mut read) = ws_stream.split();

            loop {
                tokio::select! {
                    Some(msg) = outbound_rx.recv() => {
                        if let Ok(payload) = serde_json::to_string(&msg)
                            && write.send(Message::Text(payload.into())).await.is_err() {
                            let _ = inbound_tx.send(WsUiMessage::Error("ws send failed".to_string()));
                            break;
                        }
                    }
                    Some(result) = read.next() => {
                        let message = match result {
                            Ok(message) => message,
                            Err(err) => {
                                let _ = inbound_tx.send(WsUiMessage::Error(err.to_string()));
                                break;
                            }
                        };
                        match message {
                            Message::Text(text) => {
                                if let Ok(event) = serde_json::from_str::<WsServerMessage>(&text) {
                                    match event {
                                        WsServerMessage::Session { session_id } => {
                                            let _ = inbound_tx.send(WsUiMessage::Session(session_id));
                                        }
                                        WsServerMessage::WhatsappQr { code } => {
                                            let _ = inbound_tx.send(WsUiMessage::WhatsappQr(code));
                                        }
                                        WsServerMessage::Token { token } => {
                                            let _ = inbound_tx.send(WsUiMessage::Token(token));
                                        }
                                        WsServerMessage::Done { response, .. } => {
                                            let _ = inbound_tx.send(WsUiMessage::Done(response));
                                        }
                                        WsServerMessage::Error { error } => {
                                            let _ = inbound_tx.send(WsUiMessage::Error(error));
                                        }
                                        WsServerMessage::PermissionRequired { tool, permissions, request_id } => {
                                            let _ = inbound_tx.send(WsUiMessage::PermissionRequired {
                                                tool,
                                                permissions,
                                                request_id,
                                            });
                                        }
                                        WsServerMessage::Pong => {}
                                    }
                                }
                            }
                            Message::Close(_) => break,
                            _ => {}
                        }
                    }
                }
            }
        });
    });

    WsClientHandle {
        outbound,
        inbound: inbound_rx,
    }
}
