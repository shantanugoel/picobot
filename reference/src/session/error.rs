#[derive(Debug, thiserror::Error)]
pub enum SessionDbError {
    #[error("Database open failed: {0}")]
    OpenFailed(String),
    #[error("Database migration failed: {0}")]
    MigrationFailed(String),
    #[error("Database query failed: {0}")]
    QueryFailed(String),
    #[error("Database busy")]
    Busy,
}

pub type SessionDbResult<T> = Result<T, SessionDbError>;
