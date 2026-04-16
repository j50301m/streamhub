//! User entity queries. Callers usually go through
//! [`crate::traits::UserRepoRef`].

use chrono::{DateTime, Utc};
use entity::user;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};
use uuid::Uuid;

use crate::RepoError;

/// Finds a user by UUID primary key.
pub async fn find_by_id(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<Option<user::Model>, RepoError> {
    user::Entity::find_by_id(id)
        .one(conn)
        .await
        .map_err(RepoError::from)
}

/// Finds a user by email.
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

/// Finds a user by email taking a row-level exclusive (`FOR UPDATE`) lock.
/// Must be called inside a transaction.
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

/// Counts all users.
pub async fn count_all(conn: &impl ConnectionTrait) -> Result<u64, RepoError> {
    user::Entity::find()
        .count(conn)
        .await
        .map_err(RepoError::from)
}

/// Counts users with the given role.
pub async fn count_by_role(
    conn: &impl ConnectionTrait,
    role: user::UserRole,
) -> Result<u64, RepoError> {
    user::Entity::find()
        .filter(user::Column::Role.eq(role))
        .count(conn)
        .await
        .map_err(RepoError::from)
}

/// One page of users along with the total matching row count.
pub struct UserPaginatedResult {
    /// Rows for the requested page.
    pub items: Vec<user::Model>,
    /// Total rows matching the filter (not just this page).
    pub total: u64,
}

/// Lists users with optional search / filter / pagination.
/// `page` is 1-indexed; `per_page` sets the page size.
pub async fn find_users_paginated(
    conn: &impl ConnectionTrait,
    page: u64,
    per_page: u64,
    q: Option<&str>,
    role: Option<user::UserRole>,
    suspended: Option<bool>,
) -> Result<UserPaginatedResult, RepoError> {
    let mut query = user::Entity::find();

    if let Some(search) = q {
        if !search.is_empty() {
            let pattern = format!("%{search}%");
            query = query.filter(user::Column::Email.like(&pattern));
        }
    }

    if let Some(role) = role {
        query = query.filter(user::Column::Role.eq(role));
    }

    if let Some(is_suspended) = suspended {
        query = query.filter(user::Column::IsSuspended.eq(is_suspended));
    }

    let total = query.clone().count(conn).await?;

    let items = query
        .order_by_desc(user::Column::CreatedAt)
        .paginate(conn, per_page)
        .fetch_page(page.saturating_sub(1))
        .await?;

    Ok(UserPaginatedResult { items, total })
}

/// Updates a user's role by id. Returns the updated model.
pub async fn update_role(
    conn: &impl ConnectionTrait,
    id: Uuid,
    role: user::UserRole,
) -> Result<user::Model, RepoError> {
    let model = user::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or(RepoError::NotFound)?;

    let mut active: user::ActiveModel = model.into();
    active.role = Set(role);
    active.update(conn).await.map_err(RepoError::from)
}

/// Sets a user as suspended. Returns the updated model.
pub async fn set_suspended(
    conn: &impl ConnectionTrait,
    id: Uuid,
    until: Option<DateTime<Utc>>,
    reason: Option<String>,
) -> Result<user::Model, RepoError> {
    let model = user::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or(RepoError::NotFound)?;

    let mut active: user::ActiveModel = model.into();
    active.is_suspended = Set(true);
    active.suspended_until = Set(until);
    active.suspension_reason = Set(reason);
    active.update(conn).await.map_err(RepoError::from)
}

/// Clears suspension fields. Returns the updated model.
pub async fn clear_suspended(
    conn: &impl ConnectionTrait,
    id: Uuid,
) -> Result<user::Model, RepoError> {
    let model = user::Entity::find_by_id(id)
        .one(conn)
        .await?
        .ok_or(RepoError::NotFound)?;

    let mut active: user::ActiveModel = model.into();
    active.is_suspended = Set(false);
    active.suspended_until = Set(None);
    active.suspension_reason = Set(None);
    active.update(conn).await.map_err(RepoError::from)
}

/// Inserts a new user row and returns the persisted model.
pub async fn create(
    conn: &impl ConnectionTrait,
    model: user::ActiveModel,
) -> Result<user::Model, RepoError> {
    model.insert(conn).await.map_err(RepoError::from)
}
