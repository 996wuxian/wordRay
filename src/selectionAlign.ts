/**
 * 原文 ↔ 译文的选中联动。
 *
 * 模型给的是**字面片段**（`src` / `dst` 都声明为原文/译文的连续子串），
 * 这里负责把它们定位成字符区间，再把"在一侧选中的范围"映射到另一侧。
 *
 * 内部偏移都按 **UTF-16 码元**计，与 JS 字符串索引、DOM 文本节点的 Range 偏移一致，
 * 不要混入码点计数。
 */

export type Span = [number, number];

export interface RawPair {
  src: string;
  dst: string;
}

export interface AlignedPair {
  src: Span;
  dst: Span;
}

/**
 * 把模型给的片段定位成区间。
 *
 * **src 从上次匹配的末尾继续往后找**（单调前进）：中文切分是有序的，
 * 不这样重复字词会全部指向第一次出现的位置。
 *
 * **dst 允许回退到全局查找**：英文语序与中文不同（「以后」在中间，对应的
 * "later" 可能在句尾），如果也强制单调前进，这类对会因为"位置更靠前"而整对丢失。
 * 所以先按当前位置找，找不到再全局找一次。
 *
 * **定位不到的对直接跳过**：模型不守"必须是连续子串"的约定是常态，
 * 与其猜，不如让这一对不参与高亮。
 */
export function buildAlignment(source: string, result: string, pairs: RawPair[]): AlignedPair[] {
  const out: AlignedPair[] = [];
  let srcCursor = 0;
  let dstCursor = 0;

  for (const pair of pairs) {
    const src = pair.src ?? "";
    const dst = pair.dst ?? "";
    if (!src || !dst) continue;

    const s = source.indexOf(src, srcCursor);
    if (s < 0) continue;

    let d = result.indexOf(dst, dstCursor);
    if (d < 0) d = result.indexOf(dst);
    if (d < 0) continue;

    out.push({ src: [s, s + src.length], dst: [d, d + dst.length] });
    srcCursor = s + src.length;
    // 只有真的往后走了才推进游标，避免把游标拽回去
    if (d >= dstCursor) dstCursor = d + dst.length;
  }

  return out;
}

export function overlaps(range: Span, start: number, end: number): boolean {
  return range[0] < end && start < range[1];
}

function splitsSurrogate(text: string, offset: number): boolean {
  if (offset <= 0 || offset >= text.length) return false;
  const before = text.charCodeAt(offset - 1);
  const after = text.charCodeAt(offset);
  return before >= 0xd800 && before <= 0xdbff && after >= 0xdc00 && after <= 0xdfff;
}

function validSpan(text: string, [start, end]: Span): boolean {
  return Number.isInteger(start) && Number.isInteger(end)
    && start >= 0 && start < end && end <= text.length
    && !splitsSurrogate(text, start) && !splitsSurrogate(text, end);
}

const latinWordCharacter = /[\p{Script=Latin}\p{M}\p{N}_]/u;

function characterBefore(text: string, offset: number): string {
  if (offset <= 0) return "";
  const last = text.charCodeAt(offset - 1);
  const previous = text.charCodeAt(offset - 2);
  const surrogatePair = last >= 0xdc00 && last <= 0xdfff && previous >= 0xd800 && previous <= 0xdbff;
  return text.slice(offset - (surrogatePair ? 2 : 1), offset);
}

function characterAfter(text: string, offset: number): string {
  const point = text.codePointAt(offset);
  return point === undefined ? "" : String.fromCodePoint(point);
}

/** 中文词项可以紧邻其他字；拉丁字母的词项不能截断单词或缩写。 */
function cutsLatinWord(text: string, offset: number): boolean {
  const before = characterBefore(text, offset);
  const after = characterAfter(text, offset);
  if (latinWordCharacter.test(before) && latinWordCharacter.test(after)) return true;
  if ((before === "'" || before === "’") && latinWordCharacter.test(after)) {
    return latinWordCharacter.test(characterBefore(text, offset - before.length));
  }
  if ((after === "'" || after === "’") && latinWordCharacter.test(before)) {
    return latinWordCharacter.test(characterAfter(text, offset + after.length));
  }
  return false;
}

function validAlignedSpan(text: string, range: Span): boolean {
  return validSpan(text, range) && !cutsLatinWord(text, range[0]) && !cutsLatinWord(text, range[1]);
}

