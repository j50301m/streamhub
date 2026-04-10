use entity::recording;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder,
};
use uuid::Uuid;

use crate::RepoError;

pub async fn create(
    conn: &impl ConnectionTrait,
    model: recording::ActiveModel,
) -> Result<recording::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}

pub struct PaginatedResult {
    pub items: Vec<recording::Model>,
    pub total: u64,
}

pub async fn list_by_stream(
    conn: &impl ConnectionTrait,
    stream_id: Uuid,
    page: u64,
    per_page: u64,
) -> Result<PaginatedResult, RepoError> {
    let query = recording::Entity::find()
        .filter(recording::Column::StreamId.eq(stream_id))
        .order_by_desc(recording::Column::CreatedAt);

    let total = query.clone().count(conn).await?;

    let items = query.paginate(conn, per_page).fetch_page(page - 1).await?;

    Ok(PaginatedResult { items, total })
}

pub async fn find_latest_by_stream(
    conn: &impl ConnectionTrait,
    stream_id: Uuid,
) -> Result<Option<recording::Model>, RepoError> {
    recording::Entity::find()
        .filter(recording::Column::StreamId.eq(stream_id))
        .order_by_desc(recording::Column::CreatedAt)
        .one(conn)
        .await
        .map_err(RepoError::from)
}
