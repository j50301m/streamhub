use entity::stream;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::RepoError;

pub async fn find_by_id(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<stream::Model>, RepoError> {
    stream::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(RepoError::from)
}

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

pub async fn list_live(conn: &impl ConnectionTrait) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Live))
        .order_by_desc(stream::Column::StartedAt)
        .all(conn)
        .await
        .map_err(RepoError::from)
}

pub async fn list_vod(conn: &impl ConnectionTrait) -> Result<Vec<stream::Model>, RepoError> {
    stream::Entity::find()
        .filter(stream::Column::Status.eq(stream::StreamStatus::Ended))
        .filter(stream::Column::VodStatus.eq(stream::VodStatus::Ready))
        .order_by_desc(stream::Column::EndedAt)
        .all(conn)
        .await
        .map_err(RepoError::from)
}

pub struct PaginatedResult {
    pub items: Vec<stream::Model>,
    pub total: u64,
}

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

pub async fn create(
    conn: &impl ConnectionTrait,
    model: stream::ActiveModel,
) -> Result<stream::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}

pub async fn update(
    conn: &impl ConnectionTrait,
    model: stream::ActiveModel,
) -> Result<stream::Model, RepoError> {
    model.update(conn).await.map_err(RepoError::from)
}

pub async fn delete(conn: &impl ConnectionTrait, id: Uuid) -> Result<(), RepoError> {
    stream::Entity::delete_by_id(id)
        .exec(conn)
        .await
        .map(|_| ())
        .map_err(RepoError::from)
}
