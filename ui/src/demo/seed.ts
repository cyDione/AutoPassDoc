import type { Case, KbDocument, ModelView, ProviderView, Reviewer, ReviewerProfile, Settings } from "../types";

/** Times are Unix seconds, as the Rust side sends them. */
const DAY = 86_400;
const now = Math.floor(Date.now() / 1000);

export const GATEWAY_ID = "p-gateway";

export function seedProviders(): ProviderView[] {
  return [
    {
      id: GATEWAY_ID,
      name: "测试网关",
      kind: "openai",
      baseUrl: "https://gptload.example.com/v1",
      decisionPath: null,
      rerankPath: null,
      hasKey: true,
    },
  ];
}

export function gatewayModels(): ModelView[] {
  return [
    {
      id: "cline-pass/deepseek-v4.1-flash",
      roleHint: "chat",
      profile: { contextWindow: 131_072, maxOutputTokens: 8_192, levels: ["off", "high"], reasoning: "toggle", jsonMode: true, source: "fetched" },
      manual: false,
    },
    {
      id: "jev-latest",
      roleHint: "decision",
      profile: { contextWindow: 32_768, maxOutputTokens: 1_024, levels: [], reasoning: "none", jsonMode: true, source: "builtin" },
      manual: false,
    },
    {
      id: "BAAI/bge-m3",
      roleHint: "embedding",
      profile: { contextWindow: 8_192, maxOutputTokens: 0, levels: [], reasoning: "none", jsonMode: false, source: "builtin" },
      manual: false,
    },
    {
      id: "BAAI/bge-reranker-v2-m3",
      roleHint: "rerank",
      profile: { contextWindow: 8_192, maxOutputTokens: 0, levels: [], reasoning: "none", jsonMode: false, source: "builtin" },
      manual: false,
    },
  ];
}

export function seedSettings(): Settings {
  return {
    roles: {
      chat: { providerId: GATEWAY_ID, model: "cline-pass/deepseek-v4.1-flash", thinking: "" },
      decision: { providerId: GATEWAY_ID, model: "jev-latest", thinking: "" },
      decisionBackend: "jev",
      embedding: { providerId: GATEWAY_ID, model: "BAAI/bge-m3", thinking: "" },
      rerank: { providerId: GATEWAY_ID, model: "BAAI/bge-reranker-v2-m3", thinking: "" },
    },
    fix: {
      threshold: 0.8,
      editMode: "tracked",
      author: "AutoPassDoc",
      resolveOnApply: true,
      replyOnApply: false,
      replyText: "已根据该意见修改。",
      useKb: true,
      kbPassages: 6,
      concurrency: 3,
      profileEvery: 10,
    },
    proofread: { concurrency: 6, thinking: false },
    web: { mode: "auto", modelSearch: null, engine: "bing", whitelist: [...DEFAULT_WHITELIST], service: "none" },
    kb: { parser: "builtin", mineruModel: "vlm", mineruOcr: true, mineruFormula: true, paddleocrBaseUrl: "https://paddleocr.aistudio-app.com", paddleocrModel: "PaddleOCR-VL-1.6" },
  };
}

/** Mirrors `app_core::web::DEFAULT_WHITELIST`. */
export const DEFAULT_WHITELIST = [
  "gov.cn",
  "www.gov.cn",
  "flk.npc.gov.cn",
  "www.npc.gov.cn",
  "www.moj.gov.cn",
  "std.samr.gov.cn",
  "openstd.samr.gov.cn",
  "hbba.sacinfo.org.cn",
  "dbba.sacinfo.org.cn",
  "www.mohurd.gov.cn",
  "www.ccsn.org.cn",
  "www.ndrc.gov.cn",
  "www.mof.gov.cn",
  "www.mee.gov.cn",
  "www.mnr.gov.cn",
  "www.mwr.gov.cn",
  "www.mot.gov.cn",
  "www.miit.gov.cn",
  "www.mem.gov.cn",
  "www.nea.gov.cn",
  "www.stats.gov.cn",
  "www.ccgp.gov.cn",
  "www.shanghai.gov.cn",
  "fgw.sh.gov.cn",
  "zjw.sh.gov.cn",
  "ghzyj.sh.gov.cn",
  "sthj.sh.gov.cn",
  "tjj.sh.gov.cn",
  "www.spcsc.sh.cn",
  "www.shcm.gov.cn",
  "www.pudong.gov.cn",
];

