import type { DiffSpan } from "../types";

/** Beyond this many edits the diff degrades to "delete everything, insert everything". */
const MAX_EDITS = 2000;

/** Character-level diff (Myers), grouped so each change reads as one deletion followed by one insertion. */
export function diffChars(a: string, b: string): DiffSpan[] {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const ops: DiffSpan[] = [];
  if (start > 0) ops.push({ kind: "equal", text: a.slice(0, start) });
  ops.push(...myers(a.slice(start, endA), b.slice(start, endB)));
  if (endA < a.length) ops.push({ kind: "equal", text: a.slice(endA) });
  return cleanup(ops);
}

function myers(a: string, b: string): DiffSpan[] {
  const n = a.length;
  const m = b.length;
  if (n === 0) return m ? [{ kind: "insert", text: b }] : [];
  if (m === 0) return [{ kind: "delete", text: a }];
  const max = Math.min(n + m, MAX_EDITS);
  const off = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[off + k] = x;
      if (x >= n && y >= m) return backtrack(trace, a, b, off);
    }
  }
  return [
    { kind: "delete", text: a },
    { kind: "insert", text: b },
  ];
}

function backtrack(trace: Int32Array[], a: string, b: string, off: number): DiffSpan[] {
  const reversed: DiffSpan[] = [];
  let x = a.length;
  let y = b.length;
  for (let d = trace.length - 1; d >= 0; d--) {
    const v = trace[d];
    const k = x - y;
    const prevK = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? k + 1 : k - 1;
    const prevX = v[off + prevK];
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      reversed.push({ kind: "equal", text: a[x - 1] });
      x--;
      y--;
    }
    if (d > 0) {
      if (x === prevX) reversed.push({ kind: "insert", text: b[y - 1] });
      else reversed.push({ kind: "delete", text: a[x - 1] });
    }
    x = prevX;
    y = prevY;
  }
  return reversed.reverse();
}

/**
 * Merges runs, puts each change's deletion before its insertion and folds tiny
 * equal islands (one or two characters between changes) into the change, so a
 * rewritten phrase does not show up as confetti.
 */
function cleanup(ops: DiffSpan[]): DiffSpan[] {
  let groups = group(ops);
  for (;;) {
    const i = groups.findIndex(
      (g, at) => g.kind === "equal" && g.text.length <= 2 && at > 0 && at < groups.length - 1,
    );
    if (i < 0) break;
    const merged: DiffSpan[] = [...groups.slice(0, i), { kind: "delete", text: groups[i].text }, { kind: "insert", text: groups[i].text }, ...groups.slice(i + 1)];
    groups = group(merged);
  }
  for (let i = 1; i < groups.length - 1; i++) {
    if (groups[i - 1].kind === "equal" && groups[i].kind !== "equal" && groups[i + 1].kind === "equal") {
      alignToPunctuation(groups[i - 1], groups[i], groups[i + 1]);
    }
  }
  return groups.filter((g) => g.text);
}

const PUNCTUATION = /[。，、；：！？,.;:!?）)]/;

/**
 * A pure insertion or deletion can often slide along repeated text
 * ("第|二句。第|三句" = "|第二句。|第三句"); prefer the spot right after punctuation.
 */
function alignToPunctuation(before: DiffSpan, edit: DiffSpan, after: DiffSpan) {
  let left = 0;
  while (left < before.text.length && before.text[before.text.length - 1 - left] === edit.text[edit.text.length - 1 - (left % edit.text.length)]) left++;
  let right = 0;
  while (right < after.text.length && after.text[right] === edit.text[right % edit.text.length]) right++;
  const all = before.text + edit.text + after.text;
  const start = before.text.length;
  let best = start;
  let bestScore = -1;
  for (let at = start - left; at <= start + right; at++) {
    const score = (at === 0 || PUNCTUATION.test(all[at - 1]) ? 2 : 0) + (PUNCTUATION.test(all[at + edit.text.length - 1]) ? 1 : 0);
    if (score > bestScore) {
      best = at;
      bestScore = score;
    }
  }
  before.text = all.slice(0, best);
  edit.text = all.slice(best, best + edit.text.length);
  after.text = all.slice(best + edit.text.length);
}

function group(ops: DiffSpan[]): DiffSpan[] {
  const out: DiffSpan[] = [];
  let del = "";
  let ins = "";
  const flush = () => {
    if (del) out.push({ kind: "delete", text: del });
    if (ins) out.push({ kind: "insert", text: ins });
    del = ins = "";
  };
  for (const op of ops) {
    if (!op.text) continue;
    if (op.kind === "delete") del += op.text;
    else if (op.kind === "insert") ins += op.text;
    else {
      flush();
      const last = out[out.length - 1];
      if (last?.kind === "equal") last.text += op.text;
      else out.push({ kind: "equal", text: op.text });
    }
  }
  flush();
  return out;
}
