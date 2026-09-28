//! Performance smoke test. Timings are asserted in release builds
//! (`cargo test -p kb --release --test perf -- --nocapture`) and only
//! printed in debug builds.

mod common;

use std::time::{Duration, Instant};

use common::{kb_in, write};
use kb::{KnowledgeBase, SearchQuery};

const DIM: usize = 1024;
const VECTORS: usize = 20_000;
const KEYWORD_LIMIT: Duration = Duration::from_millis(200);
const VECTOR_LIMIT: Duration = Duration::from_millis(150);

const SENTENCES: &[&str] = &[
    "各级人民政府应当加强对公共数据管理工作的领导，建立健全工作协调机制。",
    "公共数据提供单位应当按照目录要求及时归集、更新本单位的公共数据。",
    "数据处理者应当落实数据安全保护责任，采取必要措施保障数据安全。",
    "市财政部门应当将公共数据平台建设和运行维护经费纳入年度预算。",
    "违反本规定的，由有关主管部门责令改正，并依法追究相关人员责任。",
    "重要数据的处理者应当明确数据安全负责人和管理机构。",
    "开展数据处理活动应当遵守法律法规，尊重社会公德和伦理。",
    "个人信息处理者应当对其个人信息处理活动负责，并采取措施保障个人信息安全。",
    "政务服务事项应当推行一网通办，实现高频事项跨省通办。",
    "项目建设单位应当编制可行性研究报告，报发展改革部门审批。",
    "统计数据应当真实、准确、完整、及时，任何单位不得虚报、瞒报。",
    "安全生产工作应当坚持安全第一、预防为主、综合治理的方针。",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 2001) as f32 / 1000.0 - 1.0
    }
}

/// A regulation of `articles` articles, each one short parent/child.
fn synthetic_regulation(index: usize, articles: usize, rng: &mut Rng) -> String {
    let mut text = format!("某某市第{index}号管理规定\n某政规〔2023〕{index}号\n");
    for n in 1..=articles {
        if n % 20 == 1 {
            text.push_str(&format!("第{}章 第{}部分事项\n", n / 20 + 1, n / 20 + 1));
        }
        let a = SENTENCES[rng.next() as usize % SENTENCES.len()];
        let b = SENTENCES[rng.next() as usize % SENTENCES.len()];
        text.push_str(&format!("第{n}条 {a}{b}编号{index}-{n}。\n"));
    }
    text
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

fn check(label: &str, took: Duration, limit: Duration) {
    println!("{label}: {took:?} (release limit {limit:?})");
    if !cfg!(debug_assertions) {
        assert!(took < limit, "{label} took {took:?}, limit {limit:?}");
    }
}

fn search(kb: &KnowledgeBase, q: &SearchQuery) -> Duration {
    // Best of three, so a scheduling hiccup does not fail the run.
    (0..3)
        .map(|_| {
            let (hits, took) = time(|| kb.search(q).unwrap());
            assert!(!hits.is_empty());
            took
        })
        .min()
        .unwrap()
}

#[test]
fn large_knowledge_base_stays_fast() {
    let dir = tempfile::tempdir().unwrap();
    let mut kb = kb_in(dir.path());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);

    let docx = docx_engine::testgen::generate(&docx_engine::testgen::Spec::default());
    let path = write(dir.path(), "大文档.docx", docx);
    let (report, took) = time(|| kb.import_file(&path).unwrap());
    println!("import 220k-char docx: {took:?}, {} chunks", report.chunks);

    let (_, took) = time(|| {
        let mut n = 0;
        while kb.stats().unwrap().chunks < VECTORS {
            n += 1;
            let text = synthetic_regulation(n, 2_000, &mut rng);
            let path = write(dir.path(), &format!("规定{n}.txt"), text);
            kb.import_file(&path).unwrap();
        }
    });
    let stats = kb.stats().unwrap();
    println!(
        "import synthetic regulations: {took:?}, {} documents, {} chunks",
        stats.documents, stats.chunks
    );

    let queries = [
        "数据安全保护责任",
        "某政规〔2023〕3号",
        "第一千二百条",
        "项目总投资估算 硬件设备购置费 软件开发费",
        "建议补充政策依据，说明公共数据平台建设和运行维护经费的来源，并明确数据安全负责人和管理机构的职责分工。",
    ];
    for q in queries {
        let took = search(
            &kb,
            &SearchQuery {
                text: q.into(),
                ..SearchQuery::default()
            },
        );
        check(&format!("keyword search {q:?}"), took, KEYWORD_LIMIT);
    }

    let (_, took) = time(|| {
        loop {
            let pending = kb.pending_embeddings("bench", 1_000).unwrap();
            if pending.is_empty() {
                break;
            }
            let items: Vec<(i64, Vec<f32>)> = pending
                .into_iter()
                .map(|(id, _)| (id, (0..DIM).map(|_| rng.unit()).collect()))
                .collect();
            kb.store_embeddings("bench", &items).unwrap();
        }
    });
    let (done, total) = kb.embedding_progress("bench").unwrap();
    assert_eq!(done, total);
    assert!(done >= VECTORS);
    println!("store {done} embeddings of {DIM} dims: {took:?}");

    let query: Vec<f32> = (0..DIM).map(|_| rng.unit()).collect();
    let vector_only = SearchQuery {
        embedding: Some(("bench".into(), query.clone())),
        ..SearchQuery::default()
    };
    let (_, took) = time(|| kb.search(&vector_only).unwrap());
    println!("first vector search (loads the cache): {took:?}");
    check(
        &format!("vector search over {done} vectors"),
        search(&kb, &vector_only),
        VECTOR_LIMIT,
    );
    let hybrid = SearchQuery {
        text: queries[4].into(),
        embedding: Some(("bench".into(), query)),
        ..SearchQuery::default()
    };
    check(
        "hybrid search",
        search(&kb, &hybrid),
        KEYWORD_LIMIT + VECTOR_LIMIT,
    );
}
