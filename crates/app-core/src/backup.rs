//! Data backup (S-3): the knowledge base (originals, index, vectors) and the
//! reviewers (mappings, cases, profiles) in one `.apdbak` file, a zip of
//!
//! - `manifest.json`: [`BackupManifest`];
//! - `app.sqlite`: a `VACUUM INTO` snapshot of `app.db` (settings,
//!   providers, reviewers, cases, profiles; API keys are never in it);
//! - `kb/kb.sqlite`: a `VACUUM INTO` snapshot of the knowledge base;
//! - `kb/files/*`: the stored originals.
//!
//! Importing either replaces the current data (after saving it to
//! `<data dir>/backups/自动备份-时间.apdbak`) or merges the backup into it.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::core::Core;
use crate::error::{Error, Result};
use crate::store::Store;

/// Version of the backup layout written into the manifest.
pub const BACKUP_FORMAT: u32 = 1;
/// File extension of backups, without the dot.
pub const BACKUP_EXTENSION: &str = "apdbak";

const MANIFEST: &str = "manifest.json";
const APP_DB: &str = "app.sqlite";
const KB_DB: &str = "kb/kb.sqlite";
const KB_FILES: &str = "kb/files/";

/// What a backup holds, stored as `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    /// [`BACKUP_FORMAT`] of the writer.
    pub format: u32,
    pub app_version: String,
    /// Unix seconds.
    pub created_at: i64,
    pub documents: usize,
    pub chunks: usize,
    pub reviewers: usize,
    pub cases: usize,
}

/// Outcome of [`Core::export_backup`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub path: PathBuf,
    /// Size of the backup file in bytes.
    pub size: u64,
    pub manifest: BackupManifest,
    /// Stored originals that were missing on disk and are not in the backup.
    pub missing_files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Replace all current data, after backing it up automatically.
    Replace,
    /// Add what is not there yet.
    Merge,
}

/// Outcome of [`Core::import_backup`]. With [`ImportMode::Replace`] the
/// "added" counts are what the data holds now and nothing is skipped.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub mode: ImportMode,
    pub manifest: BackupManifest,
    pub reviewers_added: usize,
    /// Reviewers that already existed under the same name.
    pub reviewers_skipped: usize,
    pub cases_added: usize,
    /// Cases already present (same reviewer, comment, original and final text).
    pub cases_skipped: usize,
    pub profiles_added: usize,
    pub documents_added: usize,
    /// Documents whose content was already in the knowledge base.
    pub documents_skipped: usize,
    /// Where the data was saved before a replace.
    pub auto_backup: Option<PathBuf>,
    /// Things the user should know, e.g. vectors that need to be generated again.
    pub warnings: Vec<String>,
}

/// Holds the knowledge-base busy flag (shared with embedding) until dropped.
struct Busy<'a>(&'a AtomicBool);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

type Progress<'a> = &'a dyn Fn(usize, usize, &str);

impl Core {
    /// Where automatic backups go.
    pub fn backups_dir(&self) -> PathBuf {
        self.data_dir().join("backups")
    }

