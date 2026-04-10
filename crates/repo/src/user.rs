use entity::user;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

use crate::RepoError;

pub async fn find_by_id(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<user::Model>, RepoError> {
    user::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(RepoError::from)
}

pub async fn find_by_email(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<user::Model>, RepoError> {
    user::Entity::find()
        .filter(user::Column::Email.eq(email))
        .one(conn)
        .await
        .map_err(RepoError::from)
}

pub async fn find_by_email_for_update(
    conn: &impl ConnectionTrait,
    email: &str,
) -> Result<Option<user::Model>, RepoError> {
    user::Entity::find()
        .filter(user::Column::Email.eq(email))
        .lock_exclusive()
        .one(conn)
        .await
        .map_err(RepoError::from)
}

pub async fn create(
    conn: &impl ConnectionTrait,
    model: user::ActiveModel,
) -> Result<user::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}
