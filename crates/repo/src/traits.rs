use entity::{recording, stream, user};
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::RepoError;

// ── Typed repo wrappers ──

pub struct StreamRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);
pub struct UserRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);
pub struct RecordingRepoRef<'a, C: ConnectionTrait>(pub(crate) &'a C);

// ── StreamRepo ──

impl<C: ConnectionTrait> StreamRepoRef<'_, C> {
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_id(self.0, id).await
    }

    pub async fn find_by_key(&self, key: &str) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_key(self.0, key).await
    }

    pub async fn find_by_key_for_update(
        &self,
        key: &str,
    ) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_key_for_update(self.0, key).await
    }

    pub async fn find_by_id_for_update(
        &self,
        id: Uuid,
    ) -> Result<Option<stream::Model>, RepoError> {
        crate::stream::find_by_id_for_update(self.0, id).await
    }

    pub async fn list_live(&self) -> Result<Vec<stream::Model>, RepoError> {
        crate::stream::list_live(self.0).await
    }

    pub async fn list_vod(&self) -> Result<Vec<stream::Model>, RepoError> {
        crate::stream::list_vod(self.0).await
    }

    pub async fn list_by_user(
        &self,
        user_id: Uuid,
        status_filter: Option<stream::StreamStatus>,
        page: u64,
        per_page: u64,
    ) -> Result<crate::stream::PaginatedResult, RepoError> {
        crate::stream::list_by_user(self.0, user_id, status_filter, page, per_page).await
    }

    pub async fn create(&self, model: stream::ActiveModel) -> Result<stream::Model, RepoError> {
        crate::stream::create(self.0, model).await
    }

    pub async fn update(&self, model: stream::ActiveModel) -> Result<stream::Model, RepoError> {
        crate::stream::update(self.0, model).await
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), RepoError> {
        crate::stream::delete(self.0, id).await
    }
}

// ── UserRepo ──

impl<C: ConnectionTrait> UserRepoRef<'_, C> {
    pub async fn find_by_id(&self, id: Uuid) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_id(self.0, id).await
    }

    pub async fn find_by_email(&self, email: &str) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_email(self.0, email).await
    }

    pub async fn find_by_email_for_update(
        &self,
        email: &str,
    ) -> Result<Option<user::Model>, RepoError> {
        crate::user::find_by_email_for_update(self.0, email).await
    }

    pub async fn create(&self, model: user::ActiveModel) -> Result<user::Model, RepoError> {
        crate::user::create(self.0, model).await
    }
}

// ── RecordingRepo ──

impl<C: ConnectionTrait> RecordingRepoRef<'_, C> {
    pub async fn create(
        &self,
        model: recording::ActiveModel,
    ) -> Result<recording::Model, RepoError> {
        crate::recording::create(self.0, model).await
    }

    pub async fn list_by_stream(
        &self,
        stream_id: Uuid,
        page: u64,
        per_page: u64,
    ) -> Result<crate::recording::PaginatedResult, RepoError> {
        crate::recording::list_by_stream(self.0, stream_id, page, per_page).await
    }

    pub async fn find_latest_by_stream(
        &self,
        stream_id: Uuid,
    ) -> Result<Option<recording::Model>, RepoError> {
        crate::recording::find_latest_by_stream(self.0, stream_id).await
    }
}