export function seedReviewers(): Reviewer[] {
  return [
    { id: 1, name: "张主任", note: "市发展改革委评审组组长，看重数据口径和政策依据", threshold: null, caseCount: 0, profileVersion: 2 },
    { id: 2, name: "王处长", note: "市大数据局，关注建设内容与投资匹配", threshold: 0.85, caseCount: 0, profileVersion: null },
    { id: 3, name: "李教授", note: "高校专家，关注方法论和逻辑结构", threshold: null, caseCount: 0, profileVersion: null },
  ];
}

/** Comment signatures already mapped to reviewers. */
export function seedAliases(): Map<string, number> {
  return new Map([
    ["张主任", 1],
    ["王处长", 2],
    ["李教授", 3],
  ]);
}

export function seedProfiles(): Map<number, ReviewerProfile> {
  return new Map([
    [
      1,
      {
        reviewerId: 1,
        version: 2,
        createdAt: now - 6 * DAY,
        caseCount: 6,
        summary:
          "张主任审稿以数据口径和政策依据为主线，要求所有数据注明来源并与统计年鉴一致，引用文件必须是现行有效版本并写明文号。对措辞要求简洁规范，不接受口语化和无量化支撑的形容词。",
        focus: ["数据口径与统计年鉴一致", "引用政策文件的时效性和文号", "投资估算与附表一致", "风险措施落实到责任单位"],
        preferences: ["数据后用括号注明来源，如“（数据来源：2024年市统计年鉴）”", "用“聚焦”“稳步推进”等规范表述替代口语", "先写问题再写措施"],
        commonRequests: ["补充数据来源", "更新为现行标准版本", "细化责任单位和时间节点"],
      },
    ],
  ]);
}

export function seedCases(): Case[] {
  const c = (
    id: number,
    reviewerId: number,
    daysAgo: number,
    comment: string,
    original: string,
    suggestion: string,
    action: Case["action"],
    confidence: number | null,
    category: string,
    finalText: string | null = action === "accepted" ? suggestion : null,
  ): Case => ({
    id,
    reviewerId,
    docName: "某市数字政府项目可研报告（第二轮）.docx",
    commentId: String(900 + id),
    author: seedReviewers().find((r) => r.id === reviewerId)!.name,
    comment,
    original,
    suggestion,
    finalText,
    action,
    confidence,
    category,
    createdAt: now - daysAgo * DAY,
  });
  return [
    c(1, 1, 12, "此处数据口径需与统计年鉴保持一致，请核实并注明来源。", "全市常住人口约为 960 万人。", "全市常住人口约为960万人（数据来源：2024年市统计年鉴）。", "accepted", 0.91, "数据口径"),
    c(2, 1, 11, "此处引用的标准已废止，请更新为现行版本。", "参照GB/T 22239-2008开展等级保护测评。", "参照GB/T 22239-2019开展等级保护测评。", "accepted", 0.88, "政策依据"),
    c(3, 1, 10, "表述过于口语化，请按公文规范修改。", "我们会尽快把系统搞起来。", "项目将按计划有序推进系统建设。", "edited", 0.83, "措辞规范", "项目将按照建设计划稳步推进系统建设。"),
    c(4, 1, 9, "风险应对措施过于笼统，请细化到责任单位和时间节点。", "加强风险防控。", "由市大数据局牵头，于2025年6月底前建立风险防控机制。", "accepted", 0.86, "逻辑结构"),
    c(5, 1, 7, "建议补充政策依据，引用最新文件文号。", "按照国家有关要求建设。", "按照《政务信息化项目建设管理办法》（国办发〔2024〕12号）要求建设。", "rejected", 0.62, "政策依据"),
    c(6, 1, 3, "投资估算与附表不一致，请统一。", "项目总投资3850万元。", "项目总投资3862万元，与附表3-2一致。", "pending", 0.74, "数据口径"),
    c(7, 2, 8, "请说明测算方法和主要参数取值依据。", "预计节约运维费用约200万元。", "按现有系统年运维费用的15%测算，预计每年节约运维费用约200万元。", "accepted", 0.84, "数据口径"),
    c(8, 2, 5, "投资估算与附表不一致，请统一。", "软件开发费1720万元。", "软件开发费1726万元。", "edited", 0.79, "数据口径", "软件开发费1726万元（详见附表3-2）。"),
    c(9, 2, 2, "“显著”一词缺乏量化支撑，建议给出具体指标。", "具有显著的社会效益。", "预计业务办理时长缩短30%以上，具有较为明显的社会效益。", "accepted", 0.9, "措辞规范"),
    c(10, 3, 4, "该段逻辑不够清晰，建议调整结构，先讲问题再讲措施。", "本方案提出以下措施……现有系统存在以下问题……", "现有系统存在以下问题……针对上述问题，本方案提出以下措施……", "accepted", 0.87, "逻辑结构"),
  ];
}

