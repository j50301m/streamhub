//! Stream entity queries. Callers usually go through [`crate::traits::StreamRepoRef`].

use entity::{stream, user};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::RepoError;

/// A stream joined with its owner's email.
pub struct StreamWithOwner {
    /// The stream model.
    pub stream: stream::Model,
    /// Owner email from the users table, if the stream has a user_id.
    pub owner_email: Option<String>,
}

/// Finds a stream by its UUID primary key.
pub async fn find_by_id(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<stream::Model>, RepoError> {
    stream::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(RepoError::from)
}

/// Finds a stream by its public `stream_key`.
pub async fn find_by_key(
    conn: &impl ConnectionTrait,
    key: &str,
) -> Result<Option<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::StreamKey.eq(key))
        .one(conn)
        .await
        .map_err(RepoError::from)
}

/// Finds a stream by `stream_key` taking a row-level exclusive (`FOR UPDATE`)
/// lock. Must be called inside a transaction.
pub async fn find_by_key_for_update(
    conn: &impl ConnectionTrait,
    key: &str,
) -> Result<Option<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::StreamKey.eq(key))
        .lock_exclusive()
        .one(conn)
        .await
        .map_err(RepoError::from)
}

/// Finds a stream by id taking a row-level exclusive (`FOR UPDATE`) lock.
/// Must be called inside a transaction.
pub async fn find_by_id_for_update(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<stream::Model>, RepoError> {
    stream::Entity::find_by_id(id)
        .lock_exclusive()
        .one(conn)
        .await
        .map_err(RepoError::from)
}

/// Returns all streams currently in [`stream::StreamStatus::Live`], newest
/// `started_at` first.
pub async fn list_live(conn: &impl ConnectionTrait) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Live))
        .order_by_desc(stream::Column::StartedAt)
        .all(conn)
        .await
        .map_err(RepoError::from)
}

/// Returns ended streams whose VOD is ready, newest `ended_at` first.
pub async fn list_vod(conn: &impl ConnectionTrait) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Ended))
        .filter(stream::Column::VodStatus.eq(stream::VodStatus::Ready))
        .order_by_desc(stream::Column::EndedAt)
        .all(conn)
        .await
        .map_err(RepoError::from)
}

/// One page of streams along with the total matching row count.
pub struct PaginatedResult {
    /// Rows for the requested page.
    pub items: Vec<stream::Model>,
    /// Total rows matching the filter (not just this page).
    pub total: u64,
}

/// Lists streams owned by `user_id`, optionally filtered by status.
/// `page` is 1-indexed; `per_page` sets the page size.
pub async fn list_by_user(
    conn: &impl ConnectionTrait,
    user_id: Uuid,
    status_filter: Option<stream::StreamStatus>,
    page: u64,
    per_page: u64,
) -> Result<PaginatedResult, RepoError> {
    let mut query = stream::Entity::find().filter(stream::Column::UserId.eq(user_id));

    if let Some(status) = status_filter {
        query = query.filter(stream::Column::Status.eq(status));
    }

    let total = query.clone().count(conn).await?;

    let items = query
        .order_by_desc(stream::Column::CreatedAt)
        .paginate(conn, per_page)
        .fetch_page(page - 1)
        .await?;

    Ok(PaginatedResult { items, total })
}

/// Inserts a new stream row and returns the persisted model.
pub async fn create(
    conn: &impl ConnectionTrait,
    model: stream::ActiveModel,
) -> Result<stream::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}

/// Updates an existing stream row and returns the persisted model.
pub async fn update(
    conn: &impl ConnectionTrait,
    model: stream::ActiveModel,
) -> Result<stream::Model, RepoError> {
    model.update(conn).await.map_err(RepoError::from)
}

/// Counts streams with the given status.
pub async fn count_by_status(
    conn: &impl ConnectionTrait,
    status: stream::StreamStatus,
) -> Result<u64, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(status))
        .count(conn)
        .await
        .map_err(RepoError::from)
}

/// Counts streams that ended on or after the given timestamp.
pub async fn count_ended_since(
    conn: &impl ConnectionTrait,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<u64, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Ended))
        .filter(stream::Column::EndedAt.gte(since))
        .count(conn)
        .await
        .map_err(RepoError::from)
}

/// Returns live streams ordered by `started_at` descending, up to `limit`.
pub async fn list_live_limited(
    conn: &impl ConnectionTrait,
    limit: u64,
) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Live))
        .order_by_desc(stream::Column::StartedAt)
        .limit(limit)
        .all(conn)
        .await
        .map_err(RepoError::from)
}

/// Deletes a stream by UUID. Succeeds silently if the row is absent.
pub async fn delete(conn: &impl ConnectionTrait, id: Uuid) -> Result<(), RepoError> {
    stream::Entity::delete_by_id(id)
        .exec(conn)
        .await
        .map(|_| ())
        .map_err(RepoError::from)
}

/// Lists all streams with optional status filter and text search (title or
/// owner email via ILIKE). Results are ordered by `created_at` descending.
/// `page` is 1-indexed.
///
/// When `q` is provided, matching user IDs are resolved first so the stream
/// query can filter on `user_id IN (...)` in addition to `title ILIKE`.
pub async fn find_streams_paginated(
    conn: &impl ConnectionTrait,
    page: u64,
    per_page: u64,
    status: Option<stream::StreamStatus>,
    q: Option<&str>,
) -> Result<PaginatedResult, RepoError> {
    let mut query = stream::Entity::find();

    if let Some(status) = status {
        query = query.filter(stream::Column::Status.eq(status));
    }

    if let Some(q) = q {
        if !q.is_empty() {
            let pattern = format!("%{q}%");
            // Find user IDs matching the search term by email.
            let matching_user_ids: Vec<Uuid> = user::Entity::find()
                .filter(user::Column::Email.like(&pattern))
                .select_only()
                .column(user::Column::Id)
                .into_tuple()
                .all(conn)
                .await?;

            let mut cond = Condition::any().add(stream::Column::Title.like(&pattern));
            if !matching_user_ids.is_empty() {
                cond = cond.add(stream::Column::UserId.is_in(matching_user_ids));
            }
            query = query.filter(cond);
        }
    }

    let total = query.clone().count(conn).await?;

    let items = query
        .order_by_desc(stream::Column::CreatedAt)
        .paginate(conn, per_page)
        .fetch_page(page - 1)
        .await?;

    Ok(PaginatedResult { items, total })
}

/// Returns streams that are currently live or ended within the last 24 hours.
/// Used for cross-stream moderation ban aggregation.
pub async fn find_recent_streams(
    conn: &impl ConnectionTrait,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(
            Condition::any()
                .add(stream::Column::Status.eq(stream::StreamStatus::Live))
                .add(
                    Condition::all()
                        .add(stream::Column::Status.eq(stream::StreamStatus::Ended))
                        .add(stream::Column::EndedAt.gte(since)),
                ),
        )
        .order_by_desc(stream::Column::CreatedAt)
        .all(conn)
        .await
        .map_err(RepoError::from)
}
