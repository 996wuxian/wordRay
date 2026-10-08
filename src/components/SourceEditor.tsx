import { useCallback, useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { sliceByRanges, type Span } from "../selectionAlign";

interface SourceEditorProps {
  value: string;
  resetKey: number;
  ranges: Span[];
  selecting: boolean;
  elementRef: RefObject<HTMLDivElement | null>;
  onChange: (text: string) => void;
  onCommit: (text: string) => void;
  onPrepareSelection: () => void;
}

type TextSlice = ReturnType<typeof sliceByRanges>[number];
const normalizeNewlines = (text: string) => text.replace(/\r\n?/g, "\n");

/** Chromium inserts DIVs and caret BRs for Enter, which innerText counts twice. */
function readEditableText(element: HTMLElement): string {
  if (!element.querySelector("div, p, br")) return normalizeNewlines(element.innerText);
  const children = [...element.childNodes].filter((node) =>
    node.nodeType === Node.ELEMENT_NODE || (node.nodeType === Node.TEXT_NODE && node.textContent),
  );
  let text = "";
  let previousBlock = false;
  let hasPart = false;
  for (const [index, node] of children.entries()) {
    const child = node instanceof HTMLElement ? node : null;
    const block = child?.tagName === "DIV" || child?.tagName === "P";
    // A final BR holds the caret on an empty line; preceding BRs are actual line breaks.
    if (child?.tagName === "BR" && index === children.length - 1) continue;
    const part = child?.tagName === "BR"
      ? "\n"
      : child ? readEditableText(child) : normalizeNewlines(node.textContent ?? "");
    if (hasPart && (previousBlock || block)) text += "\n";
    text += part;
    previousBlock = block;
    hasPart = true;
  }
  return text;
}

function hasTextSelection(element: HTMLElement): boolean {
  const selection = element.ownerDocument.getSelection();
  return !!selection && !selection.isCollapsed && selection.rangeCount > 0 &&
    element.contains(selection.getRangeAt(0).commonAncestorContainer);
}

/** Resolve a caret through editing line elements before converting them to text nodes. */
function editableOffset(element: HTMLElement, node: Node, offset: number): number | null {
  const path: number[] = [];
  for (let current = node; current !== element;) {
    const parent = current.parentNode;
    if (!parent) return null;
    path.unshift([...parent.childNodes].findIndex((child) => child === current));
    current = parent;
  }
  const clone = element.cloneNode(true);
  if (!(clone instanceof HTMLElement)) return null;
  let endpoint: Node = clone;
  for (const index of path) endpoint = endpoint.childNodes[index];
  let marker = "\u0000wordray-caret\u0000";
  while (element.textContent?.includes(marker)) marker += "\u0000";
  const markerNode = element.ownerDocument.createTextNode(marker);
  if (endpoint instanceof Text) {
    const remainder = endpoint.splitText(offset);
    remainder.parentNode?.insertBefore(markerNode, remainder);
  } else endpoint.insertBefore(markerNode, endpoint.childNodes[offset] ?? null);
  const position = readEditableText(clone).indexOf(marker);
  return position < 0 ? null : position;
}

function textPoint(element: HTMLElement, offset: number): [Node, number] {
  const walker = element.ownerDocument.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const length = node.textContent?.length ?? 0;
    if (offset <= length) return [node, offset];
    offset -= length;
  }
  return [element, element.childNodes.length];
}

export default function SourceEditor(props: SourceEditorProps) {
  const { value, resetKey, ranges, selecting, elementRef, onPrepareSelection } = props;
  const latest = useRef(props);
  const lastReset = useRef<number | null>(null);
  const rendered = useRef<TextSlice[] | null>(null);
  const composing = useRef(false);
  const pendingCommit = useRef(false);

  const paint = useCallback((text: string, highlights: Span[]) => {
    const element = elementRef.current;
    if (!element) return;
    element.dataset.empty = String(text.length === 0);
    const slices = sliceByRanges(text, highlights);
    if (rendered.current?.length === slices.length && slices.every((slice, index) =>
      slice.text === rendered.current?.[index].text && slice.hit === rendered.current[index].hit,
    )) return;
    const fragment = element.ownerDocument.createDocumentFragment();
    for (const slice of slices) {
      if (slice.hit) {
        const span = element.ownerDocument.createElement("span");
        span.className = "hl";
        span.textContent = slice.text;
        fragment.append(span);
      } else {
        fragment.append(element.ownerDocument.createTextNode(slice.text));
      }
    }
    element.replaceChildren(fragment);
    rendered.current = slices;
  }, [elementRef]);

  const commit = useCallback(() => {
    const element = elementRef.current;
    if (!element) return;
    if (composing.current) {
      pendingCommit.current = true;
      return;
    }
    const text = readEditableText(element);
    // Window blur retains activeElement. Normalize line elements there too, restoring the caret.
    if (element.querySelector("div, p, br")) {
      const selection = element.ownerDocument.getSelection();
      const anchor = selection?.anchorNode && element.contains(selection.anchorNode)
        ? editableOffset(element, selection.anchorNode, selection.anchorOffset) : null;
      const focus = selection?.focusNode && element.contains(selection.focusNode)
        ? editableOffset(element, selection.focusNode, selection.focusOffset) : null;
      paint(text, []);
      if (selection && anchor !== null && focus !== null) {
        selection.setBaseAndExtent(
          ...textPoint(element, Math.min(anchor, text.length)),
          ...textPoint(element, Math.min(focus, text.length)),
        );
      }
    } else if (element.ownerDocument.activeElement !== element && !hasTextSelection(element)) {
      paint(text, text === latest.current.value ? latest.current.ranges : []);
    }
    latest.current.onCommit(text);
  }, [elementRef, paint]);

  useLayoutEffect(() => {
    latest.current = props;
    const element = elementRef.current;
    if (!element) return;
    const reset = lastReset.current !== resetKey;
    lastReset.current = resetKey;
    if (reset) {
      composing.current = false;
      pendingCommit.current = false;
      rendered.current = null;
    } else if (composing.current || element.ownerDocument.activeElement === element ||
        hasTextSelection(element)) {
      return;
    }
    paint(value, ranges);
  });

  useEffect(() => {
    const onWindowBlur = () => {
      const element = elementRef.current;
      if (element && element.ownerDocument.activeElement === element) commit();
    };
    window.addEventListener("blur", onWindowBlur);
    return () => window.removeEventListener("blur", onWindowBlur);
  }, [elementRef, commit]);

  const reportChange = () => {
    const element = elementRef.current;
    if (!element) return;
    rendered.current = null;
    const text = readEditableText(element);
    element.dataset.empty = String(text.length === 0);
    latest.current.onChange(text);
  };

  return (
    <div
      ref={elementRef}
      className={"source source-editor" + (selecting ? " is-selecting" : "")}
      contentEditable="plaintext-only"
      suppressContentEditableWarning
      role="textbox"
      aria-multiline="true"
      aria-labelledby="source-label"
      aria-describedby="source-editor-hint"
      tabIndex={0}
      data-placeholder="输入或粘贴文本…"
      data-empty={value.length === 0 ? "true" : "false"}
      onMouseDown={onPrepareSelection}
      onInput={reportChange}
      onBlur={commit}
      onCompositionStart={() => { composing.current = true; }}
      onCompositionEnd={() => {
        composing.current = false;
        reportChange();
        if (pendingCommit.current) {
          pendingCommit.current = false;
          commit();
        }
      }}
    />
  );
}
