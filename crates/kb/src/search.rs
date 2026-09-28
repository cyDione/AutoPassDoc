//! Hybrid retrieval: FTS5 bm25 and brute-force vector search, fused with RRF.

use std::collections::{HashMap, HashSet};

use rusqlite::OptionalExtension;

use crate::metadata::find_doc_numbers;
use crate::text::normalize;
use crate::vector::VectorIndex;
use crate::{Error, Hit, KnowledgeBase, Result, SearchQuery, db, display_title, tokenize};

/// Candidates taken from each retriever before fusion.
const RETRIEVE_K: usize = 50;
/// The usual reciprocal rank fusion constant.
const RRF_K: f32 = 60.0;
const DEFAULT_LIMIT: usize = 30;
/// bm25 weights of the FTS columns: chunk text, heading path, document fields.
const BM25: &str = "bm25(chunks_fts, 1.0, 2.0, 0.5)";

#[derive(Default)]
struct Fused {
    keyword_rank: Option<usize>,
    vector_rank: Option<usize>,
    score: f32,
}

/// Reciprocal rank fusion of ranked id lists: score = Σ 1/(60 + rank), ranks from 1.
fn fuse(keyword: &[i64], vector: &[i64]) -> HashMap<i64, Fused> {
    let mut fused: HashMap<i64, Fused> = HashMap::new();
    for (i, id) in keyword.iter().enumerate() {
        let f = fused.entry(*id).or_default();
        f.keyword_rank = Some(i + 1);
        f.score += 1.0 / (RRF_K + (i + 1) as f32);
    }
    for (i, id) in vector.iter().enumerate() {
        let f = fused.entry(*id).or_default();
        f.vector_rank = Some(i + 1);
        f.score += 1.0 / (RRF_K + (i + 1) as f32);
    }
    fused
}

