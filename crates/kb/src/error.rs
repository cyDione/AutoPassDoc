use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Knowledge base errors. Messages are shown to the user as is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("不支持的格式：{0}（支持 .docx、.pdf、.txt、.md）")]
    UnsupportedFormat(String),
    #[error("无法读取文件 {path}：{source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("无法解析文档：{0}")]
    Parse(String),
    #[error("知识库数据库出错：{0}")]
    Db(#[from] rusqlite::Error),
    #[error("知识库中没有编号为 {0} 的文档")]
    DocumentNotFound(i64),
    #[error("向量维度不一致：模型 {model} 的向量是 {expected} 维，收到的是 {actual} 维")]
    DimensionMismatch {
        model: String,
        expected: usize,
        actual: usize,
    },
    #[error("向量为空")]
    EmptyVector,
    #[error("知识库由更新版本的软件创建（结构版本 {0}），请升级软件后再打开")]
    NewerSchema(i64),
}