    fn claim_kb(&self) -> Result<Busy<'_>> {
        if self.embedding.swap(true, Ordering::AcqRel) {
            return Err(Error::Invalid(
                "正在生成向量，请等它结束后再备份或导入".into(),
            ));
        }
        Ok(Busy(&self.embedding))
    }

    /// Writes all data to a backup file at `path` (written to a temporary
    /// file next to it, then renamed). `progress(done, total, step)`.
    pub fn export_backup(
        &self,
        path: &Path,
        progress: impl Fn(usize, usize, &str),
    ) -> Result<ExportSummary> {
        let _busy = self.claim_kb()?;
        self.write_backup(path, &progress)
    }

    fn write_backup(&self, path: &Path, progress: Progress) -> Result<ExportSummary> {
        progress(0, 1, "正在生成数据快照");
        let work = tempfile::Builder::new()
            .prefix(".backup-")
            .tempdir_in(self.data_dir())?;
        let app_db = work.path().join("app.sqlite");
        let kb_db = work.path().join("kb.sqlite");
        let (reviewers, cases) = {
            let store = self.store();
            store.snapshot_to(&app_db)?;
            (store.reviewers()?.len(), store.case_total()?)
        };
        let (files_dir, stats) = {
            let guard = self.kb()?;
            let kb = guard.as_ref().expect("opened");
            kb.snapshot_to(&kb_db)?;
            (kb.root().join("files"), kb.stats()?)
        };
        let stored_names: Vec<String> = {
            let conn = rusqlite::Connection::open_with_flags(
                &kb_db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let mut stmt = conn.prepare("SELECT stored_name FROM documents ORDER BY id")?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let manifest = BackupManifest {
            format: BACKUP_FORMAT,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: crate::store::now(),
            documents: stats.documents,
            chunks: stats.chunks,
            reviewers,
            cases,
        };

        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        std::fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".apdbak-")
            .suffix(".tmp")
            .tempfile_in(&dir)?;
        let total = stored_names.len() + 2;
        let mut missing_files = Vec::new();
        {
            let mut zip = ZipWriter::new(BufWriter::new(tmp.as_file_mut()));
            let json = serde_json::to_vec_pretty(&manifest)?;
            zip.start_file(MANIFEST, options(json.len() as u64, true))
                .map_err(zip_error)?;
            zip.write_all(&json)?;
            progress(0, total, "正在写入审稿人数据");
            add_file(&mut zip, APP_DB, &app_db, true)?;
            progress(1, total, "正在写入知识库索引");
            add_file(&mut zip, KB_DB, &kb_db, true)?;
            for (i, name) in stored_names.iter().enumerate() {
                progress(2 + i, total, name);
                let source = files_dir.join(name);
                if !source.is_file() {
                    missing_files.push(name.clone());
                    continue;
                }
                add_file(
                    &mut zip,
                    &format!("{KB_FILES}{name}"),
                    &source,
                    compressible(name),
                )?;
            }
            zip.finish()
                .map_err(zip_error)?
                .into_inner()
                .map_err(|e| e.into_error())?
                .sync_all()?;
        }
        tmp.persist(path).map_err(|e| e.error)?;
        progress(total, total, "");
        Ok(ExportSummary {
            path: path.to_path_buf(),
            size: std::fs::metadata(path)?.len(),
            manifest,
            missing_files,
        })
    }

    /// Reads a backup's manifest without importing it, e.g. to show what it
    /// holds before the user picks a mode.
    pub fn inspect_backup(&self, path: &Path) -> Result<BackupManifest> {
        read_manifest(&mut open_archive(path)?)
    }

    /// Imports a backup. [`ImportMode::Replace`] first saves the current
    /// data under [`Core::backups_dir`]. `progress(done, total, step)`.
    pub fn import_backup(
        &self,
        path: &Path,
        mode: ImportMode,
        progress: impl Fn(usize, usize, &str),
    ) -> Result<ImportSummary> {
        let _busy = self.claim_kb()?;
        let mut archive = open_archive(path)?;
        let manifest = read_manifest(&mut archive)?;
        let work = tempfile::Builder::new()
            .prefix(".import-")
            .tempdir_in(self.data_dir())?;
        extract(&mut archive, work.path(), &progress)?;
        drop(archive);
        let app_db = work.path().join(APP_DB);
        let kb_dir = work.path().join("kb");
        if !app_db.is_file() || !kb_dir.join("kb.sqlite").is_file() {
            return Err(Error::Invalid(
                "备份文件不完整，缺少数据库，无法导入".into(),
            ));
        }
        // Brings both copies to the current schema and refuses newer ones
        // before anything is touched.
        let source = Store::open_backup(&app_db)?;
        drop(kb::KnowledgeBase::open(&kb_dir)?);

        let mut summary = ImportSummary {
            mode,
            manifest,
            reviewers_added: 0,
            reviewers_skipped: 0,
            cases_added: 0,
            cases_skipped: 0,
            profiles_added: 0,
            documents_added: 0,
            documents_skipped: 0,
            auto_backup: None,
            warnings: Vec::new(),
        };
        match mode {
            ImportMode::Replace => {
                drop(source);
                let auto = self.backups_dir().join(format!(
                    "自动备份-{}.{BACKUP_EXTENSION}",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                ));
                self.write_backup(&auto, &|done, total, step| {
                    progress(done, total, &format!("正在自动备份当前数据 {step}"))
                })?;
                summary.auto_backup = Some(auto);
                progress(0, 1, "正在替换数据");
                self.replace_data(&app_db, &kb_dir)?;
                let store = self.store();
                summary.reviewers_added = store.reviewers()?.len();
                summary.cases_added = store.case_total()?;
                summary.profiles_added = count(&store, "SELECT COUNT(*) FROM profiles")?;
                drop(store);
                summary.documents_added = self.with_kb(|kb| kb.stats())?.documents;
            }
            ImportMode::Merge => {
                progress(0, 2, "正在合并知识库");
                let stats = self.with_kb(|kb| kb.merge_from(&kb_dir))?;
                summary.documents_added = stats.documents_added;
                summary.documents_skipped = stats.documents_skipped;
                for name in stats.missing_files {
                    summary.warnings.push(format!(
                        "备份里缺少资料原件「{name}」，只能检索，无法打开原文"
                    ));
                }
                for model in stats.skipped_models {
                    summary.warnings.push(format!(
                        "向量模型「{model}」的向量维度与本机不同，新导入的资料需要重新生成向量"
                    ));
                }
                progress(1, 2, "正在合并审稿人");
                merge_store(&self.store(), &source, &mut summary)?;
            }
        }
        progress(1, 1, "");
        Ok(summary)
    }

    /// Swaps in the extracted knowledge-base folder and restores `app.db`
    /// from `app_db`; on failure the current data is left as it was.
    fn replace_data(&self, app_db: &Path, kb_dir: &Path) -> Result<()> {
        let live = self.data_dir().join("kb");
        let old = self
            .data_dir()
            .join(format!("kb.old-{}", crate::store::now()));
        let mut guard = self.kb.lock().unwrap();
        // Closes the connection; the folder is opened again on next use.
        *guard = None;
        let had_old = live.exists();
        let locked = |e: std::io::Error| {
            Error::Invalid(format!(
                "无法替换知识库文件夹（资料可能正被其他程序打开，请关闭后重试）：{e}"
            ))
        };
        if had_old {
            std::fs::rename(&live, &old).map_err(locked)?;
        }
        if let Err(e) = std::fs::rename(kb_dir, &live) {
            if had_old {
                let _ = std::fs::rename(&old, &live);
            }
            return Err(locked(e));
        }
        if let Err(e) = self.store().restore_from(app_db) {
            let _ = std::fs::remove_dir_all(&live);
            if had_old {
                let _ = std::fs::rename(&old, &live);
            }
            return Err(e);
        }
        drop(guard);
        // Stored proposals point at cases of the replaced data.
        self.proposals.lock().unwrap().clear();
        if had_old {
            let _ = std::fs::remove_dir_all(&old);
        }
        Ok(())
    }
}

