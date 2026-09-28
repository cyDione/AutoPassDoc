use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("无法读取文件: {0}")]
    Io(#[from] std::io::Error),
    #[error("不是有效的 .docx 文件（zip 结构错误）: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("XML 解析失败（{part}）: {message}")]
    Xml { part: String, message: String },
    #[error("文档缺少必要部件: {0}")]
    MissingPart(String),
    #[error("{0}")]
    Invalid(String),
    /// An edit that cannot be written back safely; the document is unchanged.
    #[error("{0}")]
    Edit(String),
}

impl Error {
    pub(crate) fn xml(part: &str, err: impl std::fmt::Display) -> Self {
        Error::Xml {
            part: part.to_string(),
            message: err.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
