mod common;

use common::{SAMPLE, write};
use kb::{KnowledgeBase, SearchQuery};

fn query(text: &str, embedding: Option<Vec<f32>>) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        embedding: embedding.map(|e| ("m".to_string(), e)),
        ..SearchQuery::default()
    }
}

fn embed_all(kb: &mut KnowledgeBase, model: &str, dim: usize) {
    let (_, total) = kb.embedding_progress(model).unwrap();
    let items: Vec<(i64, Vec<f32>)> = kb
        .pending_embeddings(model, total)
        .unwrap()
        .into_iter()
        .map(|(id, text)| {
            let mut v = vec![0.0f32; dim];
            for c in text.chars() {
                v[(c as usize) % dim] += 1.0;
            }
            (id, v)
        })
        .collect();
    kb.store_embeddings(model, &items).unwrap();
}

#[test]
fn merge_copies_new_documents_with_index_vectors_and_files() {
    let dir = tempfile::tempdir().unwrap();
    let a_path = write(dir.path(), "数据管理办法.txt", SAMPLE);
    let b_path = write(
        dir.path(),
        "交通方案.txt",
        "某市智慧交通三年行动方案\n一、总体目标\n到2026年，全市主要路口信号灯联网率达到95%。\n二、重点任务\n（一）推广电子停车收费，覆盖中心城区全部道路停车位。",
    );

    // Source: both documents, vectors of "m" (8 dims) and "other" (4 dims).
    let mut source = KnowledgeBase::open(&dir.path().join("source")).unwrap();
    source.import_file(&a_path).unwrap();
    source.import_file(&b_path).unwrap();
    embed_all(&mut source, "m", 8);
    embed_all(&mut source, "other", 4);
    let snapshot_dir = dir.path().join("snap");
    std::fs::create_dir_all(snapshot_dir.join("files")).unwrap();
    source.snapshot_to(&snapshot_dir.join("kb.sqlite")).unwrap();
    for doc in source.documents().unwrap() {
        std::fs::copy(
            &doc.stored_path,
            snapshot_dir
                .join("files")
                .join(doc.stored_path.file_name().unwrap()),
        )
        .unwrap();
    }

    // Target: already has the first document; "other" has another dimension.
    let mut target = KnowledgeBase::open(&dir.path().join("target")).unwrap();
    target.import_file(&a_path).unwrap();
    embed_all(&mut target, "m", 8);
    embed_all(&mut target, "other", 6);
    // Loads the vector cache, which the merge must invalidate.
    let v = vec![1.0f32; 8];
    target.search(&query("停车", Some(v.clone()))).unwrap();

    let stats = target.merge_from(&snapshot_dir).unwrap();
    assert_eq!(stats.documents_added, 1);
    assert_eq!(stats.documents_skipped, 1);
    assert!(stats.chunks_added > 0);
    assert_eq!(stats.embeddings_added, stats.chunks_added);
    assert_eq!(stats.skipped_models, ["other"]);
    assert!(stats.missing_files.is_empty());

    let docs = target.documents().unwrap();
    assert_eq!(docs.len(), 2);
    let added = docs.iter().find(|d| d.file_name == "交通方案.txt").unwrap();
    assert_eq!(
        std::fs::read(&added.stored_path).unwrap(),
        std::fs::read(&b_path).unwrap()
    );
    let hits = target.search(&query("停车收费", None)).unwrap();
    assert_eq!(hits[0].doc_id, added.id);
    // Vector-only hits of the new document show up too.
    let hits = target.search(&query("zzzz", Some(v))).unwrap();
    assert!(hits.iter().any(|h| h.doc_id == added.id));
    let (done, total) = target.embedding_progress("m").unwrap();
    assert_eq!(done, total);

    // Merging again adds nothing.
    let again = target.merge_from(&snapshot_dir).unwrap();
    assert_eq!(again.documents_added, 0);
    assert_eq!(again.documents_skipped, 2);

    // Removing the merged document cleans its index rows.
    target.remove_document(added.id).unwrap();
    assert!(
        target
            .search(&query("停车收费", None))
            .unwrap()
            .iter()
            .all(|h| h.doc_id != added.id)
    );
}
