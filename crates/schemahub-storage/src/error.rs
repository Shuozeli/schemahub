use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("redb error: {0}")]
    Redb(#[from] redb::Error),
    #[error("redb database error: {0}")]
    RedbDatabase(#[from] redb::DatabaseError),
    #[error("redb transaction error: {0}")]
    RedbTransaction(#[from] redb::TransactionError),
    #[error("redb table error: {0}")]
    RedbTable(#[from] redb::TableError),
    #[error("redb storage error: {0}")]
    RedbStorage(#[from] redb::StorageError),
    #[error("redb commit error: {0}")]
    RedbCommit(#[from] redb::CommitError),
    #[error("key encoding error: {0}")]
    KeyEncoding(String),
    #[error("value encoding error: {0}")]
    ValueEncoding(String),
}
