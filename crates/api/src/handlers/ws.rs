use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use tokio::sync::mpsc;
use uuid::Uuid;

use common::AppState;

use crate::ws::manager::WsManager;
use crate::ws::types::{ClientMessage, ServerMessage};

/// GET /v1/ws — WebSocket upgrade endpoint.
pub(crate) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
    axum::Extension(ws_manager): axum::Extension<Arc<WsManager>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, ws_manager))
}

async fn handle_ws(mut socket: WebSocket, ws_manager: Arc<WsManager>) {
    let conn_id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    ws_manager.add_connection(conn_id, tx).await;

    // Send initial live streams state
    let cached = ws_manager.get_cached_live_streams().await;
    let init_msg = ServerMessage::LiveStreams { data: cached };
    if let Ok(text) = serde_json::to_string(&init_msg) {
        if socket.send(Message::Text(text.into())).await.is_err() {
            ws_manager.remove_connection(&conn_id).await;
            return;
        }
    }

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
