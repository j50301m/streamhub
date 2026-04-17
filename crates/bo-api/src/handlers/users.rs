//! Admin user management handlers.

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{Duration, Utc};
use entity::user::UserRole;
use error::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::extractors::AdminUser;
use crate::state::BoAppState;

// ── Request / Response types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListUsersQuery {
    #[serde(default = "default_page")]
    pub page: u64,
    #[serde(default = "default_per_page")]
    pub per_page: u64,
    pub q: Option<String>,
    pub role: Option<UserRole>,
    pub suspended: Option<bool>,
}

fn default_page() -> u64 {
    1
}
fn default_per_page() -> u64 {
    20
}

#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub role: UserRole,
    pub is_suspended: bool,
    pub suspended_until: Option<chrono::DateTime<Utc>>,
    pub suspension_reason: Option<String>,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ListUsersResponse {
    pub data: ListUsersData,
}

#[derive(Debug, Serialize)]
pub struct ListUsersData {
    pub users: Vec<UserResponse>,
    pub total: u64,
    pub page: u64,
    pub per_page: u64,
}

#[derive(Debug, Serialize)]
pub struct DataResponse<T: Serialize> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleRequest {
    pub role: UserRole,
}

#[derive(Debug, Deserialize)]
pub struct SuspendRequest {
    pub duration_secs: Option<u64>,
    pub reason: Option<String>,
}

fn user_to_response(m: &entity::user::Model) -> UserResponse {
    UserResponse {
        id: m.id,
        email: m.email.clone(),
        role: m.role.clone(),
        is_suspended: m.is_suspended,
        suspended_until: m.suspended_until,
        suspension_reason: m.suspension_reason.clone(),
        created_at: m.created_at,
    }
}

// ── Handlers ───────────────────────────────────────────────────────

/// `GET /v1/admin/users` — list users with search / filter / pagination.
#[tracing::instrument(
    skip(state, _admin),
    fields(
        admin_id = %_admin.0.id,
        page = query.page,
        per_page = query.per_page,
        role = ?query.role,
        suspended = ?query.suspended,
    )
)]
pub async fn list_users(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<ListUsersResponse>, AppError> {
    let per_page = query.per_page.clamp(1, 100);
    let page = query.page.max(1);

    let result = state
        .uow
        .user_repo()
        .find_users_paginated(
            page,
            per_page,
            query.q.as_deref(),
            query.role,
            query.suspended,
        )
        .await?;

    let users: Vec<UserResponse> = result.items.iter().map(user_to_response).collect();

    Ok(Json(ListUsersResponse {
        data: ListUsersData {
            users,
            total: result.total,
            page,
            per_page,
        },
    }))
}

/// `PATCH /v1/admin/users/:id/role` — update a user's role.
#[tracing::instrument(
    skip(state, payload),
    fields(admin_id = %admin.0.id, user_id = %user_id, new_role = ?payload.role)
)]
pub async fn update_role(
    admin: AdminUser,
    State(state): State<BoAppState>,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<UpdateRoleRequest>,
) -> Result<Json<DataResponse<UserResponse>>, AppError> {
    if admin.0.id == user_id {
        return Err(AppError::BadRequest(
            "Cannot change your own role".to_string(),
        ));
    }

    let model = state
        .uow
        .user_repo()
        .update_role(user_id, payload.role)
        .await
        .map_err(|e| match e {
            repo::RepoError::NotFound => AppError::NotFound("USER_NOT_FOUND".to_string()),
            other => AppError::from(other),
        })?;

    Ok(Json(DataResponse {
        data: user_to_response(&model),
    }))
}

/// `POST /v1/admin/users/:id/suspend` — suspend a user.
#[tracing::instrument(
    skip(state, payload),
    fields(
        admin_id = %admin.0.id,
        user_id = %user_id,
        duration_secs = ?payload.duration_secs,
    )
)]
pub async fn suspend(
    admin: AdminUser,
    State(state): State<BoAppState>,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<SuspendRequest>,
) -> Result<Json<DataResponse<UserResponse>>, AppError> {
    if admin.0.id == user_id {
        return Err(AppError::BadRequest("Cannot suspend yourself".to_string()));
    }

    let until = payload
        .duration_secs
        .map(|secs| Utc::now() + Duration::seconds(secs as i64));

    let model = suspend_update_db(&state, user_id, until, payload.reason.clone()).await?;
    suspend_set_cache(&state, user_id, payload.duration_secs).await;
    suspend_broadcast(&state, user_id).await;

    Ok(Json(DataResponse {
        data: user_to_response(&model),
    }))
}

/// Persists the suspension to Postgres. Maps `NotFound` into a 404 so the
/// admin client can distinguish missing users from transient DB failures.
#[tracing::instrument(skip(state), fields(user_id = %user_id))]
async fn suspend_update_db(
    state: &BoAppState,
    user_id: Uuid,
    until: Option<chrono::DateTime<Utc>>,
    reason: Option<String>,
) -> Result<entity::user::Model, AppError> {
    state
        .uow
        .user_repo()
        .set_suspended(user_id, until, reason)
        .await
        .map_err(|e| match e {
            repo::RepoError::NotFound => AppError::NotFound("USER_NOT_FOUND".to_string()),
            other => AppError::from(other),
        })
}

/// Writes the `user:state:{user_id} = "suspended"` flag used by api's
/// access-state negative cache (SPEC-014). Failure is non-fatal.
#[tracing::instrument(skip(state), fields(user_id = %user_id, ttl_secs = ?ttl))]
async fn suspend_set_cache(state: &BoAppState, user_id: Uuid, ttl: Option<u64>) {
    let key = mediamtx::keys::user_state(&user_id);
    if let Err(e) = state.cache.set(&key, "suspended", ttl).await {
        tracing::warn!(error = %e, "Failed to write user_state cache");
    }
}

/// Publishes `user_suspended` on the shared pub/sub channel so live API
/// instances drop any open WS for this user. Failure is non-fatal.
#[tracing::instrument(skip(state), fields(user_id = %user_id))]
async fn suspend_broadcast(state: &BoAppState, user_id: Uuid) {
    let event = serde_json::json!({ "user_id": user_id });
    if let Err(e) = state
        .pubsub
        .publish(mediamtx::keys::USER_SUSPENDED_CHANNEL, &event.to_string())
        .await
    {
        tracing::warn!(error = %e, "Failed to publish user_suspended event");
    }
}

/// `DELETE /v1/admin/users/:id/suspend` — unsuspend a user (idempotent).
#[tracing::instrument(skip(state, _admin), fields(admin_id = %_admin.0.id, user_id = %user_id))]
pub async fn unsuspend(
    _admin: AdminUser,
    State(state): State<BoAppState>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<DataResponse<UserResponse>>, AppError> {
    let model = state
        .uow
        .user_repo()
        .clear_suspended(user_id)
        .await
        .map_err(|e| match e {
            repo::RepoError::NotFound => AppError::NotFound("USER_NOT_FOUND".to_string()),
            other => AppError::from(other),
        })?;

    // Redis: SET user:state:{user_id} "active" EX 300
    let key = mediamtx::keys::user_state(&user_id);
    let _ = state.cache.set(&key, "active", Some(300)).await;

    Ok(Json(DataResponse {
        data: user_to_response(&model),
    }))
}