fn id_list(ids: impl IntoIterator<Item = i64>) -> String {
    ids.into_iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

impl KnowledgeBase {
    /// Hybrid search: keyword top 50 and, when `q.embedding` is given and
    /// chunks have embeddings from that model, vector top 50, fused with RRF.
    /// Chunks of the same parent are collapsed to the best one. Documents
    /// whose 文号 appears in the query come first.
    pub fn search(&self, q: &SearchQuery) -> Result<Vec<Hit>> {
        let limit = if q.limit == 0 { DEFAULT_LIMIT } else { q.limit };
        let filter: Option<HashSet<i64>> =
            q.doc_ids.as_ref().map(|ids| ids.iter().copied().collect());
        if filter.as_ref().is_some_and(HashSet::is_empty) {
            return Ok(Vec::new());
        }
        let (keyword, cited_docs) = self.keyword_search(&q.text, filter.as_ref())?;
        let vector = match &q.embedding {
            Some((model, embedding)) => self.vector_search(model, embedding, filter.as_ref())?,
            None => Vec::new(),
        };
        let fused = fuse(&keyword, &vector);
        if fused.is_empty() {
            return Ok(Vec::new());
        }

        let mut owners: HashMap<i64, (i64, i64)> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT id, doc_id, parent_id FROM chunks WHERE id IN ({})",
                id_list(fused.keys().copied())
            ))?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                owners.insert(r.get(0)?, (r.get(1)?, r.get(2)?));
            }
        }
        let mut ranked: Vec<(i64, Fused)> = fused
            .into_iter()
            .filter(|(id, _)| owners.contains_key(id))
            .collect();
        let best_rank = |f: &Fused| {
            [f.keyword_rank, f.vector_rank]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(usize::MAX)
        };
        ranked.sort_by(|(a_id, a), (b_id, b)| {
            let cited = |id: &i64| cited_docs.contains(&owners[id].0);
            cited(b_id)
                .cmp(&cited(a_id))
                .then(b.score.total_cmp(&a.score))
                .then(best_rank(a).cmp(&best_rank(b)))
                .then(a_id.cmp(b_id))
        });
        let mut seen_parents = HashSet::new();
        ranked.retain(|(id, _)| seen_parents.insert(owners[id].1));
        ranked.truncate(limit);

        ranked
            .into_iter()
            .filter_map(|(id, f)| {
                self.hit(id, f.keyword_rank, f.vector_rank, f.score)
                    .transpose()
            })
            .collect()
    }

    /// Chunk ids ranked by bm25, and the documents cited by 文号 in the query
    /// (whose chunks are ranked first).
    fn keyword_search(
        &self,
        text: &str,
        filter: Option<&HashSet<i64>>,
    ) -> Result<(Vec<i64>, HashSet<i64>)> {
        let terms = tokenize::query_terms(text);
        if terms.is_empty() {
            return Ok((Vec::new(), HashSet::new()));
        }
        let expression = tokenize::match_expression(&terms);
        let run = |docs: Option<String>| -> Result<Vec<i64>> {
            // `+rowid` filters the matches instead of letting SQLite probe the
            // FTS index once per chunk of the listed documents, which is slow.
            let restriction = docs.map_or(String::new(), |ids| {
                format!(" AND +rowid IN (SELECT id FROM chunks WHERE doc_id IN ({ids}))")
            });
            let mut stmt = self.conn.prepare(&format!(
                "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1{restriction}
                 ORDER BY {BM25} LIMIT ?2"
            ))?;
            let ids = stmt
                .query_map(rusqlite::params![expression, RETRIEVE_K as i64], |r| {
                    r.get(0)
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        };
        let mut ids = run(filter.map(|f| id_list(f.iter().copied())))?;

        let cited = self.docs_cited_by_number(text, filter)?;
        if !cited.is_empty() {
            let mut first = run(Some(id_list(cited.iter().copied())))?;
            let in_first: HashSet<i64> = first.iter().copied().collect();
            first.extend(ids.into_iter().filter(|id| !in_first.contains(id)));
            first.truncate(RETRIEVE_K);
            ids = first;
        }
        Ok((ids, cited))
    }

    /// Documents whose stored 文号 appears in `text`.
    fn docs_cited_by_number(
        &self,
        text: &str,
        filter: Option<&HashSet<i64>>,
    ) -> Result<HashSet<i64>> {
        let numbers = find_doc_numbers(&normalize(text));
        if numbers.is_empty() {
            return Ok(HashSet::new());
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id, doc_number FROM documents WHERE doc_number IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        let mut out = HashSet::new();
        while let Some(r) = rows.next()? {
            let (id, number): (i64, String) = (r.get(0)?, r.get(1)?);
            let cited = numbers
                .iter()
                .any(|m| m.normalized == number || m.with_context.ends_with(&number));
            if cited && filter.is_none_or(|f| f.contains(&id)) {
                out.insert(id);
            }
        }
        Ok(out)
    }

    fn vector_search(
        &self,
        model: &str,
        embedding: &[f32],
        filter: Option<&HashSet<i64>>,
    ) -> Result<Vec<i64>> {
        let Some((model_id, dim)) = self.model(model)? else {
            return Ok(Vec::new());
        };
        if embedding.len() != dim {
            return Err(Error::DimensionMismatch {
                model: model.to_string(),
                expected: dim,
                actual: embedding.len(),
            });
        }
        let mut cache = self.vectors.borrow_mut();
        let version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if version != self.vectors_version.replace(version) {
            cache.clear();
        }
        let index = match cache.entry(model_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(self.load_vectors(model_id, dim)?)
            }
        };
        debug_assert_eq!(index.dim(), dim);
        Ok(index
            .search(embedding, RETRIEVE_K, filter)
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    fn load_vectors(&self, model_id: i64, dim: usize) -> Result<VectorIndex> {
        let mut index = VectorIndex::new(dim);
        let mut stmt = self.conn.prepare(
            "SELECT e.chunk_id, c.doc_id, e.vector FROM embeddings e
             JOIN chunks c ON c.id = e.chunk_id WHERE e.model_id = ?1",
        )?;
        let mut rows = stmt.query([model_id])?;
        while let Some(r) = rows.next()? {
            let vector = db::blob_to_vector(&r.get::<_, Vec<u8>>(2)?);
            if vector.len() == dim {
                index.upsert(r.get(0)?, r.get(1)?, &vector);
            }
        }
        Ok(index)
    }

    pub(crate) fn hit(
        &self,
        chunk_id: i64,
        keyword_rank: Option<usize>,
        vector_rank: Option<usize>,
        score: f32,
    ) -> Result<Option<Hit>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT c.doc_id, d.file_name, d.title, d.stored_name, c.heading_path, c.text,
                    p.text, c.char_start, c.char_end
             FROM chunks c
             JOIN parents p ON p.id = c.parent_id
             JOIN documents d ON d.id = c.doc_id
             WHERE c.id = ?1",
        )?;
        Ok(stmt
            .query_row([chunk_id], |r| {
                let file_name: String = r.get(1)?;
                Ok(Hit {
                    chunk_id,
                    doc_id: r.get(0)?,
                    title: display_title(r.get::<_, Option<String>>(2)?.as_deref(), &file_name),
                    stored_path: self.files_dir().join(r.get::<_, String>(3)?),
                    heading_path: db::split_path(&r.get::<_, String>(4)?),
                    text: r.get(5)?,
                    parent_text: r.get(6)?,
                    char_start: r.get::<_, i64>(7)? as usize,
                    char_end: r.get::<_, i64>(8)? as usize,
                    keyword_rank,
                    vector_rank,
                    score,
                    file_name,
                })
            })
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_rewards_agreement() {
        let fused = fuse(&[1, 2, 3], &[3, 4, 1]);
        let score = |id| fused[&id].score;
        // 1: 1/61 + 1/63; 3: 1/63 + 1/61 — equal; both beat single-list hits.
        assert!((score(1) - score(3)).abs() < 1e-6);
        assert!(score(3) > score(2) && score(3) > score(4));
        assert!(score(2) > score(4) - 1e-6 && (score(2) - score(4)).abs() < 1e-6);
        assert_eq!(fused[&4].keyword_rank, None);
        assert_eq!(fused[&4].vector_rank, Some(2));
    }
}
