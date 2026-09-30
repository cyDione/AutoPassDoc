import { memo, useCallback, useEffect, useMemo, useState } from "react";
import { BookMarked, Check, CheckCheck, ChevronDown, ChevronRight, Crosshair, EyeOff, FileSearch, Play, Search, Square } from "lucide-react";
import type { Backend } from "../api";
import type { Section } from "../components/reviewers/PreReviewCard";
import type { Notify, RunEdit } from "../hooks/useDocumentSession";
import type { OpenedDoc, ProjectFacts, ProofCategory, ProofCitation, ProofIssue, ProofProgress, ProofReport, Settings } from "../types";
import { errorMessage, formatNumber } from "../util";

interface Props {
  backend: Backend;
  doc: OpenedDoc | null;
  section: Section | null;
  settings: Settings | null;
  edit: RunEdit;
  onJump: (paragraphIndex: number) => void;
  /** Looks for the current version of a cited document on the web. */
  onSearch: (need: string) => void;
  onOpenSettings: () => void;
  notify: Notify;
}

export const CATEGORIES: [ProofCategory, string, string][] = [
  ["typo", "错别字", "错字、别字、多字漏字、的地得、标点误用"],
  ["numbering", "序号", "章节和条目编号跳号、重号、层级混用"],
  ["consistency", "前后一致", "同一数据、名称、结论前后不一致或相互矛盾"],
  ["misattribution", "张冠李戴", "把别的项目、地区、单位的名称写进本项目"],
  ["format", "格式", "多余空格、连续空行、全半角混用、括号引号不配对"],
  ["citation", "引用时效", "《》中引用的法律、标准、政策文件是否已废止或被替代"],
];
const LABEL = Object.fromEntries(CATEGORIES.map(([k, l]) => [k, l])) as Record<ProofCategory, string>;

const STAGE_TEXT: Record<ProofProgress["stage"], string> = {
  rules: "规则检查",
  screen: "决策模型初筛可疑段落",
  model: "大语言模型逐节通读",
  consistency: "比对前后数据",
  citations: "联网核查引用文件",
  done: "整理结果",
};

const STATUS: Record<NonNullable<ProofCitation["status"]>["status"], [string, string]> = {
  current: ["现行有效", "positive"],
  repealed: ["已废止", "negative"],
  superseded: ["已被替代", "warn"],
  unknown: ["未能确认", "neutral"],
};

const FACT_FIELDS: [keyof Omit<ProjectFacts, "others">, string][] = [
  ["name", "项目名称"],
  ["province", "省"],
  ["city", "市"],
  ["district", "区县"],
  ["owner", "建设单位"],
];

type Range = "all" | "section";
type State = "open" | "applied" | "ignored";

/** Moves the other issues of a paragraph past the text the applied ones changed. */
function shift(issues: ProofIssue[], applied: ProofIssue[]): ProofIssue[] {
  const done = new Set(applied.map((a) => a.id));
  return issues.map((i) => {
    if (done.has(i.id)) return i;
    const delta = applied
      .filter((a) => a.paragraph === i.paragraph && a.end <= i.start)
      .reduce((n, a) => n + [...(a.suggestion ?? "")].length - (a.end - a.start), 0);
    return delta ? { ...i, start: i.start + delta, end: i.end + delta } : i;
  });
}

/** The paragraph texts after `applied`, so the remaining issues show the right context. */
function rewrite(texts: Record<number, string>, applied: ProofIssue[]): Record<number, string> {
  const next = { ...texts };
  for (const a of [...applied].sort((x, y) => y.start - x.start)) {
    const chars = [...(next[a.paragraph] ?? "")];
    next[a.paragraph] = [...chars.slice(0, a.start), ...(a.suggestion ?? ""), ...chars.slice(a.end)].join("");
  }
  return next;
}

function Context({ text, issue }: { text: string | undefined; issue: ProofIssue }) {
  if (text === undefined) return null;
  const chars = [...text];
  if (issue.start === issue.end && issue.original === "") {
    const tail = chars.slice(Math.max(0, chars.length - 60)).join("");
    return <div className="proof-context">{chars.length > 60 ? `…${tail}` : tail}</div>;
  }
  const before = chars.slice(Math.max(0, issue.start - 30), issue.start).join("");
  const after = chars.slice(issue.end, issue.end + 30).join("");
  return (
    <div className="proof-context">
      {issue.start > 30 && "…"}
      {before}
      <del>{issue.original}</del>
      {issue.suggestion ? <ins>{issue.suggestion}</ins> : null}
      {after}
      {issue.end + 30 < chars.length && "…"}
    </div>
  );
}

