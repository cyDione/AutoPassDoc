use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Document(#[from] docx_engine::Error),
    #[error("数据库错误: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("文件读写失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("数据格式错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Storage(String),
    /// Something the user must set up first, e.g. a model for a role.
    #[error("{0}")]
    Setup(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