fn zip_error(e: zip::result::ZipError) -> Error {
    Error::Invalid(format!("备份文件读写失败：{e}"))
}

fn options(size: u64, compress: bool) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(if compress {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        })
        .large_file(size >= u64::from(u32::MAX))
}

/// Already compressed formats are stored as they are.
fn compressible(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    !matches!(
        ext.as_deref(),
        Some("docx" | "pdf" | "png" | "jpg" | "jpeg" | "zip" | "xlsx" | "pptx")
    )
}

fn add_file<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    name: &str,
    source: &Path,
    compress: bool,
) -> Result<()> {
    let mut file = File::open(source)?;
    let size = file.metadata()?.len();
    zip.start_file(name, options(size, compress))
        .map_err(zip_error)?;
    std::io::copy(&mut file, zip)?;
    Ok(())
}

fn open_archive(path: &Path) -> Result<ZipArchive<BufReader<File>>> {
    let file = File::open(path).map_err(|e| Error::Invalid(format!("无法打开备份文件：{e}")))?;
    ZipArchive::new(BufReader::new(file))
        .map_err(|_| Error::Invalid("这不是 AutoPassDoc 的备份文件".into()))
}

fn read_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<BackupManifest> {
    let mut text = String::new();
    archive
        .by_name(MANIFEST)
        .map_err(|_| Error::Invalid("这不是 AutoPassDoc 的备份文件（缺少说明文件）".into()))?
        .read_to_string(&mut text)
        .map_err(|_| Error::Invalid("备份文件已损坏，无法读取说明文件".into()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| Error::Invalid("备份文件已损坏，说明文件格式不对".into()))?;
    match value.get("format").and_then(serde_json::Value::as_u64) {
        Some(f) if f == u64::from(BACKUP_FORMAT) => {}
        Some(f) => {
            return Err(Error::Invalid(format!(
                "无法识别的备份格式（版本 {f}），可能来自更新版本的软件，请升级后再导入"
            )));
        }
        None => {
            return Err(Error::Invalid(
                "备份文件已损坏，说明文件缺少格式版本".into(),
            ));
        }
    }
    serde_json::from_value(value)
        .map_err(|_| Error::Invalid("备份文件已损坏，说明文件格式不对".into()))
}

/// Extracts the known entries into `dir`; anything else is ignored, as are
/// names that would escape the folder.
fn extract<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    dir: &Path,
    progress: Progress,
) -> Result<()> {
    std::fs::create_dir_all(dir.join(KB_FILES))?;
    let total = archive.len();
    for i in 0..total {
        let mut entry = archive.by_index(i).map_err(zip_error)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let target = match name.as_str() {
            APP_DB | KB_DB => dir.join(&name),
            _ => match name.strip_prefix(KB_FILES) {
                Some(file)
                    if !file.is_empty()
                        && !file.contains(['/', '\\', ':'])
                        && file != "."
                        && file != ".." =>
                {
                    dir.join(KB_FILES).join(file)
                }
                _ => continue,
            },
        };
        progress(i, total, "正在读取备份");
        let mut out = BufWriter::new(File::create(&target)?);
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| Error::Invalid(format!("备份文件已损坏：{e}")))?;
        out.flush()?;
    }
    Ok(())
}

