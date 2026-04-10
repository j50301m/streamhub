use entity::stream_token;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::RepoError;

pub async fn create(
    conn: &impl ConnectionTrait,
    model: stream_token::ActiveModel,
) -> Result<stream_token::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}

pub async fn find_by_stream_and_hash(
    conn: &impl ConnectionTrait,
    stream_id: Uuid,
    token_hash: &str,
) -> Result<Option<stream_token::Model>, RepoError> {
    stream_token::Entity::find()
        .filter(stream_token::Column::StreamId.eq(stream_id))
        .filter(stream_token::Column::TokenHash.eq(token_hash))
        .one(conn)
        .await
        .map_err(RepoError::from)
}
