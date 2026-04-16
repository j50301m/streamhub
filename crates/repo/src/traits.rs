//! Typed repository wrappers that bind a `ConnectionTrait` (either a
//! [`sea_orm::DatabaseConnection`] or a [`sea_orm::DatabaseTransaction`]) to
//! a set of entity-specific operations.

use entity::{recording, stream, user};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::RepoError;

/// Stream repository bound to a connection or transaction.
pub struct StreamRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);
/// User repository bound to a connection or transaction.
pub struct UserRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);
/// Recording repository bound to a connection or transaction.
pub struct RecordingRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);

impl<C: ConnectionTrait> StreamRepoRef<'_, C> {
    /// Finds a stream by its UUID primary key.
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_id(self.0, id).await
    }

    /// Finds a stream by its public `stream_key`.
    pub async fn find_by_key(&self, key: &str) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_key(self.0, key).await
    }

    /// Finds a stream by `stream_key` taking a row-level exclusive lock.
    /// Must be called inside a transaction.
    pub async fn find_by_key_for_update(
        &self,
        key: &str,
    ) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_key_for_update(self.0, key).await
    }

    /// Finds a stream by id taking a row-level exclusive lock. Must be
    /// called inside a transaction.
    pub async fn find_by_id_for_update(
        &self,
        id: Uuid,
    ) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_id_for_update(self.0, id).await
    }

    /// Lists all streams currently in [`stream::StreamStatus::Live`],
    /// newest first.
    pub async fn list_live(&self) -> Result<Vec<stream::Model>, RepoError> {
        crate::stream::list_live(self.0).await
    }

    /// Lists ended streams with a ready VOD, newest first.
    pub async fn list_vod(&self) -> Result<Vec<stream::Model>, RepoError> {
        crate::stream::list_vod(self.0).await
    }

    /// Lists streams owned by `user_id`, optionally filtered by status,
    /// paginated 1-indexed by `page` with `per_page` rows per page.
    pub async fn list_by_user(
        &self,
        user_id: Uuid,
        status_filter: Option<stream::StreamStatus>,
        page: u64,
        per_page: u64,
    ) -> Result<crate::stream::PaginatedResult, RepoError> {
        crate::stream::list_by_user(self.0, user_id, status_filter, page, per_page).await
    }

    /// Inserts a new stream row.
    pub async fn create(&self, model: stream::ActiveModel) -> Result<stream::Model, RepoError> {
        crate::stream::create(self.0, model).await
    }

    /// Updates an existing stream row.
    pub async fn update(&self, model: stream::ActiveModel) -> Result<stream::Model, RepoError> {
        crate::stream::update(self.0, model).await
    }

    /// Deletes a stream by id.
    pub async fn delete(&self, id: Uuid) -> Result<(), RepoError> {
        crate::stream::delete(self.0, id).await
    }

    /// Counts streams with the given status.
    pub async fn count_by_status(&self, status: stream::StreamStatus) -> Result<u64, RepoError> {
        crate::stream::count_by_status(self.0, status).await
    }

    /// Counts streams that ended on or after the given timestamp.
    pub async fn count_ended_since(
        &self,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, RepoError> {
        crate::stream::count_ended_since(self.0, since).await
    }

    /// Returns live streams ordered by `started_at` descending, up to `limit`.
    pub async fn list_live_limited(&self, limit: u64) -> Result<Vec<stream::Model>, RepoError> {
        crate::stream::list_live_limited(self.0, limit).await
    }
}

impl<C: ConnectionTrait> UserRepoRef<'_, C> {
    /// Finds a user by UUID primary key.
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_id(self.0, id).await
    }

    /// Finds a user by email.
    pub async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_email(self.0, email).await
    }

    /// Finds a user by email taking a row-level exclusive lock. Must be
    /// called inside a transaction.
    pub async fn find_by_email_for_update(
        &self,
        email: &str,
    ) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_email_for_update(self.0, email).await
    }

    /// Inserts a new user row.
    pub async fn create(&self, model: user::ActiveModel) -> Result<user::Model, RepoError> {
        crate::user::create(self.0, model).await
    }

    /// Counts all users.
    pub async fn count_all(&self) -> Result<u64, RepoError> {
        crate::user::count_all(self.0).await
    }

    /// Counts users with the given role.
    pub async fn count_by_role(&self, role: user::UserRole) -> Result<u64, RepoError> {
        crate::user::count_by_role(self.0, role).await
    }
}

impl<C: ConnectionTrait> RecordingRepoRef<'_, C> {
    /// Inserts a new recording row.
    pub async fn create(
        &self,
        model: recording::ActiveModel,
    ) -> Result<recording::Model, RepoError> {
        crate::recording::create(self.0, model).await
    }

    /// Lists recordings for `stream_id`, newest first, 1-indexed pagination.
    pub async fn list_by_stream(
        &self,
        stream_id: Uuid,
        page: u64,
        per_page: u64,
    ) -> Result<crate::recording::PaginatedResult, RepoError> {
        crate::recording::list_by_stream(self.0, stream_id, page, per_page).await
    }

    /// Returns the most recent recording for `stream_id`, if any.
    pub async fn find_latest_by_stream(
        &self,
        stream_id: Uuid,
    ) -> Result<Option<recording::Model>, RepoError> {
        crate::recording::find_latest_by_stream(self.0, stream_id).await
    }
}