fn count(store: &Store, sql: &str) -> Result<usize> {
    Ok(store.conn().query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
}

/// Adds the reviewers, author mappings, cases and profiles of `source` that
/// `target` does not have. Settings and model providers are left alone.
fn merge_store(target: &Store, source: &Store, summary: &mut ImportSummary) -> Result<()> {
    let src = source.conn();
    let tx = target.conn().unchecked_transaction()?;

    // Reviewers by name: backup id → id here.
    let mut ids: HashMap<i64, i64> = HashMap::new();
    {
        let mut stmt =
            src.prepare("SELECT id, name, note, threshold, created_at FROM reviewers ORDER BY id")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let (id, name): (i64, String) = (r.get(0)?, r.get(1)?);
            let existing: Option<i64> = tx
                .query_row("SELECT id FROM reviewers WHERE name = ?1", [&name], |r| {
                    r.get(0)
                })
                .optional()?;
            let here = match existing {
                Some(here) => {
                    summary.reviewers_skipped += 1;
                    here
                }
                None => {
                    tx.execute(
                        "INSERT INTO reviewers (name, note, threshold, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![
                            name,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<f64>>(3)?,
                            r.get::<_, i64>(4)?
                        ],
                    )?;
                    summary.reviewers_added += 1;
                    tx.last_insert_rowid()
                }
            };
            ids.insert(id, here);
        }
    }

    // Author mappings; one made here wins.
    {
        let mut stmt =
            src.prepare("SELECT author, initials, doc_key, reviewer_id FROM reviewer_aliases")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let Some(&reviewer) = ids.get(&r.get::<_, i64>(3)?) else {
                continue;
            };
            tx.execute(
                "INSERT OR IGNORE INTO reviewer_aliases (author, initials, doc_key, reviewer_id)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    reviewer
                ],
            )?;
        }
    }

    // Cases, deduplicated by (reviewer, comment, original, final text).
    {
        let mut stmt = src.prepare(
            "SELECT reviewer_id, doc_key, doc_name, comment_id, author, comment, original,
                    suggestion, final_text, action, confidence, judge, category, created_at,
                    updated_at
             FROM cases ORDER BY id",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let reviewer = r
                .get::<_, Option<i64>>(0)?
                .and_then(|id| ids.get(&id).copied());
            let comment: String = r.get(5)?;
            let original: String = r.get(6)?;
            let final_text: Option<String> = r.get(8)?;
            let exists = tx
                .query_row(
                    "SELECT 1 FROM cases WHERE reviewer_id IS ?1 AND comment = ?2
                       AND original = ?3 AND final_text IS ?4 LIMIT 1",
                    params![reviewer, comment, original, final_text],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if exists {
                summary.cases_skipped += 1;
                continue;
            }
            tx.execute(
                "INSERT INTO cases (reviewer_id, doc_key, doc_name, comment_id, author, comment,
                    original, suggestion, final_text, action, confidence, judge, category,
                    created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    reviewer,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    comment,
                    original,
                    r.get::<_, String>(7)?,
                    final_text,
                    r.get::<_, String>(9)?,
                    r.get::<_, Option<f64>>(10)?,
                    r.get::<_, Option<String>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, i64>(13)?,
                    r.get::<_, i64>(14)?
                ],
            )?;
            summary.cases_added += 1;
        }
    }

    // The latest profile of a reviewer, unless the one here is as new.
    for (&from, &here) in &ids {
        let latest: Option<(String, i64, i64)> = src
            .query_row(
                "SELECT summary, case_count, created_at FROM profiles
                 WHERE reviewer_id = ?1 ORDER BY version DESC LIMIT 1",
                [from],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((text, case_count, created_at)) = latest else {
            continue;
        };
        let (version, newest): (i64, Option<i64>) = tx.query_row(
            "SELECT COALESCE(MAX(version), 0), MAX(created_at) FROM profiles WHERE reviewer_id = ?1",
            [here],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if newest.is_some_and(|t| t >= created_at) {
            continue;
        }
        tx.execute(
            "INSERT INTO profiles (reviewer_id, version, summary, case_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![here, version + 1, text, case_count, created_at],
        )?;
        summary.profiles_added += 1;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretStore;
    use crate::store::{Case, CaseAction};

    fn core(dir: &Path) -> Core {
        Core::with_parts(
            dir,
            models::Client::new().unwrap(),
            SecretStore::file_only(dir),
        )
        .unwrap()
    }

    fn case(reviewer_id: i64, comment: &str) -> Case {
        Case {
            id: 0,
            reviewer_id: Some(reviewer_id),
            doc_key: "d".into(),
            doc_name: "报告.docx".into(),
            comment_id: "1".into(),
            author: "张三".into(),
            comment: comment.into(),
            original: "原文".into(),
            suggestion: "建议".into(),
            final_text: Some("定稿".into()),
            action: CaseAction::Edited,
            confidence: Some(0.9),
            judge: None,
            category: None,
            created_at: 0,
        }
    }

    fn import_text(core: &Core, dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        let r = core.kb_import(&[path], |_, _, _| {});
        assert!(r[0].error.is_none(), "{:?}", r[0].error);
    }

    const NOTICE: &str = "关于加强政务数据共享工作的通知\n第一条 政务数据共享遵循“以共享为原则、不共享为例外”的要求。\n第二条 各地区应将政务数据共享工作经费纳入财政预算。";
    const PLAN: &str = "某市智慧交通三年行动方案\n一、总体目标\n到2026年，全市主要路口信号灯联网率达到95%。\n二、重点任务\n推广电子停车收费，覆盖中心城区全部道路停车位。";

    /// A core with one reviewer, a mapping, two cases, a profile and one document.
    fn seeded(dir: &Path) -> Core {
        let core = core(&dir.join("data"));
        {
            let store = core.store();
            let r = store.create_reviewer("张三", "发改委").unwrap();
            store.set_alias("zs", "", "", Some(r.id)).unwrap();
            store.add_case(&case(r.id, "数据口径不一致")).unwrap();
            store.add_case(&case(r.id, "表述不规范")).unwrap();
            store.add_profile(r.id, "{\"v\":1}", 2).unwrap();
        }
        import_text(&core, dir, "通知.txt", NOTICE);
        core
    }

    fn search_titles(core: &Core, text: &str) -> Vec<String> {
        core.with_kb(|kb| {
            kb.search(&kb::SearchQuery {
                text: text.into(),
                ..Default::default()
            })
        })
        .unwrap()
        .into_iter()
        .map(|h| h.file_name)
        .collect()
    }

    #[test]
    fn export_then_replace_restores_everything() {
        let dir = tempfile::tempdir().unwrap();
        let source = seeded(dir.path());
        let file = dir.path().join("out").join("备份.apdbak");
        let exported = source.export_backup(&file, |_, _, _| {}).unwrap();
        assert_eq!(exported.manifest.documents, 1);
        assert_eq!(exported.manifest.reviewers, 1);
        assert_eq!(exported.manifest.cases, 2);
        assert!(exported.missing_files.is_empty());
        assert_eq!(source.inspect_backup(&file).unwrap(), exported.manifest);

        // A fresh core with other data of its own.
        let fresh = core(&dir.path().join("fresh"));
        fresh.store().create_reviewer("李四", "").unwrap();
        import_text(&fresh, dir.path(), "方案.txt", PLAN);

        let summary = fresh
            .import_backup(&file, ImportMode::Replace, |_, _, _| {})
            .unwrap();
        let auto = summary.auto_backup.clone().unwrap();
        assert!(auto.starts_with(fresh.backups_dir()));
        assert!(auto.is_file());
        assert_eq!(summary.reviewers_added, 1);
        assert_eq!(summary.cases_added, 2);
        assert_eq!(summary.documents_added, 1);

        let reviewers = fresh.store().reviewers().unwrap();
        assert_eq!(reviewers.len(), 1);
        assert_eq!(reviewers[0].name, "张三");
        assert_eq!(reviewers[0].case_count, 2);
        assert_eq!(reviewers[0].profile_version, Some(1));
        assert_eq!(fresh.store().aliases().unwrap().len(), 1);
        let docs = fresh.kb_documents().unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].stored_path.is_file());
        assert!(docs[0].stored_path.starts_with(fresh.data_dir()));
        assert!(search_titles(&fresh, "财政预算").contains(&"通知.txt".to_string()));
        assert!(search_titles(&fresh, "停车收费").is_empty());
        // The store connection still works for writes.
        fresh.store().create_reviewer("王五", "").unwrap();

        // The automatic backup brings the replaced data back.
        fresh
            .import_backup(&auto, ImportMode::Replace, |_, _, _| {})
            .unwrap();
        let names: Vec<String> = fresh
            .store()
            .reviewers()
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, ["李四"]);
        assert!(!search_titles(&fresh, "停车收费").is_empty());

        // Restored data is on disk, not only in the open connections.
        drop(fresh);
        let reopened = core(&dir.path().join("fresh"));
        assert_eq!(reopened.store().reviewers().unwrap()[0].name, "李四");
        let mode: String = reopened
            .store()
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(reopened.kb_documents().unwrap().len(), 1);
    }

    #[test]
    fn merge_dedupes_reviewers_cases_and_documents() {
        let dir = tempfile::tempdir().unwrap();
        let source = seeded(dir.path());
        {
            let store = source.store();
            let other = store.create_reviewer("李四", "").unwrap();
            store.add_case(&case(other.id, "缺少依据")).unwrap();
            store.add_profile(other.id, "{}", 1).unwrap();
        }
        import_text(&source, dir.path(), "方案.txt", PLAN);
        let file = dir.path().join("备份.apdbak");
        source.export_backup(&file, |_, _, _| {}).unwrap();

        // The target shares 张三, one of his cases and the notice.
        let tdir = tempfile::tempdir().unwrap();
        let target = core(&tdir.path().join("data"));
        {
            let store = target.store();
            let r = store.create_reviewer("张三", "").unwrap();
            store.add_case(&case(r.id, "数据口径不一致")).unwrap();
        }
        import_text(&target, tdir.path(), "通知.txt", NOTICE);

        let s = target
            .import_backup(&file, ImportMode::Merge, |_, _, _| {})
            .unwrap();
        assert_eq!((s.reviewers_added, s.reviewers_skipped), (1, 1));
        assert_eq!((s.cases_added, s.cases_skipped), (2, 1));
        assert_eq!(s.profiles_added, 2);
        assert_eq!((s.documents_added, s.documents_skipped), (1, 1));
        assert!(s.auto_backup.is_none());
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);

        let reviewers = target.store().reviewers().unwrap();
        assert_eq!(reviewers.len(), 2);
        let zhang = reviewers.iter().find(|r| r.name == "张三").unwrap();
        assert_eq!(zhang.case_count, 2);
        assert_eq!(zhang.profile_version, Some(1));
        assert_eq!(target.store().aliases().unwrap()[0].reviewer_id, zhang.id);
        assert_eq!(target.kb_documents().unwrap().len(), 2);
        assert!(!search_titles(&target, "停车收费").is_empty());

        // A second merge adds nothing.
        let s = target
            .import_backup(&file, ImportMode::Merge, |_, _, _| {})
            .unwrap();
        assert_eq!(
            (
                s.reviewers_added,
                s.cases_added,
                s.profiles_added,
                s.documents_added
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn rejects_bad_files_and_busy_state() {
        let dir = tempfile::tempdir().unwrap();
        let core = core(&dir.path().join("data"));
        let err = |path: &Path| {
            core.import_backup(path, ImportMode::Merge, |_, _, _| {})
                .unwrap_err()
                .to_string()
        };

        let not_zip = dir.path().join("a.apdbak");
        std::fs::write(&not_zip, "hello").unwrap();
        assert!(err(&not_zip).contains("不是 AutoPassDoc 的备份文件"));

        let write_zip = |name: &str, manifest: Option<&str>| {
            let path = dir.path().join(name);
            let mut zip = ZipWriter::new(File::create(&path).unwrap());
            if let Some(m) = manifest {
                zip.start_file(MANIFEST, SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(m.as_bytes()).unwrap();
            }
            zip.start_file("other.txt", SimpleFileOptions::default())
                .unwrap();
            zip.finish().unwrap();
            path
        };
        assert!(err(&write_zip("b.apdbak", None)).contains("缺少说明文件"));
        assert!(err(&write_zip("c.apdbak", Some("{\"format\":2}"))).contains("版本 2"));
        assert!(err(&write_zip("d.apdbak", Some("not json"))).contains("格式不对"));
        let incomplete = write_zip(
            "e.apdbak",
            Some(
                r#"{"format":1,"appVersion":"0.1.0","createdAt":0,"documents":0,"chunks":0,"reviewers":0,"cases":0}"#,
            ),
        );
        assert!(err(&incomplete).contains("不完整"));

        core.embedding.store(true, Ordering::Release);
        let busy = core
            .export_backup(&dir.path().join("x.apdbak"), |_, _, _| {})
            .unwrap_err();
        assert!(busy.to_string().contains("正在生成向量"));
        assert!(err(&incomplete).contains("正在生成向量"));
        // A refused call leaves the flag to its owner.
        assert!(core.embedding.load(Ordering::Acquire));
    }
}
