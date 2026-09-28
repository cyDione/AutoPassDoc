//! Brute-force vector search over an in-memory copy of one model's embeddings.
//!
//! Vectors are L2-normalised and quantised to i8 (scale 127), so cosine
//! similarity becomes an integer dot product and 100k × 1024 dims take ~100 MB.

use std::collections::{HashMap, HashSet};

pub(crate) struct VectorIndex {
    dim: usize,
    ids: Vec<i64>,
    docs: Vec<i64>,
    data: Vec<i8>,
    positions: HashMap<i64, usize>,
}

impl VectorIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            ids: Vec::new(),
            docs: Vec::new(),
            data: Vec::new(),
            positions: HashMap::new(),
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Adds or replaces the vector of a chunk. `vector` must have `dim` values.
    pub fn upsert(&mut self, chunk_id: i64, doc_id: i64, vector: &[f32]) {
        debug_assert_eq!(vector.len(), self.dim);
        let i = *self.positions.entry(chunk_id).or_insert_with(|| {
            self.ids.push(chunk_id);
            self.docs.push(doc_id);
            self.data.resize(self.data.len() + self.dim, 0);
            self.ids.len() - 1
        });
        quantize(vector, &mut self.data[i * self.dim..(i + 1) * self.dim]);
    }

    pub fn remove_doc(&mut self, doc_id: i64) {
        if !self.docs.contains(&doc_id) {
            return;
        }
        let mut kept = 0;
        for i in 0..self.ids.len() {
            if self.docs[i] == doc_id {
                continue;
            }
            if kept != i {
                self.ids[kept] = self.ids[i];
                self.docs[kept] = self.docs[i];
                self.data
                    .copy_within(i * self.dim..(i + 1) * self.dim, kept * self.dim);
            }
            kept += 1;
        }
        self.ids.truncate(kept);
        self.docs.truncate(kept);
        self.data.truncate(kept * self.dim);
        self.positions = self
            .ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();
    }

    /// The `k` chunks most similar to `query`, best first, with cosine similarity.
    pub fn search(&self, query: &[f32], k: usize, docs: Option<&HashSet<i64>>) -> Vec<(i64, f32)> {
        if k == 0 || self.ids.is_empty() {
            return Vec::new();
        }
        let mut q = vec![0i8; self.dim];
        quantize(query, &mut q);
        let mut scored: Vec<(i32, usize)> = self
            .data
            .chunks_exact(self.dim)
            .enumerate()
            .filter(|(i, _)| docs.is_none_or(|d| d.contains(&self.docs[*i])))
            .map(|(i, v)| (dot(&q, v), i))
            .collect();
        let k = k.min(scored.len());
        if k < scored.len() {
            scored.select_nth_unstable_by(k, |a, b| b.0.cmp(&a.0));
            scored.truncate(k);
        }
        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(self.ids[a.1].cmp(&self.ids[b.1])));
        let scale = 127.0 * 127.0;
        scored
            .into_iter()
            .map(|(s, i)| (self.ids[i], s as f32 / scale))
            .collect()
    }
}

fn quantize(v: &[f32], out: &mut [i8]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let scale = if norm > 0.0 { 127.0 / norm } else { 0.0 };
    for (o, x) in out.iter_mut().zip(v) {
        *o = (x * scale).round().clamp(-127.0, 127.0) as i8;
    }
}

fn dot(a: &[i8], b: &[i8]) -> i32 {
    // i16 products summed in i32 lanes vectorise well.
    a.iter()
        .zip(b)
        .map(|(&x, &y)| i32::from(i16::from(x) * i16::from(y)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nearest_and_removes_documents() {
        let mut index = VectorIndex::new(4);
        index.upsert(1, 10, &[1.0, 0.0, 0.0, 0.0]);
        index.upsert(2, 10, &[0.0, 1.0, 0.0, 0.0]);
        index.upsert(3, 20, &[0.0, 0.0, 3.0, 0.1]);
        let hits = index.search(&[0.0, 0.1, 5.0, 0.0], 2, None);
        assert_eq!(hits[0].0, 3);
        assert!(hits[0].1 > 0.95);
        index.upsert(3, 20, &[0.0, 1.0, 0.0, 0.0]);
        assert_eq!(index.search(&[0.0, 0.0, 1.0, 0.0], 3, None).len(), 3);
        index.remove_doc(10);
        let hits = index.search(&[1.0, 0.0, 0.0, 0.0], 5, None);
        assert_eq!(hits.iter().map(|h| h.0).collect::<Vec<_>>(), [3]);
        let only_20: HashSet<i64> = [10].into();
        assert!(
            index
                .search(&[1.0, 0.0, 0.0, 0.0], 5, Some(&only_20))
                .is_empty()
        );
    }
}
