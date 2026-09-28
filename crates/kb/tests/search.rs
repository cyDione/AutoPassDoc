mod common;

use common::{SAMPLE, kb_in, write};
use kb::{Error, Hit, KnowledgeBase, SearchQuery};

/// A district notice that cites the sample by 文号 and mentions 第二十条.
const NOTICE: &str = "某某区人民政府办公室关于贯彻落实公共数据管理工作的通知
某区政办发〔2024〕21号
各街道办事处，区政府各部门：
　　根据某政发〔2024〕7号文件精神，结合本区实际，现就做好公共数据管理工作通知如下。
一、提高思想认识
各部门要充分认识公共数据管理的重要意义，把数据归集共享作为重点工作。
二、落实工作责任
区数据局负责统筹协调，各部门主要负责人为第一责任人。依照第二十条规定，区政府将公共数据工作纳入年度考核。
某某区人民政府办公室
2024年4月2日";

struct Fixture {
    _dir: tempfile::TempDir,
    kb: KnowledgeBase,
    regulation: i64,
    notice: i64,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut kb = kb_in(dir.path());
    let regulation = kb
        .import_file(&write(dir.path(), "管理办法.txt", SAMPLE))
        .unwrap()
        .doc_id;
    let notice = kb
        .import_file(&write(dir.path(), "区通知.txt", NOTICE))
        .unwrap()
        .doc_id;
    Fixture {
        _dir: dir,
        kb,
        regulation,
        notice,
    }
}

fn text(q: &str) -> SearchQuery {
    SearchQuery {
        text: q.into(),
        ..SearchQuery::default()
    }
}

fn with_embedding(q: &str, model: &str, v: Vec<f32>) -> SearchQuery {
    SearchQuery {
        text: q.into(),
        embedding: Some((model.into(), v)),
        ..SearchQuery::default()
    }
}

fn one_hot(dim: usize, i: usize) -> Vec<f32> {
    let mut v = vec![0.0; dim];
    v[i] = 1.0;
    v
}

/// Gives every chunk a one-hot vector; returns (chunk id, text) in vector order.
fn embed_one_hot(kb: &mut KnowledgeBase, model: &str) -> Vec<(i64, String)> {
    let pending = kb.pending_embeddings(model, 10_000).unwrap();
    let dim = pending.len();
    let items: Vec<(i64, Vec<f32>)> = pending
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, one_hot(dim, i)))
        .collect();
    kb.store_embeddings(model, &items).unwrap();
    pending
}

fn last_heading(hit: &Hit) -> &str {
    hit.heading_path.last().map_or("", String::as_str)
}

#[test]
fn keyword_search_finds_words_and_attaches_parent() {
    let f = fixture();
    let hits = f.kb.search(&text("数据质量评价机制")).unwrap();
    let top = &hits[0];
    assert_eq!(top.doc_id, f.regulation);
    assert!(top.text.contains("质量评价机制"));
    assert_eq!(last_heading(top), "第六条");
    assert_eq!(top.keyword_rank, Some(1));
    assert_eq!(top.vector_rank, None);
    assert!((top.score - 1.0 / 61.0).abs() < 1e-6);
    // The parent is the whole article, which is longer than the child.
    assert!(top.parent_text.starts_with("第二节　数据归集\n第六条"));
    assert!(top.parent_text.ends_with("应当在目录中注明理由。"));
    assert!(top.parent_text.len() > top.text.len());
    assert_eq!(top.file_name, "管理办法.txt");
    assert_eq!(
        top.title,
        "某某市人民政府关于印发《某某市公共数据管理办法》的通知"
    );
    assert!(top.stored_path.exists());

    // Children of one parent collapse into one hit.
    let hits = f.kb.search(&text("公共数据平台 单位")).unwrap();
    let mut parents: Vec<&str> = hits.iter().map(|h| h.parent_text.as_str()).collect();
    let n = parents.len();
    parents.sort();
    parents.dedup();
    assert_eq!(parents.len(), n);
    // Scores are ordered.
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
}

