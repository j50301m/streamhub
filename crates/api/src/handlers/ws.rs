use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use error::AppError;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::state::AppState;

use crate::handlers::chat::{self, ChatUser};
use crate::ws::manager::WsManager;
use crate::ws::types::{ClientMessage, ServerMessage};

/// Query params accepted on the WebSocket upgrade.
/// `token` is an optional JWT access token — connections without one can only
/// subscribe/receive chat, not send.
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    token: Option<String>,
}

/// GET /v1/ws — WebSocket upgrade endpoint.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    axum::Extension(ws_manager): axum::Extension<Arc<WsManager>>,
    Query(query): Query<WsQuery>,
) -> Response {
    let authed = match resolve_user(&state, query.token.as_deref()).await {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };
    ws.on_upgrade(move |socket| handle_ws(socket, state, ws_manager, authed))
        .into_response()
}

/// Resolves an authenticated user from an optional JWT token.
///
/// - `Ok(Some(user))` — valid token, active user
/// - `Ok(None)` — no token provided (anonymous connection allowed)
/// - `Err(AppError::Forbidden)` — token provided but user is suspended (reject upgrade)
async fn resolve_user(state: &AppState, token: Option<&str>) -> Result<Option<ChatUser>, AppError> {
    let Some(token) = token else {
        return Ok(None);
    };

    let claims = match auth::jwt::verify_token(token, &state.config.jwt_secret) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    if claims.typ != "access" {
        return Ok(None);
    }

    // Access-state check — reject suspended users from WS upgrade
    let access = auth::access_state::load_user_access_state(
        state.cache.as_ref(),
        state.uow.db(),
        claims.sub,
    )
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    if access == auth::access_state::AccessState::Suspended {
        return Err(AppError::Forbidden("ACCOUNT_SUSPENDED".to_string()));
    }

    let model = state
        .uow
        .user_repo()
        .find_by_id(claims.sub)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::Unauthorized("TOKEN_INVALID".to_string()))?;

    Ok(Some(ChatUser {
        id: model.id,
        email: model.email,
    }))
}

async fn handle_ws(
    mut socket: WebSocket,
    state: AppState,
    ws_manager: Arc<WsManager>,
    authed: Option<ChatUser>,
) {
    let conn_id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    ws_manager.add_connection(conn_id, tx).await;

    // Track user_id → conn_id for suspend-disconnect
    if let Some(ref user) = authed {
        ws_manager.track_user_connection(user.id, conn_id).await;
    }

    // Send initial live streams state
    let cached = ws_manager.get_cached_live_streams().await;
    let init_msg = ServerMessage::LiveStreams { data: cached };
    if let Ok(text) = serde_json::to_string(&init_msg) {
        if socket.send(Message::Text(text.into())).await.is_err() {
            ws_manager.remove_connection(&conn_id).await;
            return;
        }
    }

    // Chat rooms this connection subscribed to on this instance; used to
    // spawn the per-room pubsub task only on first subscribe per instance.
    let mut chat_rooms_seen: HashMap<Uuid, ()> = HashMap::new();

    loop {
        tokio::select! {
            // Forward queued outgoing messages to the WebSocket
            Some(msg) = rx.recv() => {
                if socket.send(msg).await.is_err() {
                    break;
                }
            }
            // Read incoming messages from the WebSocket
            result = socket.recv() => {
                match result {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            match client_msg {
                                ClientMessage::Subscribe { stream_id } => {
                                    ws_manager.subscribe_stream(conn_id, stream_id).await;
                                }
                                ClientMessage::Unsubscribe { stream_id } => {
                                    ws_manager.unsubscribe_stream(&conn_id, &stream_id).await;
                                }
                                ClientMessage::SubscribeChat { stream_id } => {
                                    if chat_rooms_seen.insert(stream_id, ()).is_none() {
                                        // First local subscriber for this room → ensure the
                                        // Redis pubsub fan-out task exists.
                                        chat::ensure_chat_pubsub_task(
                                            state.pubsub.clone(),
                                            ws_manager.clone(),
                                            stream_id,
                                        ).await;
                                    }
                                    chat::handle_subscribe_chat(
                                        state.cache.as_ref(),
                                        ws_manager.as_ref(),
                                        conn_id,
                                        stream_id,
                                    ).await;
                                }
                                ClientMessage::UnsubscribeChat { stream_id } => {
                                    chat::handle_unsubscribe_chat(
                                        ws_manager.as_ref(),
                                        conn_id,
                                        stream_id,
                                    ).await;
                                }
                                ClientMessage::SendChat { stream_id, content } => {
                                    let outcome = chat::handle_send_chat(
                                        state.cache.as_ref(),
                                        state.pubsub.as_ref(),
                                        &state.uow,
                                        authed.as_ref(),
                                        stream_id,
                                        content,
                                    ).await;
                                    if let chat::SendChatOutcome::Rejected(reason) = outcome {
                                        let err = ServerMessage::ChatError {
                                            stream_id: Some(stream_id),
                                            reason,
                                        };
                                        ws_manager.send_to_connection(&conn_id, &err).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    ws_manager.remove_connection(&conn_id).await;
    tracing::debug!(conn_id = %conn_id, "WebSocket connection closed");
}
