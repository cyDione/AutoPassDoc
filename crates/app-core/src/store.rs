//! `app.db`: settings, model providers, reviewers and their author aliases,
//! fix cases (the feedback that reviewer profiles and judge calibration
//! learn from) and reviewer profiles.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::settings::Settings;

const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    base_url TEXT NOT NULL,
    decision_path TEXT,
    rerank_path TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE provider_models (
    provider_id TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    info TEXT,
    manual TEXT,
    fetched_at INTEGER,
    PRIMARY KEY (provider_id, model_id)
);
CREATE TABLE reviewers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    note TEXT NOT NULL DEFAULT '',
    threshold REAL,
    created_at INTEGER NOT NULL
);
CREATE TABLE reviewer_aliases (
    author TEXT NOT NULL,
    initials TEXT NOT NULL DEFAULT '',
    doc_key TEXT NOT NULL DEFAULT '',
    reviewer_id INTEGER NOT NULL REFERENCES reviewers(id) ON DELETE CASCADE,
    PRIMARY KEY (author, initials, doc_key)
);
CREATE TABLE cases (
    id INTEGER PRIMARY KEY,
    reviewer_id INTEGER REFERENCES reviewers(id) ON DELETE SET NULL,
    doc_key TEXT NOT NULL,
    doc_name TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    author TEXT NOT NULL,
    comment TEXT NOT NULL,
    original TEXT NOT NULL,
    suggestion TEXT NOT NULL,
    final_text TEXT,
    action TEXT NOT NULL DEFAULT 'pending',
    confidence REAL,
    judge TEXT,
    category TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX cases_reviewer ON cases(reviewer_id, id);
CREATE TABLE profiles (
    id INTEGER PRIMARY KEY,
    reviewer_id INTEGER NOT NULL REFERENCES reviewers(id) ON DELETE CASCADE,
    version INTEGER NOT NULL,
    summary TEXT NOT NULL,
    case_count INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
"#,
    r#"
CREATE TABLE proofread_cache (
    doc_key TEXT NOT NULL,
    para_hash TEXT NOT NULL,
    checks TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (doc_key, para_hash, checks)
);
"#,
];

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    /// "openai" | "openrouter" | "anthropic" | "ollama"
    pub kind: String,
    pub base_url: String,
    #[serde(default)]
    pub decision_path: Option<String>,
    #[serde(default)]
    pub rerank_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    pub id: i64,
    pub name: String,
    pub note: String,
    /// Overrides the global confidence threshold for this reviewer.
    pub threshold: Option<f32>,
    pub case_count: usize,
    pub profile_version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alias {
    pub author: String,
    pub initials: String,
    /// Empty for a global alias; otherwise the document it applies to.
    pub doc_key: String,
    pub reviewer_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseAction {
    Pending,
    Accepted,
    Edited,
    Rejected,
}

impl CaseAction {
    fn as_str(self) -> &'static str {
        match self {
            CaseAction::Pending => "pending",
            CaseAction::Accepted => "accepted",
            CaseAction::Edited => "edited",
            CaseAction::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "accepted" => CaseAction::Accepted,
            "edited" => CaseAction::Edited,
            "rejected" => CaseAction::Rejected,
            _ => CaseAction::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    pub id: i64,
    pub reviewer_id: Option<i64>,
    pub doc_key: String,
    pub doc_name: String,
    pub comment_id: String,
    pub author: String,
    pub comment: String,
    pub original: String,
    pub suggestion: String,
    pub final_text: Option<String>,
    pub action: CaseAction,
    pub confidence: Option<f32>,
    /// Judge answers as JSON.
    pub judge: Option<String>,
    pub category: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub reviewer_id: i64,
    pub version: i64,
    /// Distilled profile as JSON (see `profiles::ReviewerProfile`).
    pub summary: String,
    pub case_count: usize,
    pub created_at: i64,
}

/// A cached model: (model id, fetched info JSON, manual profile JSON).
pub type CachedModel = (String, Option<String>, Option<String>);

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(Self { conn })
    }

    // Settings

    pub fn settings(&self) -> Result<Settings> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'settings'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(value
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    // Providers

    pub fn providers(&self) -> Result<Vec<ProviderRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, base_url, decision_path, rerank_path FROM providers ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ProviderRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                base_url: r.get(3)?,
                decision_path: r.get(4)?,
                rerank_path: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn provider(&self, id: &str) -> Result<Option<ProviderRecord>> {
        Ok(self.providers()?.into_iter().find(|p| p.id == id))
    }

    pub fn upsert_provider(&self, p: &ProviderRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO providers (id, name, kind, base_url, decision_path, rerank_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, kind = excluded.kind,
               base_url = excluded.base_url, decision_path = excluded.decision_path,
               rerank_path = excluded.rerank_path",
            params![
                p.id,
                p.name,
                p.kind,
                p.base_url,
                p.decision_path,
                p.rerank_path,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM providers WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Cached model list of a provider.
    pub fn provider_models(&self, provider_id: &str) -> Result<Vec<CachedModel>> {
        let mut stmt = self.conn.prepare(
            "SELECT model_id, info, manual FROM provider_models WHERE provider_id = ?1 ORDER BY model_id",
        )?;
        let rows = stmt.query_map([provider_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces the fetched model list, keeping manual profiles.
    pub fn replace_provider_models(
        &self,
        provider_id: &str,
        models: &[(String, String)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM provider_models WHERE provider_id = ?1 AND manual IS NULL",
            [provider_id],
        )?;
        tx.execute(
            "UPDATE provider_models SET info = NULL WHERE provider_id = ?1",
            [provider_id],
        )?;
        for (id, info) in models {
            tx.execute(
                "INSERT INTO provider_models (provider_id, model_id, info, fetched_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(provider_id, model_id) DO UPDATE SET info = excluded.info, fetched_at = excluded.fetched_at",
                params![provider_id, id, info, now()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_manual_profile(
        &self,
        provider_id: &str,
        model_id: &str,
        manual: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO provider_models (provider_id, model_id, manual) VALUES (?1, ?2, ?3)
             ON CONFLICT(provider_id, model_id) DO UPDATE SET manual = excluded.manual",
            params![provider_id, model_id, manual],
        )?;
        Ok(())
    }

    // Reviewers

    pub fn reviewers(&self) -> Result<Vec<Reviewer>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.name, r.note, r.threshold,
                    (SELECT COUNT(*) FROM cases c WHERE c.reviewer_id = r.id AND c.action != 'pending'),
                    (SELECT MAX(version) FROM profiles p WHERE p.reviewer_id = r.id)
             FROM reviewers r ORDER BY r.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Reviewer {
                id: r.get(0)?,
                name: r.get(1)?,
                note: r.get(2)?,
                threshold: r.get(3)?,
                case_count: r.get::<_, i64>(4)? as usize,
                profile_version: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn reviewer(&self, id: i64) -> Result<Option<Reviewer>> {
        Ok(self.reviewers()?.into_iter().find(|r| r.id == id))
    }

    pub fn reviewer_by_name(&self, name: &str) -> Result<Option<Reviewer>> {
        Ok(self.reviewers()?.into_iter().find(|r| r.name == name))
    }

    pub fn create_reviewer(&self, name: &str, note: &str) -> Result<Reviewer> {
        self.conn.execute(
            "INSERT INTO reviewers (name, note, created_at) VALUES (?1, ?2, ?3)",
            params![name.trim(), note, now()],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.reviewer(id)?.expect("just inserted"))
    }

    pub fn update_reviewer(
        &self,
        id: i64,
        name: &str,
        note: &str,
        threshold: Option<f32>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE reviewers SET name = ?2, note = ?3, threshold = ?4 WHERE id = ?1",
            params![id, name.trim(), note, threshold],
        )?;
        Ok(())
    }

    pub fn delete_reviewer(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM reviewers WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Moves aliases, cases and profiles of `from` to `into`, then deletes `from`.
    pub fn merge_reviewers(&self, from: i64, into: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE OR REPLACE reviewer_aliases SET reviewer_id = ?2 WHERE reviewer_id = ?1",
            [from, into],
        )?;
        tx.execute(
            "UPDATE cases SET reviewer_id = ?2 WHERE reviewer_id = ?1",
            [from, into],
        )?;
        tx.execute("DELETE FROM reviewers WHERE id = ?1", [from])?;
        tx.commit()?;
        Ok(())
    }

    pub fn aliases(&self) -> Result<Vec<Alias>> {
        let mut stmt = self.conn.prepare(
            "SELECT author, initials, doc_key, reviewer_id FROM reviewer_aliases ORDER BY author",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Alias {
                author: r.get(0)?,
                initials: r.get(1)?,
                doc_key: r.get(2)?,
                reviewer_id: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Maps a comment author to a reviewer, or removes the mapping (`None`).
    pub fn set_alias(
        &self,
        author: &str,
        initials: &str,
        doc_key: &str,
        reviewer_id: Option<i64>,
    ) -> Result<()> {
        match reviewer_id {
            Some(id) => self.conn.execute(
                "INSERT INTO reviewer_aliases (author, initials, doc_key, reviewer_id) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(author, initials, doc_key) DO UPDATE SET reviewer_id = excluded.reviewer_id",
                params![author, initials, doc_key, id],
            )?,
            None => self.conn.execute(
                "DELETE FROM reviewer_aliases WHERE author = ?1 AND initials = ?2 AND doc_key = ?3",
                params![author, initials, doc_key],
            )?,
        };
        Ok(())
    }

    // Cases

    pub fn add_case(&self, c: &Case) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO cases (reviewer_id, doc_key, doc_name, comment_id, author, comment, original, suggestion,
                                final_text, action, confidence, judge, category, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
            params![
                c.reviewer_id,
                c.doc_key,
                c.doc_name,
                c.comment_id,
                c.author,
                c.comment,
                c.original,
                c.suggestion,
                c.final_text,
                c.action.as_str(),
                c.confidence,
                c.judge,
                c.category,
                now()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_case_outcome(
        &self,
        id: i64,
        action: CaseAction,
        final_text: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE cases SET action = ?2, final_text = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, action.as_str(), final_text, now()],
        )?;
        Ok(())
    }

    fn case_rows(&self, sql: &str, p: impl rusqlite::Params) -> Result<Vec<Case>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(p, |r| {
            Ok(Case {
                id: r.get(0)?,
                reviewer_id: r.get(1)?,
                doc_key: r.get(2)?,
                doc_name: r.get(3)?,
                comment_id: r.get(4)?,
                author: r.get(5)?,
                comment: r.get(6)?,
                original: r.get(7)?,
                suggestion: r.get(8)?,
                final_text: r.get(9)?,
                action: CaseAction::parse(&r.get::<_, String>(10)?),
                confidence: r.get(11)?,
                judge: r.get(12)?,
                category: r.get(13)?,
                created_at: r.get(14)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    const CASE_COLUMNS: &str =
        "id, reviewer_id, doc_key, doc_name, comment_id, author, comment, original, suggestion,
                                final_text, action, confidence, judge, category, created_at";

    /// Decided cases of a reviewer, newest first.
    pub fn reviewer_cases(&self, reviewer_id: i64, limit: usize) -> Result<Vec<Case>> {
        self.case_rows(
            &format!(
                "SELECT {} FROM cases WHERE reviewer_id = ?1 AND action != 'pending' ORDER BY id DESC LIMIT ?2",
                Self::CASE_COLUMNS
            ),
            params![reviewer_id, limit as i64],
        )
    }

    pub fn case(&self, id: i64) -> Result<Option<Case>> {
        Ok(self
            .case_rows(
                &format!("SELECT {} FROM cases WHERE id = ?1", Self::CASE_COLUMNS),
                [id],
            )?
            .pop())
    }

    /// All decided cases, oldest first (dataset export).
    pub fn decided_cases(&self) -> Result<Vec<Case>> {
        self.case_rows(
            &format!(
                "SELECT {} FROM cases WHERE action != 'pending' ORDER BY id",
                Self::CASE_COLUMNS
            ),
            [],
        )
    }

    /// Decided cases of a reviewer added after its latest profile.
    pub fn cases_since_profile(&self, reviewer_id: i64) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM cases WHERE reviewer_id = ?1 AND action != 'pending'
               AND updated_at >= COALESCE((SELECT MAX(created_at) FROM profiles WHERE reviewer_id = ?1), 0)",
            [reviewer_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    // Profiles

    pub fn latest_profile(&self, reviewer_id: i64) -> Result<Option<Profile>> {
        Ok(self
            .conn
            .query_row(
                "SELECT reviewer_id, version, summary, case_count, created_at FROM profiles
                 WHERE reviewer_id = ?1 ORDER BY version DESC LIMIT 1",
                [reviewer_id],
                |r| {
                    Ok(Profile {
                        reviewer_id: r.get(0)?,
                        version: r.get(1)?,
                        summary: r.get(2)?,
                        case_count: r.get::<_, i64>(3)? as usize,
                        created_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    // Proofreading cache

    /// Cached proofreading results for these paragraph hashes, newer than
    /// `since` (unix seconds): hash → result JSON.
    pub fn proofread_cached(
        &self,
        doc_key: &str,
        hashes: &[String],
        checks: &str,
        since: i64,
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT result FROM proofread_cache
             WHERE doc_key = ?1 AND para_hash = ?2 AND checks = ?3 AND created_at >= ?4",
        )?;
        let mut out = std::collections::HashMap::new();
        for h in hashes {
            if out.contains_key(h) {
                continue;
            }
            if let Some(result) = stmt
                .query_row(params![doc_key, h, checks, since], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
            {
                out.insert(h.clone(), result);
            }
        }
        Ok(out)
    }

    /// Stores proofreading results: (paragraph hash, result JSON).
    pub fn save_proofread_cache(
        &self,
        doc_key: &str,
        checks: &str,
        entries: &[(String, String)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO proofread_cache (doc_key, para_hash, checks, result, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(doc_key, para_hash, checks)
                 DO UPDATE SET result = excluded.result, created_at = excluded.created_at",
            )?;
            let t = now();
            for (hash, result) in entries {
                stmt.execute(params![doc_key, hash, checks, result, t])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets a document's cached proofreading results (all documents when
    /// `doc_key` is `None`).
    pub fn clear_proofread_cache(&self, doc_key: Option<&str>) -> Result<()> {
        match doc_key {
            Some(k) => self
                .conn
                .execute("DELETE FROM proofread_cache WHERE doc_key = ?1", [k])?,
            None => self.conn.execute("DELETE FROM proofread_cache", [])?,
        };
        Ok(())
    }

    pub fn add_profile(
        &self,
        reviewer_id: i64,
        summary: &str,
        case_count: usize,
    ) -> Result<Profile> {
        let version = self
            .latest_profile(reviewer_id)?
            .map_or(1, |p| p.version + 1);
        self.conn.execute(
            "INSERT INTO profiles (reviewer_id, version, summary, case_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![reviewer_id, version, summary, case_count as i64, now()],
        )?;
        Ok(self.latest_profile(reviewer_id)?.expect("just inserted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(reviewer_id: Option<i64>, action: CaseAction) -> Case {
        Case {
            id: 0,
            reviewer_id,
            doc_key: "d".into(),
            doc_name: "报告.docx".into(),
            comment_id: "1".into(),
            author: "张三".into(),
            comment: "数据口径不一致".into(),
            original: "原文".into(),
            suggestion: "建议".into(),
            final_text: None,
            action,
            confidence: Some(0.9),
            judge: None,
            category: Some("数据口径".into()),
            created_at: 0,
        }
    }

    #[test]
    fn settings_round_trip_and_default() {
        let store = Store::open_in_memory().unwrap();
        let mut s = store.settings().unwrap();
        assert_eq!(s.fix.threshold, 0.8);
        s.fix.threshold = 0.7;
        s.roles.chat.model = "deepseek".into();
        store.save_settings(&s).unwrap();
        assert_eq!(store.settings().unwrap(), s);
    }

    #[test]
    fn reviewers_aliases_cases_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("app.db")).unwrap();
        let a = store.create_reviewer("张三", "发改委").unwrap();
        let b = store.create_reviewer("张三（重复）", "").unwrap();
        store.set_alias("zs", "", "", Some(b.id)).unwrap();
        store
            .set_alias("Administrator", "A", "doc1", Some(a.id))
            .unwrap();

        let id = store
            .add_case(&case(Some(b.id), CaseAction::Pending))
            .unwrap();
        store
            .set_case_outcome(id, CaseAction::Edited, Some("最终"))
            .unwrap();
        assert_eq!(
            store.reviewer_cases(b.id, 10).unwrap()[0]
                .final_text
                .as_deref(),
            Some("最终")
        );

        store.merge_reviewers(b.id, a.id).unwrap();
        let reviewers = store.reviewers().unwrap();
        assert_eq!(reviewers.len(), 1);
        assert_eq!(reviewers[0].case_count, 1);
        assert!(
            store
                .aliases()
                .unwrap()
                .iter()
                .all(|al| al.reviewer_id == a.id)
        );
        assert_eq!(store.cases_since_profile(a.id).unwrap(), 1);

        let p = store.add_profile(a.id, "{}", 1).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(store.add_profile(a.id, "{}", 2).unwrap().version, 2);
        assert_eq!(store.reviewers().unwrap()[0].profile_version, Some(2));

        store.delete_reviewer(a.id).unwrap();
        assert!(store.aliases().unwrap().is_empty());
        assert_eq!(store.decided_cases().unwrap()[0].reviewer_id, None);
    }

    #[test]
    fn proofread_cache_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let hashes = vec!["h1".to_string(), "h2".to_string()];
        assert!(
            store
                .proofread_cached("doc", &hashes, "typo", 0)
                .unwrap()
                .is_empty()
        );
        store
            .save_proofread_cache("doc", "typo", &[("h1".into(), "{\"a\":1}".into())])
            .unwrap();
        store
            .save_proofread_cache("doc", "typo", &[("h1".into(), "{\"a\":2}".into())])
            .unwrap();
        let got = store.proofread_cached("doc", &hashes, "typo", 0).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got["h1"], "{\"a\":2}");
        assert!(
            store
                .proofread_cached("doc", &hashes, "format", 0)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .proofread_cached("other", &hashes, "typo", 0)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .proofread_cached("doc", &hashes, "typo", now() + 10)
                .unwrap()
                .is_empty()
        );
        store.clear_proofread_cache(Some("doc")).unwrap();
        assert!(
            store
                .proofread_cached("doc", &hashes, "typo", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn providers_and_model_cache() {
        let store = Store::open_in_memory().unwrap();
        let p = ProviderRecord {
            id: "gw".into(),
            name: "网关".into(),
            kind: "openai".into(),
            base_url: "https://example.com/v1".into(),
            decision_path: None,
            rerank_path: None,
        };
        store.upsert_provider(&p).unwrap();
        store
            .replace_provider_models(
                "gw",
                &[("a".into(), "{}".into()), ("b".into(), "{}".into())],
            )
            .unwrap();
        store
            .set_manual_profile("gw", "b", Some("{\"contextWindow\":8000}"))
            .unwrap();
        store
            .replace_provider_models("gw", &[("c".into(), "{}".into())])
            .unwrap();
        let models = store.provider_models("gw").unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(ids, ["b", "c"], "manual profiles survive a refresh");
        store.delete_provider("gw").unwrap();
        assert!(store.provider_models("gw").unwrap().is_empty());
    }
}