#[test]
fn doc_number_in_query_puts_that_document_first() {
    let f = fixture();
    for q in [
        "某政发〔2024〕7号",
        "某政发[2024]7号的要求",
        "按照某政发（2024）7号",
    ] {
        let hits = f.kb.search(&text(q)).unwrap();
        assert_eq!(hits[0].doc_id, f.regulation, "{q}");
        // The citing notice matches too, but after the cited document.
        let first_notice = hits.iter().position(|h| h.doc_id == f.notice).unwrap();
        assert!(
            hits[..first_notice]
                .iter()
                .all(|h| h.doc_id == f.regulation)
        );
        assert!(hits[first_notice..].iter().all(|h| h.doc_id == f.notice));
    }
    let hits = f.kb.search(&text("某区政办发〔2024〕21号")).unwrap();
    assert_eq!(hits[0].doc_id, f.notice);
}

#[test]
fn article_lookup_prefers_the_article_itself() {
    let f = fixture();
    let hits = f.kb.search(&text("第二十条")).unwrap();
    assert_eq!(hits[0].heading_path, ["第三章 监督管理", "第二十条"]);
    assert_eq!(hits[0].doc_id, f.regulation);
    // Only real references match: not 第二十一条 or 第二十二条.
    assert!(
        hits.iter().all(|h| h.text.contains("第二十条")),
        "{hits:#?}"
    );
    assert!(hits.iter().any(|h| h.doc_id == f.notice));

    // 第六条 cites 第五条, but the article itself comes first.
    let hits = f.kb.search(&text("第5条")).unwrap();
    assert_eq!(last_heading(&hits[0]), "第五条");
    assert!(hits.iter().any(|h| last_heading(h) == "第六条"));

    let hits = f.kb.search(&text("第二十条 绩效考核")).unwrap();
    assert_eq!(last_heading(&hits[0]), "第二十条");
}

#[test]
fn document_filter_limits_results() {
    let f = fixture();
    let q = SearchQuery {
        doc_ids: Some(vec![f.notice]),
        ..text("第二十条 公共数据")
    };
    let hits = f.kb.search(&q).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| h.doc_id == f.notice));
    let none = SearchQuery {
        doc_ids: Some(vec![]),
        ..text("公共数据")
    };
    assert!(f.kb.search(&none).unwrap().is_empty());
    let limited = SearchQuery {
        limit: 2,
        ..text("公共数据")
    };
    assert_eq!(f.kb.search(&limited).unwrap().len(), 2);
    assert!(f.kb.search(&text("，。的")).unwrap().is_empty());
}

#[test]
fn vector_search_finds_the_nearest_chunk() {
    let mut f = fixture();
    let order = embed_one_hot(&mut f.kb, "onehot");
    let dim = order.len();
    let (target, target_text) = order
        .iter()
        .find(|(_, t)| t.contains("空间地理"))
        .unwrap()
        .clone();
    let hits =
        f.kb.search(&with_embedding(
            "",
            "onehot",
            one_hot(dim, order.iter().position(|o| o.0 == target).unwrap()),
        ))
        .unwrap();
    assert_eq!(hits[0].chunk_id, target);
    assert_eq!(hits[0].vector_rank, Some(1));
    assert_eq!(hits[0].keyword_rank, None);
    assert!(target_text.starts_with("附件 > 二、分类方法\n"));

    // A blend of two directions ranks both, in order.
    let (a, b) = (3, 9);
    let mut q = vec![0.0; dim];
    q[a] = 0.9;
    q[b] = 0.4;
    let hits = f.kb.search(&with_embedding("", "onehot", q)).unwrap();
    assert_eq!(hits[0].chunk_id, order[a].0);
    assert_eq!(hits[1].chunk_id, order[b].0);

    // Vectors of other models or dimensions do not mix.
    let other =
        f.kb.search(&with_embedding("", "other-model", vec![1.0; 8]))
            .unwrap();
    assert!(other.is_empty());
    let err =
        f.kb.search(&with_embedding("", "onehot", vec![1.0; dim + 1]))
            .unwrap_err();
    assert!(matches!(err, Error::DimensionMismatch { .. }));
}

