//! Database repositories and Unit-of-Work transaction wrapper.
//!
//! Handlers never touch SeaORM directly; they go through [`UnitOfWork`] for
//! reads and [`TransactionContext`] for writes. Per-entity query functions
//! live in [`stream`], [`user`], and [`recording`]; the typed repo wrappers
//! in [`traits`] bind a `&DatabaseConnection` or `&DatabaseTransaction` so
//! the same call sites work inside and outside transactions.
#![warn(missing_docs)]

pub mod recording;
pub mod stream;
pub mod traits;
pub mod user;

use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};

use traits::{RecordingRepoRef, StreamRepoRef, UserRepoRef};

/// Errors returned by the repository layer.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    /// Raw database error from SeaORM.
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    /// `commit()` or `rollback()` was called twice on the same
    /// [`TransactionContext`].
    #[error("transaction already consumed")]
    TransactionConsumed,
    /// Expected row was absent.
    #[error("not found")]
    NotFound,
}

/// Non-transactional database access for read operations; also the factory
/// for [`TransactionContext`].
#[derive(Debug, Clone)]
pub struct UnitOfWork {
    db: DatabaseConnection,
}

impl UnitOfWork {
    /// Wraps an existing SeaORM connection.
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// Begins a new database transaction.
    ///
    /// # Errors
    /// Returns [`RepoError::Db`] if the transaction cannot be started.
    pub async fn begin(&self) -> Result<TransactionContext, RepoError> {
        let txn = self.db.begin().await?;
        Ok(TransactionContext { txn: Some(txn) })
    }

    /// Returns the underlying connection for ad-hoc queries.
    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Returns a stream repository bound to the non-transactional connection.
    pub fn stream_repo(&self) -> StreamRepoRef<'_, DatabaseConnection> {
        StreamRepoRef(&self.db)
    }

    /// Returns a user repository bound to the non-transactional connection.
    pub fn user_repo(&self) -> UserRepoRef<'_, DatabaseConnection> {
        UserRepoRef(&self.db)
    }

    /// Returns a recording repository bound to the non-transactional
    /// connection.
    pub fn recording_repo(&self) -> RecordingRepoRef<'_, DatabaseConnection> {
        RecordingRepoRef(&self.db)
    }
}

/// Scope for a single database transaction. Consume exactly once with
/// [`Self::commit`] or [`Self::rollback`].
pub struct TransactionContext {
    txn: Option<DatabaseTransaction>,
}

impl TransactionContext {
    /// Returns the underlying transaction handle.
    ///
    /// # Panics
    /// Panics if the transaction has already been committed or rolled back.
    pub fn txn(&self) -> &DatabaseTransaction {
        self.txn.as_ref().expect("transaction not consumed")
    }

    /// Commits the transaction.
    ///
    /// # Errors
    /// Returns [`RepoError::TransactionConsumed`] if the transaction was
    /// already consumed, or [`RepoError::Db`] if the commit itself fails.
    pub async fn commit(mut self) -> Result<(), RepoError> {
        let txn = self.txn.take().ok_or(RepoError::TransactionConsumed)?;
        txn.commit().await.map_err(RepoError::from)
    }

    /// Rolls back the transaction.
    ///
    /// # Errors
    /// Returns [`RepoError::TransactionConsumed`] if the transaction was
    /// already consumed, or [`RepoError::Db`] if the rollback itself fails.
    pub async fn rollback(mut self) -> Result<(), RepoError> {
        let txn = self.txn.take().ok_or(RepoError::TransactionConsumed)?;
        txn.rollback().await.map_err(RepoError::from)
    }

    /// Returns a stream repository bound to this transaction.
    pub fn stream_repo(&self) -> StreamRepoRef<'_, DatabaseTransaction> {
        StreamRepoRef(self.txn())
    }

    /// Returns a user repository bound to this transaction.
    pub fn user_repo(&self) -> UserRepoRef<'_, DatabaseTransaction> {
        UserRepoRef(self.txn())
    }

    /// Returns a recording repository bound to this transaction.
    pub fn recording_repo(&self) -> RecordingRepoRef<'_, DatabaseTransaction> {
        RecordingRepoRef(self.txn())
    }
}