function IssueRow({
  issue,
  text,
  state,
  busy,
  onApply,
  onIgnore,
  onJump,
}: {
  issue: ProofIssue;
  text: string | undefined;
  state: State;
  busy: boolean;
  /** Omit to show the finding without apply and ignore (while the run is going). */
  onApply?: () => void;
  onIgnore?: () => void;
  onJump: () => void;
}) {
  return (
    <div className={`proof-issue ${state}`}>
      <div className="proof-issue-head">
        <span className={`pill sm ${issue.severity === "error" ? "negative" : "warn"}`}>{LABEL[issue.category]}</span>
        <span className="muted">第 {issue.paragraph + 1} 段</span>
        <span className="muted">{issue.source === "model" ? "模型" : "规则"}</span>
        <span className="spacer" />
        {state === "open" ? (
          <>
            <button type="button" className="btn sm ghost" title="在文档中定位" onClick={onJump}>
              <Crosshair size={13} /> 定位
            </button>
            {onIgnore && (
              <button type="button" className="btn sm ghost" onClick={onIgnore}>
                <EyeOff size={13} /> 忽略
              </button>
            )}
            {onApply && (
              <button
                type="button"
                className="btn sm primary"
                disabled={busy || issue.suggestion === null}
                title={issue.suggestion === null ? "这一条需要你判断后手动修改" : "写入文档（按设置作为修订或直接修改）"}
                onClick={onApply}
              >
                <Check size={13} /> 应用
              </button>
            )}
          </>
        ) : (
          <span className="muted">{state === "applied" ? "已应用" : "已忽略"}</span>
        )}
      </div>
      <Context text={text} issue={issue} />
      <div className="proof-reason">{issue.reason}</div>
    </div>
  );
}

function CitationRow({ c, onJump, onSearch }: { c: ProofCitation; onJump: () => void; onSearch: () => void }) {
  const status = c.status ? STATUS[c.status.status] : null;
  const number = c.standardNo ?? c.docNo;
  return (
    <div className="proof-citation">
      <div className="proof-issue-head">
        <span className="citation-title">
          《{c.title}》{number && <span className="muted"> {number}</span>}
        </span>
        {status ? <span className={`pill sm ${status[1]}`}>{status[0]}</span> : <span className="pill sm">未核查</span>}
        <span className="spacer" />
        <span className="muted">引用 {c.paragraphs.length} 处</span>
        <button type="button" className="btn sm ghost" onClick={onJump}>
          <Crosshair size={13} /> 定位
        </button>
        <button type="button" className="btn sm ghost" title="联网查找现行版本，可下载到知识库" onClick={onSearch}>
          <Search size={13} /> 查找现行版本
        </button>
      </div>
      {c.status?.replacement && (
        <div className="proof-reason">
          现行版本：{c.status.replacement.title}
          {c.status.replacement.number && `（${c.status.replacement.number}）`}
        </div>
      )}
      {c.status?.reason && <div className="proof-reason muted">{c.status.reason}</div>}
    </div>
  );
}

function FactsCard({ facts, onChange }: { facts: ProjectFacts; onChange: (f: ProjectFacts) => void }) {
  return (
    <section className="card proof-facts">
      <div className="section-head">
        <h3>项目基本信息</h3>
        <span className="muted">从全文识别，用于检查张冠李戴；有误可直接修改后重新校对</span>
      </div>
      <div className="field-grid four">
        {FACT_FIELDS.map(([key, label]) => (
          <label key={key} className={`field${key === "name" || key === "owner" ? " wide" : ""}`}>
            <span className="field-label">{label}</span>
            <input className="input" value={facts[key] ?? ""} placeholder="未识别" onChange={(e) => onChange({ ...facts, [key]: e.target.value || null })} />
          </label>
        ))}
        {facts.others.map(([label, value], i) => (
          <label key={label} className="field">
            <span className="field-label">{label}</span>
            <input
              className="input"
              value={value}
              onChange={(e) => onChange({ ...facts, others: facts.others.map((o, j) => (j === i ? [o[0], e.target.value] : o)) })}
            />
          </label>
        ))}
      </div>
    </section>
  );
}