function mergeRanges(ranges: Span[]): Span[] {
  const sorted = ranges.map(([start, end]): Span => [start, end])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged: Span[] = [];
  for (const range of sorted) {
    const previous = merged[merged.length - 1];
    if (previous && range[0] <= previous[1]) previous[1] = Math.max(previous[1], range[1]);
    else merged.push(range);
  }
  return merged;
}

function repeatedFragment(text: string, [start, end]: Span): boolean {
  const fragment = text.slice(start, end);
  return text.indexOf(fragment) !== text.lastIndexOf(fragment);
}

/**
 * 只把完整、已定位的词项映射到另一侧；片段内部没有可靠的字符比例关系。
 *
 * 返回 null 表示需要进一步语义对齐：选中了某个片段的一部分，或选中的词
 * 没有被现有对齐覆盖，或重复片段的具体出现位置不确定。空白和标点间隙不影响
 * 覆盖，不连续的译文区间保留间隙。
 */
export function mapAlignedSelection(
  source: string,
  result: string,
  alignment: AlignedPair[],
  column: "src" | "dst",
  selection: Span,
): Span[] | null {
  const selectedText = column === "src" ? source : result;
  if (!validSpan(selectedText, selection)) return null;

  const targetColumn = column === "src" ? "dst" : "src";
  const selectedRanges: Span[] = [];
  const targetRanges: Span[] = [];
  for (const pair of alignment) {
    if (!validAlignedSpan(source, pair.src) || !validAlignedSpan(result, pair.dst)) continue;
    const range = pair[column];
    if (!overlaps(range, selection[0], selection[1])) continue;
    if (selection[0] > range[0] || selection[1] < range[1]) return null;
    // indexOf 找到的字符位置不代表模型确认了重复词的具体 occurrence。
    // 尤其译文允许语序回退时，多项可能都落到第一次出现的位置。
    if (repeatedFragment(source, pair.src) || repeatedFragment(result, pair.dst)) return null;
    selectedRanges.push(range);
    targetRanges.push(pair[targetColumn]);
  }
  if (!targetRanges.length) return null;

  const coverage = mergeRanges(selectedRanges);
  let coveredIndex = 0;
  let offset = selection[0];
  for (const character of selectedText.slice(selection[0], selection[1])) {
    const end = offset + character.length;
    if (!/[\p{White_Space}\p{P}]/u.test(character)) {
      while (coveredIndex < coverage.length && coverage[coveredIndex][1] <= offset) coveredIndex += 1;
      const range = coverage[coveredIndex];
      if (!range || range[0] > offset || range[1] < end) return null;
    }
    offset = end;
  }
  return mergeRanges(targetRanges);
}

/**
 * 读取 DOM 选区在容器文本里的字符偏移。
 *
 * 容器里可能已经因为高亮被切成多个文本节点（我们自己的渲染结果），
 * DOM Range 的文本节点偏移是 UTF-16，元素节点偏移是子节点索引。
 * 使用从容器开头到端点的 Range，让 DOM 统一处理两种端点。
 */
export function offsetIn(container: HTMLElement, node: Node, offset: number): number {
  if (!container.contains(node) || !Number.isInteger(offset) || offset < 0) return -1;
  const prefix = container.ownerDocument.createRange();
  try {
    prefix.selectNodeContents(container);
    prefix.setEnd(node, offset);
    return prefix.toString().length;
  } catch {
    // 非法端点（如超出子节点数量）不能当成字符位置。
    return -1;
  }
}

/** 把文本按高亮区间切成片段，供渲染使用。 */
export function sliceByRanges(text: string, ranges: Span[]): { text: string; hit: boolean }[] {
  if (!text) return [];
  if (!ranges.length) return [{ text, hit: false }];

  const points = new Set<number>([0, text.length]);
  for (const [start, end] of ranges) {
    if (start > 0 && start < text.length) points.add(start);
    if (end > 0 && end < text.length) points.add(end);
  }

  const sorted = [...points].sort((a, b) => a - b);
  const slices: { text: string; hit: boolean }[] = [];

  for (let i = 0; i < sorted.length - 1; i += 1) {
    const from = sorted[i];
    const to = sorted[i + 1];
    if (to <= from) continue;
    const hit = ranges.some(([start, end]) => start <= from && to <= end);
    slices.push({ text: text.slice(from, to), hit });
  }

  return slices;
}