#[test]
fn rrf_ranks_agreement_first() {
    let mut f = fixture();
    let keyword = f.kb.search(&text("责令 限期整改")).unwrap();
    let (p, q) = (keyword[0].chunk_id, keyword[1].chunk_id);
    let r =
        f.kb.search(&text("电子证照"))
            .unwrap()
            .into_iter()
            .find(|h| !keyword.iter().any(|k| k.chunk_id == h.chunk_id))
            .unwrap()
            .chunk_id;
    // Only three chunks get vectors: q is nearest, then r, then p.
    f.kb.store_embeddings(
        "m",
        &[
            (q, vec![1.0, 0.0, 0.0]),
            (r, vec![0.0, 1.0, 0.0]),
            (p, vec![0.0, 0.0, 1.0]),
        ],
    )
    .unwrap();
    let hits =
        f.kb.search(&with_embedding("责令 限期整改", "m", vec![1.0, 0.5, 0.0]))
            .unwrap();
    let ids: Vec<i64> = hits.iter().map(|h| h.chunk_id).collect();
    // q: 1/62 + 1/61, p: 1/61 + 1/63, r: 1/62.
    assert_eq!(ids[..3], [q, p, r]);
    assert_eq!(
        (hits[0].keyword_rank, hits[0].vector_rank),
        (Some(2), Some(1))
    );
    assert_eq!(
        (hits[1].keyword_rank, hits[1].vector_rank),
        (Some(1), Some(3))
    );
    assert_eq!((hits[2].keyword_rank, hits[2].vector_rank), (None, Some(2)));
    for h in &hits {
        let expected: f32 = [h.keyword_rank, h.vector_rank]
            .into_iter()
            .flatten()
            .map(|r| 1.0 / (60.0 + r as f32))
            .sum();
        assert!((h.score - expected).abs() < 1e-6);
    }
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
}

#[test]
fn embedding_queue_and_progress() {
    let mut f = fixture();
    let total = f.kb.stats().unwrap().chunks;
    assert_eq!(f.kb.embedding_progress("m").unwrap(), (0, total));
    let batch = f.kb.pending_embeddings("m", 5).unwrap();
    assert_eq!(batch.len(), 5);
    assert!(batch.windows(2).all(|w| w[0].0 < w[1].0));

    let all = f.kb.pending_embeddings("m", 1000).unwrap();
    assert_eq!(all.len(), total);
    let article = all
        .iter()
        .find(|(_, t)| t.contains("纳入绩效考核体系"))
        .unwrap();
    assert!(
        article
            .1
            .starts_with("第三章 监督管理 > 第二十条\n第三章　监督管理\n第二十条　")
    );
    // A chunk without headings is embedded as is.
    let header = all
        .iter()
        .find(|(_, t)| t.contains("某政发〔2024〕7号"))
        .unwrap();
    assert!(header.1.starts_with("某某市人民政府文件"));

    let vectors: Vec<(i64, Vec<f32>)> = batch.iter().map(|(id, _)| (*id, vec![0.5; 4])).collect();
    f.kb.store_embeddings("m", &vectors).unwrap();
    assert_eq!(f.kb.embedding_progress("m").unwrap(), (5, total));
    let rest = f.kb.pending_embeddings("m", 1000).unwrap();
    assert_eq!(rest.len(), total - 5);
    assert!(rest.iter().all(|(id, _)| !batch.iter().any(|b| b.0 == *id)));
    assert_eq!(f.kb.pending_embeddings("other", 1000).unwrap().len(), total);

    // Replacing is idempotent; unknown chunks are skipped.
    f.kb.store_embeddings("m", &vectors[..2]).unwrap();
    f.kb.store_embeddings("m", &[(i64::MAX, vec![1.0; 4])])
        .unwrap();
    assert_eq!(f.kb.embedding_progress("m").unwrap(), (5, total));
    assert_eq!(f.kb.stats().unwrap().embedded.get("m"), Some(&5));

    let err =
        f.kb.store_embeddings("m", &[(rest[0].0, vec![1.0; 3])])
            .unwrap_err();
    assert!(matches!(
        err,
        Error::DimensionMismatch {
            expected: 4,
            actual: 3,
            ..
        }
    ));
    assert!(err.to_string().contains("维"));
    assert!(matches!(
        f.kb.store_embeddings("m", &[(rest[0].0, vec![])]),
        Err(Error::EmptyVector)
    ));
    f.kb.store_embeddings("m", &[]).unwrap();

    f.kb.clear_embeddings("m").unwrap();
    assert_eq!(f.kb.embedding_progress("m").unwrap(), (0, total));
    assert!(f.kb.stats().unwrap().embedded.is_empty());
    // After clearing, the model may come back with another dimension.
    f.kb.store_embeddings("m", &[(rest[0].0, vec![1.0; 3])])
        .unwrap();
}