export function seedKbDocuments(): KbDocument[] {
  const d = (
    id: number,
    title: string,
    fileName: string,
    format: string,
    docNumber: string | null,
    issuer: string,
    date: string,
    chunkCount: number,
    charCount: number,
    daysAgo: number,
    warnings: string[] = [],
  ): KbDocument => ({
    id,
    title,
    fileName,
    storedPath: `C:\\Users\\演示\\AppData\\Roaming\\AutoPassDoc\\kb\\${fileName}`,
    originalPath: `D:\\资料\\政策文件\\${fileName}`,
    format,
    meta: { title, docNumber, issuer, date },
    chunkCount,
    charCount,
    importedAt: now - daysAgo * DAY,
    sha256: `${id}`.padStart(64, "a"),
    warnings,
    parser: "builtin",
  });
  return [
    d(1, "国务院办公厅关于印发《政务信息化项目建设管理办法》的通知", "政务信息化项目建设管理办法.pdf", "pdf", "国办发〔2024〕12号", "国务院办公厅", "2024-03-15", 142, 38_200, 20),
    d(2, "某市数字政府建设“十四五”规划", "某市数字政府建设十四五规划.docx", "docx", "某政发〔2023〕8号", "某市人民政府", "2023-02-20", 318, 86_500, 18),
    d(3, "关于加强政务数据安全管理工作的通知", "关于加强政务数据安全管理工作的通知.docx", "docx", "某网信办〔2024〕5号", "某市委网信办", "2024-06-03", 56, 12_900, 9),
    d(4, "数据管理能力成熟度评估模型（GB/T 36073-2018）", "GBT 36073-2018 数据管理能力成熟度评估模型.pdf", "pdf", "GB/T 36073-2018", "国家市场监督管理总局", "2018-03-15", 204, 61_300, 7, [
      "第 3–5 页是扫描图片，未能提取文字（需要 OCR）。",
    ]),
    d(5, "某省公共数据管理办法", "某省公共数据管理办法.md", "md", "某省政府令第280号", "某省人民政府", "2023-11-01", 88, 21_700, 2),
  ];
}

/** Passages cited by demo fixes and returned by demo searches. */
export const KB_PASSAGES: { docId: number; headingPath: string[]; text: string }[] = [
  {
    docId: 1,
    headingPath: ["第三章 项目建设", "第十四条"],
    text: "项目建设单位应当在可行性研究报告中说明主要数据的来源和测算方法，数据应与统计部门公布的数据口径保持一致。",
  },
  {
    docId: 1,
    headingPath: ["第二章 项目审批", "第九条"],
    text: "投资估算应当与项目建设内容相匹配，估算表与正文数据不一致的，审批部门应当退回补正。",
  },
  {
    docId: 2,
    headingPath: ["四、主要任务", "（二）推进数据资源整合共享"],
    text: "到2025年，全市政务数据共享率达到95%以上，“一网通办”事项覆盖率达到98%，业务办理时长平均压缩50%。",
  },
  {
    docId: 3,
    headingPath: ["二、工作要求", "（三）落实安全责任"],
    text: "各部门应当明确数据安全责任人，按照“谁主管谁负责、谁使用谁负责”的原则，于每年6月底前完成数据安全自查。",
  },
  {
    docId: 4,
    headingPath: ["5 能力成熟度等级", "5.3 稳健级"],
    text: "组织已将数据作为实现组织目标的重要资产，在组织层面制定了系列的标准化管理流程，促进数据管理的规范化。",
  },
  {
    docId: 5,
    headingPath: ["第四章 公共数据共享", "第二十一条"],
    text: "公共管理和服务机构应当通过公共数据平台共享公共数据，不得通过其他渠道重复采集。",
  },
];