function Strategy() {
  const [open, setOpen] = useState(false);
  return (
    <section className="card proof-strategy">
      <button type="button" className="link-btn" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />} 校对策略
      </button>
      {open && (
        <ol>
          <li>规则先行：格式、序号、地名、常见错别字和引用清单由本机规则检查，快速且不花费额度；太短的段落（数字、单位、表格短单元格）不送模型，重复出现的段落只查一次。</li>
          <li>决策模型初筛（快速模式）：Jev 逐段判断是否可能有错，只有可疑段落交给大语言模型细读；判断结果随段落缓存。交终稿前可切到“逐段通读”。</li>
          <li>识别项目信息：从标题、封面和正文提取项目名称、所在省市区、建设单位，作为“张冠李戴”的比对基准。</li>
          <li>逐节通读：大语言模型按章节（约 3000 字一段）多节同时检查错别字、用词和与项目信息不符的表述，边查边显示；每条发现必须原文引用，否则丢弃。并发数和是否深度思考在设置 › AI 修复中调整。</li>
          <li>前后比对：本机抽取全文的总投资、面积、工期、长度、规模等关键指标，比对后再由模型确认是否真的矛盾。</li>
          <li>引用时效：对《》中的法律、标准和政策文件联网核查，已废止或被替代的给出现行版本，可一键查找并下载到知识库。</li>
          <li>增量复查：未改动的段落沿用上次结果，修改后再次校对只检查变化的章节。</li>
        </ol>
      )}
    </section>
  );
}

