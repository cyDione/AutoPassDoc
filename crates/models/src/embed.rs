//! Embeddings (`POST embeddings`) and reranking (`POST {rerank_path}`).

use serde_json::{Value, json};

use crate::client::{Client, endpoint_label};
use crate::error::{Error, Result};
use crate::provider::Provider;

impl Client {
    /// Embeds `texts` in batches of [`crate::ClientConfig::embed_batch_size`],
    /// returning one vector per text in input order. Fails when the server
    /// returns the wrong count or vectors of different dimensions.
    pub async fn embed(
        &self,
        p: &Provider,
        model: &str,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>> {
        p.require_openai_style("向量（Embedding）")?;
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = p.openai_url("embeddings")?;
        let endpoint = endpoint_label("POST", &url);
        let decode = |detail: String| Error::decode(&endpoint, detail);

        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.config().embed_batch_size.max(1)) {
            let body = json!({ "model": model, "input": batch, "encoding_format": "float" });
            let v = self.post_json(p, &url, &body).await?;
            let data = v
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| decode("缺少 data 数组".into()))?;
            let mut rows = data
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let index = d
                        .get("index")
                        .and_then(Value::as_u64)
                        .map_or(i, |n| n as usize);
                    let vector = d
                        .get("embedding")
                        .and_then(Value::as_array)
                        .and_then(|a| a.iter().map(|x| x.as_f64().map(|f| f as f32)).collect())
                        .ok_or_else(|| decode(format!("第 {index} 条缺少浮点向量")))?;
                    Ok((index, vector))
                })
                .collect::<Result<Vec<(usize, Vec<f32>)>>>()?;
            rows.sort_by_key(|(i, _)| *i);
            if rows.len() != batch.len() || rows.iter().enumerate().any(|(i, (idx, _))| i != *idx) {
                return Err(decode(format!(
                    "返回 {} 个向量，应为 {} 个且序号连续",
                    rows.len(),
                    batch.len()
                )));
            }
            out.extend(rows.into_iter().map(|(_, v)| v));
        }

        let dim = out[0].len();
        if dim == 0 || out.iter().any(|v| v.len() != dim) {
            return Err(decode("向量维度不一致或为空".into()));
        }
        Ok(out)
    }

    /// Scores every doc against `query`; returns one score per input doc in
    /// input order (`f32::NEG_INFINITY` for docs the server left out). Accepts
    /// `results` / `data` / top-level arrays of `{index, relevance_score | score}`.
    pub async fn rerank(
        &self,
        p: &Provider,
        model: &str,
        query: &str,
        docs: &[String],
    ) -> Result<Vec<f32>> {
        p.require_openai_style("重排（Rerank）")?;
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let url = p.openai_url(p.rerank_path())?;
        let body = json!({
            "model": model,
            "query": query,
            "documents": docs,
            "top_n": docs.len(),
            "return_documents": false,
        });
        let v = self.post_json(p, &url, &body).await?;
        let endpoint = endpoint_label("POST", &url);
        let items = v
            .as_array()
            .or_else(|| v.get("results")?.as_array())
            .or_else(|| v.get("data")?.as_array())
            .ok_or_else(|| Error::decode(&endpoint, "缺少 results 数组"))?;

        let mut scores = vec![f32::NEG_INFINITY; docs.len()];
        let mut found = 0;
        for item in items {
            let index = item.get("index").and_then(Value::as_u64);
            let score = item
                .get("relevance_score")
                .or_else(|| item.get("score"))
                .and_then(Value::as_f64);
            if let (Some(i), Some(s)) = (index, score)
                && let Some(slot) = scores.get_mut(i as usize)
            {
                *slot = s as f32;
                found += 1;
            }
        }
        if found == 0 {
            return Err(Error::decode(&endpoint, "未返回任何文档得分"));
        }
        Ok(scores)
    }
}