#[test]
fn vector_cache_follows_writes() {
    let mut f = fixture();
    let pending = f.kb.pending_embeddings("m", 1000).unwrap();
    let (first, second) = (pending[0].0, pending[1].0);
    f.kb.store_embeddings("m", &[(first, vec![1.0, 0.0])])
        .unwrap();
    let q = || with_embedding("", "m", vec![0.0, 1.0]);
    assert_eq!(f.kb.search(&q()).unwrap()[0].chunk_id, first);
    // The loaded cache sees new vectors and replacements.
    f.kb.store_embeddings("m", &[(second, vec![0.1, 1.0])])
        .unwrap();
    assert_eq!(f.kb.search(&q()).unwrap()[0].chunk_id, second);
    f.kb.store_embeddings("m", &[(first, vec![0.0, 1.0])])
        .unwrap();
    assert_eq!(f.kb.search(&q()).unwrap()[0].chunk_id, first);
}

#[test]
fn remove_document_cleans_index_embeddings_and_file() {
    let mut f = fixture();
    embed_one_hot(&mut f.kb, "m");
    let dim = f.kb.embedding_progress("m").unwrap().1;
    // Warm the vector cache so removal has to prune it.
    assert!(
        !f.kb
            .search(&with_embedding("", "m", one_hot(dim, 0)))
            .unwrap()
            .is_empty()
    );
    let doc = f.kb.document(f.regulation).unwrap().unwrap();
    let notice_chunks = f.kb.document(f.notice).unwrap().unwrap().chunk_count;

    f.kb.remove_document(f.regulation).unwrap();
    assert!(f.kb.document(f.regulation).unwrap().is_none());
    assert!(f.kb.full_text(f.regulation).unwrap().is_none());
    assert!(!doc.stored_path.exists());
    let stats = f.kb.stats().unwrap();
    assert_eq!((stats.documents, stats.chunks), (1, notice_chunks));
    assert_eq!(stats.embedded.get("m"), Some(&notice_chunks));
    assert_eq!(
        f.kb.embedding_progress("m").unwrap(),
        (notice_chunks, notice_chunks)
    );

    assert!(f.kb.search(&text("篡改 日志 空间地理")).unwrap().is_empty());
    for i in 0..dim {
        let hits =
            f.kb.search(&with_embedding("", "m", one_hot(dim, i)))
                .unwrap();
        assert!(hits.iter().all(|h| h.doc_id == f.notice));
    }
    let conn = rusqlite::Connection::open(f.kb.root().join("kb.sqlite")).unwrap();
    let fts_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fts_rows as usize, notice_chunks);
    let fts_hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH '\"篡改\"'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_hits, 0);

    assert!(matches!(
        f.kb.remove_document(f.regulation),
        Err(Error::DocumentNotFound(_))
    ));
}

#[test]
fn vector_cache_sees_writes_from_another_instance() {
    fn assert_send<T: Send>() {}
    assert_send::<KnowledgeBase>();

    let mut f = fixture();
    let mut other = KnowledgeBase::open(f.kb.root()).unwrap();
    let pending = f.kb.pending_embeddings("m", 1000).unwrap();
    let (first, second) = (pending[0].0, pending[1].0);
    f.kb.store_embeddings("m", &[(first, vec![1.0, 0.0])])
        .unwrap();
    let q = || with_embedding("", "m", vec![0.0, 1.0]);
    assert_eq!(f.kb.search(&q()).unwrap()[0].chunk_id, first);

    other
        .store_embeddings("m", &[(second, vec![0.0, 1.0])])
        .unwrap();
    assert_eq!(f.kb.search(&q()).unwrap()[0].chunk_id, second);
    other.remove_document(f.regulation).unwrap();
    other.remove_document(f.notice).unwrap();
    assert!(f.kb.search(&q()).unwrap().is_empty());
}