export const ProofreadPage = memo(function ProofreadPage({ backend, doc, section, settings, edit, onJump, onSearch, onOpenSettings, notify }: Props) {
  const [categories, setCategories] = useState<ProofCategory[]>(() => CATEGORIES.map(([k]) => k));
  const [useModel, setUseModel] = useState(true);
  const [range, setRange] = useState<Range>("all");
  const [depth, setDepth] = useState<"quick" | "full">("quick");
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<ProofProgress | null>(null);
  // Findings shown while a run is going.
  const [live, setLive] = useState<{ issues: ProofIssue[]; texts: Record<number, string> }>({ issues: [], texts: {} });
  const [report, setReport] = useState<ProofReport | null>(null);
  const [issues, setIssues] = useState<ProofIssue[]>([]);
  const [states, setStates] = useState<ReadonlyMap<string, State>>(() => new Map());
  const [facts, setFacts] = useState<ProjectFacts | null>(null);
  const [filter, setFilter] = useState<ProofCategory | "all">("all");
  const [showDone, setShowDone] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const docId = doc?.docId ?? null;

  // A new document starts over.
  useEffect(() => {
    setReport(null);
    setIssues([]);
    setStates(new Map());
    setFacts(null);
    setError(null);
  }, [docId]);

  useEffect(
    () =>
      backend.onProofreadProgress((p) => {
        if (p.docId !== docId) return;
        setProgress(p);
        const found = p.found ?? [];
        if (found.length > 0)
          setLive((prev) => {
            const seen = new Set(prev.issues.map((i) => i.id));
            const issues = [...prev.issues, ...found.filter((i) => !seen.has(i.id))].sort((a, b) => a.paragraph - b.paragraph || a.start - b.start);
            return { issues, texts: { ...prev.texts, ...p.paragraphs } };
          });
      }),
    [backend, docId],
  );

  const hasModel = !!settings?.roles.chat.model;
  // Screening needs Jev itself; a chat model standing in for it is no faster than reading.
  const canScreen = hasModel && !!settings?.roles.decision.model && settings.roles.decisionBackend === "jev";
  const run = async () => {
    if (docId === null) return;
    setRunning(true);
    setError(null);
    setProgress(null);
    setLive({ issues: [], texts: {} });
    try {
      const r = await backend.proofread(
        docId,
        { categories, useModel: useModel && hasModel, facts, screen: canScreen && depth === "quick" },
        range === "section" && section ? [section.start, Math.max(section.start, section.end - 1)] : null,
      );
      setReport(r);
      setIssues(r.issues);
      setStates(new Map());
      setFacts(r.facts);
      if (r.cancelled) notify("已停止，显示已完成部分的结果");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRunning(false);
      setProgress(null);
      setLive({ issues: [], texts: {} });
    }
  };

  const setState = (ids: string[], state: State) =>
    setStates((prev) => {
      const next = new Map(prev);
      ids.forEach((id) => next.set(id, state));
      return next;
    });

  const apply = useCallback(
    async (list: ProofIssue[]) => {
      if (docId === null || list.length === 0) return;
      setBusy(true);
      try {
        let applied: string[] = [];
        await edit(async () => {
          const r = await backend.applyProofIssues(docId, list);
          applied = r.applied;
          return r.outcome;
        });
        setState(applied, "applied");
        const done = list.filter((i) => applied.includes(i.id));
        setIssues((all) => shift(all, done));
        setReport((r) => (r ? { ...r, paragraphs: rewrite(r.paragraphs, done) } : r));
        const skipped = list.length - applied.length;
        notify(skipped > 0 ? `已应用 ${applied.length} 条，${skipped} 条原文已变化未应用` : `已应用 ${applied.length} 条，可撤销`);
      } catch (e) {
        notify(`应用失败：${errorMessage(e)}`, true);
      } finally {
        setBusy(false);
      }
    },
    [backend, docId, edit, notify],
  );

  const visible = useMemo(
    () => issues.filter((i) => (filter === "all" || i.category === filter) && (showDone || (states.get(i.id) ?? "open") === "open")),
    [issues, filter, showDone, states],
  );
  const applicable = visible.filter((i) => i.suggestion !== null && (states.get(i.id) ?? "open") === "open");
  const openCount = issues.filter((i) => (states.get(i.id) ?? "open") === "open").length;
  const counts = useMemo(() => {
    const c = new Map<ProofCategory, number>();
    for (const i of issues) if ((states.get(i.id) ?? "open") === "open") c.set(i.category, (c.get(i.category) ?? 0) + 1);
    return c;
  }, [issues, states]);

  if (!doc) {
    return (
      <div className="page scroll">
        <div className="page-inner">
          <div className="empty-state">
            <FileSearch size={34} strokeWidth={1.4} />
            <h2>先打开一份文档</h2>
            <p>文档校对在交稿前通读全文，检查错别字、序号、前后矛盾、张冠李戴、格式问题和引用文件的时效。</p>
          </div>
        </div>
      </div>
    );
  }

  const toggle = (c: ProofCategory) => setCategories((list) => (list.includes(c) ? list.filter((x) => x !== c) : [...list, c]));
  const citations = report?.citations ?? [];

  return (
    <div className="page scroll">
      <div className="page-inner">
        <header className="page-head">
          <div className="page-title">
            <h1>文档校对</h1>
            <div className="page-sub">
              {doc.fileName}
              {report &&
                ` · ${openCount} 条待处理 · ${report.sections} 节${report.cachedSections ? `（${report.cachedSections} 节沿用上次结果）` : ""} · 调用模型 ${formatNumber(report.modelCalls)} 次` +
                  (report.screenedOut ? ` · 决策模型初筛放过 ${formatNumber(report.screenedOut)} 段` : "")}
            </div>
          </div>
          <span className="spacer" />
          {running ? (
            <button type="button" className="btn" onClick={() => void backend.cancelProofread()}>
              <Square size={14} /> 停止
            </button>
          ) : (
            <button type="button" className="btn primary" disabled={categories.length === 0} onClick={() => void run()}>
              <Play size={14} /> {report ? "重新校对" : "开始校对"}
            </button>
          )}
        </header>

        <section className="card proof-options">
          <div className="proof-cats">
            {CATEGORIES.map(([key, label, hint]) => (
              <label key={key} className="check" title={hint}>
                <input type="checkbox" checked={categories.includes(key)} disabled={running} onChange={() => toggle(key)} /> {label}
              </label>
            ))}
          </div>
          <div className="proof-row">
            <div className="radio-group">
              <label className="radio">
                <input type="radio" checked={range === "all"} disabled={running} onChange={() => setRange("all")} /> 全文
              </label>
              <label className="radio" title={section?.title}>
                <input type="radio" checked={range === "section"} disabled={running || !section} onChange={() => setRange("section")} /> 当前章节
                {section && <span className="muted ellipsis section-name">（{section.title}）</span>}
              </label>
            </div>
            <span className="spacer" />
            {useModel && hasModel && (
              <div
                className="radio-group"
                title={canScreen ? undefined : "需要在设置 › 模型分配中配置决策模型 Jev（大模型代答的判定模型不能加速）"}
              >
                <label className="radio" title="决策模型先判断每段是否可疑，大语言模型只细读可疑段落，快好几倍">
                  <input type="radio" checked={canScreen && depth === "quick"} disabled={running || !canScreen} onChange={() => setDepth("quick")} /> 快速（Jev 初筛）
                </label>
                <label className="radio" title="大语言模型逐段细读全文，最全面，也最慢；适合交终稿前">
                  <input type="radio" checked={!canScreen || depth === "full"} disabled={running} onChange={() => setDepth("full")} /> 逐段通读
                </label>
              </div>
            )}
            <label className="check" title={hasModel ? "错别字、张冠李戴、前后矛盾需要大语言模型；关闭后只做规则检查" : "尚未配置大语言模型"}>
              <input type="checkbox" checked={useModel && hasModel} disabled={running || !hasModel} onChange={(e) => setUseModel(e.target.checked)} />
              使用大语言模型
            </label>
            {!hasModel && (
              <button type="button" className="link-btn" onClick={onOpenSettings}>
                去配置
              </button>
            )}
          </div>
        </section>

        <Strategy />

        {running && (
          <div className="card kb-progress">
            <div className="row">
              <span className="spinner sm" />
              <span className="label">{progress ? STAGE_TEXT[progress.stage] : "准备中"}</span>
              {progress && progress.total > 1 && (
                <span className="muted">
                  {progress.done} / {progress.total}
                </span>
              )}
              {live.issues.length > 0 && <span className="muted">· 已发现 {live.issues.length} 条，全部完成后可应用</span>}
            </div>
            <div className="progress">
              <span style={{ width: `${progress && progress.total ? (progress.done / progress.total) * 100 : 5}%` }} />
            </div>
          </div>
        )}
        {error && <div className="fix-note error">{error}</div>}
        {report?.failures.map((f) => (
          <div key={f} className="fix-note warn">
            {f}
          </div>
        ))}

        {facts && <FactsCard facts={facts} onChange={setFacts} />}

        {running && live.issues.length > 0 && (
          <section className="card proof-issues">
            <div className="section-head">
              <h3>问题</h3>
              <span className="muted">校对中，已发现 {live.issues.length} 条</span>
            </div>
            {live.issues.map((i) => (
              <IssueRow
                key={i.id}
                issue={i}
                text={live.texts[i.paragraph]}
                state="open"
                busy
                onJump={() => onJump(i.paragraph)}
              />
            ))}
          </section>
        )}

        {report && !(running && live.issues.length > 0) && (
          <section className="card proof-issues">
            <div className="section-head">
              <h3>问题</h3>
              <div className="proof-filters">
                <button type="button" className={`chip${filter === "all" ? " on" : ""}`} onClick={() => setFilter("all")}>
                  全部 {openCount}
                </button>
                {CATEGORIES.filter(([k]) => k !== "citation" && counts.get(k)).map(([k, label]) => (
                  <button key={k} type="button" className={`chip${filter === k ? " on" : ""}`} onClick={() => setFilter(k)}>
                    {label} {counts.get(k)}
                  </button>
                ))}
              </div>
              <span className="spacer" />
              <label className="check">
                <input type="checkbox" checked={showDone} onChange={(e) => setShowDone(e.target.checked)} /> 显示已处理
              </label>
              <button type="button" className="btn sm" disabled={busy || applicable.length === 0} onClick={() => void apply(applicable)}>
                <CheckCheck size={13} /> 全部应用（{applicable.length}）
              </button>
            </div>
            {visible.length === 0 && <div className="panel-empty">{issues.length === 0 ? "没有发现问题" : "当前筛选下没有待处理的问题"}</div>}
            {visible.map((i) => (
              <IssueRow
                key={i.id}
                issue={i}
                text={report.paragraphs[i.paragraph]}
                state={states.get(i.id) ?? "open"}
                busy={busy}
                onApply={() => void apply([i])}
                onIgnore={() => setState([i.id], "ignored")}
                onJump={() => onJump(i.paragraph)}
              />
            ))}
          </section>
        )}

        {report && citations.length > 0 && (
          <section className="card proof-issues">
            <div className="section-head">
              <h3>
                <BookMarked size={15} /> 引用文件
              </h3>
              <span className="muted">
                {citations.length} 份
                {citations.some((c) => c.status && c.status.status !== "current") &&
                  `，${citations.filter((c) => c.status && c.status.status !== "current" && c.status.status !== "unknown").length} 份已废止或被替代`}
              </span>
            </div>
            {citations.map((c) => (
              <CitationRow key={c.title} c={c} onJump={() => onJump(c.paragraphs[0])} onSearch={() => onSearch(c.status?.replacement?.title ?? `《${c.title}》 最新版`)} />
            ))}
          </section>
        )}
      </div>
    </div>
  );
});
