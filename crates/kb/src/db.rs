//! SQLite schema and migrations (tracked in `PRAGMA user_version`).

use rusqlite::Connection;

use crate::{Error, Result};

/// Separator of heading path elements in the `heading_path` columns.
pub(crate) const PATH_SEP: char = '\u{1F}';

/// Each entry upgrades the schema by one version.
const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    file_name TEXT NOT NULL,
    original_path TEXT NOT NULL,
    -- Relative to the knowledge base folder.
    stored_name TEXT NOT NULL,
    format TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    title TEXT,
    doc_number TEXT,
    issuer TEXT,
    date TEXT,
    -- 1 once the user corrected the metadata; re-imports keep it then.
    meta_edited INTEGER NOT NULL DEFAULT 0,
    char_count INTEGER NOT NULL,
    -- Lines joined with "\n"; chunk offsets are char offsets into it.
    full_text TEXT NOT NULL,
    -- Newline-separated.
    warnings TEXT NOT NULL DEFAULT '',
    imported_at INTEGER NOT NULL
);
CREATE INDEX documents_sha256 ON documents(sha256);
CREATE INDEX documents_original_path ON documents(original_path);

CREATE TABLE parents (
    id INTEGER PRIMARY KEY,
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    char_start INTEGER NOT NULL,
    char_end INTEGER NOT NULL,
    text TEXT NOT NULL
);
CREATE INDEX parents_doc ON parents(doc_id);

CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    parent_id INTEGER NOT NULL REFERENCES parents(id) ON DELETE CASCADE,
    heading_path TEXT NOT NULL,
    char_start INTEGER NOT NULL,
    char_end INTEGER NOT NULL,
    text TEXT NOT NULL
);
CREATE INDEX chunks_doc ON chunks(doc_id);
CREATE INDEX chunks_parent ON chunks(parent_id);

CREATE TABLE models (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    dim INTEGER NOT NULL
);

-- Little-endian f32 vectors as given by the caller.
CREATE TABLE embeddings (
    chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    model_id INTEGER NOT NULL REFERENCES models(id) ON DELETE CASCADE,
    vector BLOB NOT NULL,
    PRIMARY KEY (chunk_id, model_id)
);
CREATE INDEX embeddings_model ON embeddings(model_id);

-- rowid = chunks.id. Columns hold space-separated jieba tokens of the chunk
-- text, its heading path, and its document's title, 文号 and issuer.
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    body, head, doc,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#,
    // v2: which parser produced the text (`builtin`, `mineru`, `paddleocr`).
    r#"
ALTER TABLE documents ADD COLUMN parser TEXT NOT NULL DEFAULT 'builtin';
"#,
];

pub(crate) fn open(path: &std::path::Path) -> Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let latest = MIGRATIONS.len() as i64;
    if version > latest {
        return Err(Error::NewerSchema(version));
    }
    if version == latest {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for sql in &MIGRATIONS[version as usize..] {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", latest)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn join_path(path: &[String]) -> String {
    let mut s = String::new();
    for (i, p) in path.iter().enumerate() {
        if i > 0 {
            s.push(PATH_SEP);
        }
        s.push_str(p);
    }
    s
}

pub(crate) fn split_path(s: &str) -> Vec<String> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split(PATH_SEP).map(str::to_string).collect()
    }
}

pub(crate) fn vector_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(crate) fn blob_to_vector(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}
