use axum::Json;
use axum::extract::State;
use chrono::Utc;
use common::{AppError, AppState};
use entity::stream::StreamStatus;
use entity::user::UserRole;
use mediamtx::keys;
use serde::Serialize;
use uuid::Uuid;

use crate::middleware::AdminUser;

/// Response wrapper for `GET /v1/admin/dashboard`.
#[derive(Debug, Serialize)]
pub struct DashboardResponse {
    /// Dashboard payload.
    pub data: DashboardData,
}

/// Platform summary data shown on the admin dashboard.
#[derive(Debug, Serialize)]
pub struct DashboardData {
    /// Number of currently live streams.
    pub live_stream_count: u64,
    /// Total registered users.
    pub total_user_count: u64,
    /// Users with the broadcaster role.
    pub broadcaster_count: u64,
    /// Streams that ended in the last 24 hours.
    pub ended_streams_24h: u64,
    /// Streams currently in error state.
    pub error_stream_count: u64,
    /// Up to 10 most recent live streams with owner info.
    pub recent_live_streams: Vec<RecentLiveStream>,
}

/// A single live stream entry in the dashboard.
#[derive(Debug, Serialize)]
pub struct RecentLiveStream {
    /// Stream UUID.
    pub id: Uuid,
    /// Display title.
    pub title: Option<String>,
    /// MediaMTX path key.
    pub stream_key: String,
    /// Owner email (from users table).
    pub user_email: Option<String>,
    /// When the stream went live.
    pub started_at: Option<chrono::DateTime<Utc>>,
    /// Current viewer count from Redis cache.
    pub viewer_count: u32,
}

/// `GET /v1/admin/dashboard` — returns platform summary for the admin console.
pub async fn dashboard(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Result<Json<DashboardResponse>, AppError> {
    let stream_repo = state.uow.stream_repo();
    let user_repo = state.uow.user_repo();

    let since_24h = Utc::now() - chrono::Duration::hours(24);

    let (live_count, error_count, ended_24h, total_users, broadcaster_count, live_streams) = tokio::try_join!(
        async {
            stream_repo
                .count_by_status(StreamStatus::Live)
                .await
                .map_err(AppError::from)
        },
        async {
            stream_repo
                .count_by_status(StreamStatus::Error)
                .await
                .map_err(AppError::from)
        },
        async {
            stream_repo
                .count_ended_since(since_24h)
                .await
                .map_err(AppError::from)
        },
        async { user_repo.count_all().await.map_err(AppError::from) },
        async {
            user_repo
                .count_by_role(UserRole::Broadcaster)
                .await
                .map_err(AppError::from)
        },
        async {
            stream_repo
                .list_live_limited(10)
                .await
                .map_err(AppError::from)
        },
    )?;

    let mut recent_live_streams = Vec::with_capacity(live_streams.len());
    for s in &live_streams {
        let user_email = if let Some(uid) = s.user_id {
            user_repo
                .find_by_id(uid)
                .await
                .map_err(AppError::from)?
                .map(|u| u.email)
        } else {
            None
        };

        let viewer_count = state
            .cache
            .get(&keys::viewer_count(&s.id))
            .await
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);

        recent_live_streams.push(RecentLiveStream {
            id: s.id,
            title: s.title.clone(),
            stream_key: s.stream_key.clone(),
            user_email,
            started_at: s.started_at,
            viewer_count,
        });
    }

    Ok(Json(DashboardResponse {
        data: DashboardData {
            live_stream_count: live_count,
            total_user_count: total_users,
            broadcaster_count,
            ended_streams_24h: ended_24h,
            error_stream_count: error_count,
            recent_live_streams,
        },
    }))
}
