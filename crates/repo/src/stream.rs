//! Stream entity queries. Callers usually go through [`crate::traits::StreamRepoRef`].

use entity::stream;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::RepoError;

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

/// Deletes a stream by UUID. Succeeds silently if the row is absent.
pub async fn delete(conn: &impl ConnectionTrait, id: Uuid) -> Result<(), RepoError> {
    stream::Entity::delete_by_id(id)
        .exec(conn)
        .await
        .map(|_| ())
        .map_err(RepoError::from)
}
