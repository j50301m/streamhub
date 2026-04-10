pub mod recording;
pub mod stream;
pub mod stream_token;
pub mod traits;
pub mod user;

use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};

use traits::{RecordingRepoRef, StreamRepoRef, StreamTokenRepoRef, UserRepoRef};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("transaction already consumed")]
    TransactionConsumed,
    #[error("not found")]
    NotFound,
}

/// Non-transactional database access for read operations.
/// Also serves as the factory for TransactionContext.
#[derive(Debug, Clone)]
pub struct UnitOfWork {
    db: DatabaseConnection,
}

impl UnitOfWork {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn begin(&self) -> Result<TransactionContext, RepoError> {
        let txn = self.db.begin().await?;
        Ok(TransactionContext { txn: Some(txn) })
    }

    /// Access the underlying connection for repo queries.
    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    pub fn stream_repo(&self) -> StreamRepoRef<'_, DatabaseConnection> {
        StreamRepoRef(&self.db)
    }

    pub fn user_repo(&self) -> UserRepoRef<'_, DatabaseConnection> {
        UserRepoRef(&self.db)
    }

    pub fn recording_repo(&self) -> RecordingRepoRef<'_, DatabaseConnection> {
        RecordingRepoRef(&self.db)
    }

    pub fn stream_token_repo(&self) -> StreamTokenRepoRef<'_, DatabaseConnection> {
        StreamTokenRepoRef(&self.db)
    }
}

/// Transactional database access for write operations.
pub struct TransactionContext {
    txn: Option<DatabaseTransaction>,
}

impl TransactionContext {
    /// Access the underlying transaction for repo queries.
    /// Panics if the transaction has already been consumed.
    pub fn txn(&self) -> &DatabaseTransaction {
        self.txn.as_ref().expect("transaction not consumed")
    }

    pub async fn commit(mut self) -> Result<(), RepoError> {
        let txn = self.txn.take().ok_or(RepoError::TransactionConsumed)?;
        txn.commit().await.map_err(RepoError::from)
    }

    pub async fn rollback(mut self) -> Result<(), RepoError> {
        let txn = self.txn.take().ok_or(RepoError::TransactionConsumed)?;
        txn.rollback().await.map_err(RepoError::from)
    }

    pub fn stream_repo(&self) -> StreamRepoRef<'_, DatabaseTransaction> {
        StreamRepoRef(self.txn())
    }

    pub fn user_repo(&self) -> UserRepoRef<'_, DatabaseTransaction> {
        UserRepoRef(self.txn())
    }

    pub fn recording_repo(&self) -> RecordingRepoRef<'_, DatabaseTransaction> {
        RecordingRepoRef(self.txn())
    }

    pub fn stream_token_repo(&self) -> StreamTokenRepoRef<'_, DatabaseTransaction> {
        StreamTokenRepoRef(self.txn())
    }
}
