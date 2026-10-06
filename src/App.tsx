import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  IconClose,
  IconCopied,
  IconCopy,
  IconHistory,
  IconLayoutColumns,
  IconLayoutRows,
  IconPin,
  IconPinOff,
  IconSettings,
} from "./components/icons";
import {
  buildAlignment,
  mapAlignedSelection,
  offsetIn,
  sliceByRanges,
  type RawPair,
  type Span,
} from "./selectionAlign";

type Phase = "idle" | "streaming" | "done" | "error";
type Column = "src" | "dst";

interface TextSelection {
  sessionId: number;
  column: Column;
  range: Span;
}

type WordAlignmentState =
  | { kind: "idle" | "matched" | "unmatched" }
  | { kind: "pending"; attempt: number; maxAttempts: number }
  | { kind: "error"; message: string };

interface StatePayload {
  hotkey: string | null;
  panel_pinned: boolean;
  panel_wide: boolean;
}
interface LayoutPayload {
  wide: boolean;
}
interface StartPayload {
  session_id: number;
  source: string;
}
interface DeltaPayload {
  session_id: number;
  text: string;
}
interface DonePayload {
  session_id: number;
  full_text: string;
}
interface ErrorPayload {
  session_id: number;
  message: string;
}
interface AlignPayload {
  session_id: number;
  pairs: RawPair[];
}
interface HistoryStatusPayload {
  error: string | null;
}
interface SelectionAlignStatusPayload {
  session_id: number;
  column: Column;
  start: number;
  end: number;
  attempt: number;
  max_attempts: number;
}

const selectionRequestKey = ({ sessionId, column, range }: TextSelection) =>
  `${sessionId}:${column}:${range[0]}:${range[1]}`;

function alignmentErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (typeof error === "object" && error !== null && "message" in error &&
      typeof error.message === "string" && error.message.trim()) return error.message;
  return "词语对齐暂时不可用，请重试。";
}

function rangeInText(container: HTMLElement, [start, end]: Span): Range | null {
  const endpoint = (offset: number): [Node, number] | null => {
    const walker = container.ownerDocument.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node) {
      const length = node.textContent?.length ?? 0;
      if (offset <= length) return [node, offset];
      offset -= length;
      node = walker.nextNode();
    }
    return null;
  };
  const startPoint = endpoint(start);
  const endPoint = endpoint(end);
  if (!startPoint || !endPoint) return null;
  const range = container.ownerDocument.createRange();
  range.setStart(...startPoint);
  range.setEnd(...endPoint);
  return range;
}

export default function App() {
  const activeSession = useRef(0);
  const [phase, setPhase] = useState<Phase>("idle");
  const [source, setSource] = useState("");
  const [result, setResult] = useState("");
  const [message, setMessage] = useState("");
  const [copied, setCopied] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [hotkey, setHotkey] = useState<string | null>(null);
  const [pinned, setPinned] = useState(false);
  // 宽版 = 原文在左、译文在右。窗口尺寸是 Rust 在改，这里只负责按标志渲染。
  const [wide, setWide] = useState(false);

  // 原文 ↔ 译文联动
  const sourceRef = useRef<HTMLDivElement>(null);
  const resultRef = useRef<HTMLDivElement>(null);
  const alignmentFeedbackRef = useRef<HTMLDivElement>(null);
  const [rawPairs, setRawPairs] = useState<RawPair[]>([]);
  const [hlSrc, setHlSrc] = useState<Span[]>([]);
  const [hlDst, setHlDst] = useState<Span[]>([]);
  const [textSelection, setTextSelection] = useState<TextSelection | null>(null);
  const [selectionColumn, setSelectionColumn] = useState<Column | null>(null);
  const selectionRef = useRef<TextSelection | null>(null);
  const selectionVersion = useRef(0);
  const pointerSelecting = useRef(false);
  const selectionRequests = useRef(new Map<string, Promise<Span[]>>());
  const [retryRevision, setRetryRevision] = useState(0);
  const [wordAlignment, setWordAlignment] = useState<WordAlignmentState>({ kind: "idle" });
  // 对齐表是否已经出结果（成功或失败都算"有了结论"）
  const [alignReady, setAlignReady] = useState(false);

  // 把模型给的片段定位成字符区间；定位不到的会被丢掉
  const alignment = useMemo(
    () => buildAlignment(source, result, rawPairs),
    [source, result, rawPairs],
  );

  useEffect(() => {
    // 热键是候选链里第一个注册成功的那个，启动时才知道，因此要向 Rust 侧问一次
    void invoke<StatePayload>("get_state").then((state) => {
      setHotkey(state.hotkey);
      setPinned(state.panel_pinned);
      setWide(state.panel_wide);
    });

    const disposers = [
      listen<HistoryStatusPayload>("history://changed", (e) => setHistoryError(e.payload.error)),
      listen<HistoryStatusPayload>("history://status", (e) => setHistoryError(e.payload.error)),
      // 面板形态由 Rust 决定（它同时要改窗口尺寸），前端跟着渲染即可
      listen<LayoutPayload>("panel://layout", (e) => setWide(e.payload.wide)),
      listen<StartPayload>("translation://start", (e) => {
        if (e.payload.session_id < activeSession.current) return;
        activeSession.current = e.payload.session_id;
        setSource(e.payload.source);
        setResult("");
        setMessage("");
        setCopied(false);
        // 新一轮翻译：清掉上一轮的高亮与对齐数据
        setRawPairs([]);
        setAlignReady(false);
        setHlSrc([]);
        setHlDst([]);
        selectionRef.current = null;
        pointerSelecting.current = false;
        selectionVersion.current += 1;
        selectionRequests.current.clear();
        setTextSelection(null);
        setSelectionColumn(null);
        setWordAlignment({ kind: "idle" });
        setPhase("streaming");
      }),
      listen<AlignPayload>("translation://align", (e) => {
        if (e.payload.session_id !== activeSession.current) return;
        setRawPairs(e.payload.pairs);
        setAlignReady(true);
      }),
      listen<SelectionAlignStatusPayload>("translation://selection-align-status", (e) => {
        const status = e.payload;
        const current = selectionRef.current;
        if (pointerSelecting.current || status.session_id !== activeSession.current ||
            current?.sessionId !== status.session_id || current.column !== status.column ||
            current.range[0] !== status.start || current.range[1] !== status.end) return;
        if (!Number.isInteger(status.attempt) || !Number.isInteger(status.max_attempts) ||
            status.attempt < 1 || status.attempt > status.max_attempts) return;
        setWordAlignment((previous) => previous.kind === "pending"
          ? { kind: "pending", attempt: status.attempt, maxAttempts: status.max_attempts }
          : previous);
      }),
      listen<DeltaPayload>("translation://delta", (e) => {
        if (e.payload.session_id !== activeSession.current) return;
        setResult((prev) => prev + e.payload.text);
      }),
      listen<DonePayload>("translation://done", (e) => {
        if (e.payload.session_id !== activeSession.current) return;
        setResult(e.payload.full_text);
        setPhase("done");
      }),
      listen<ErrorPayload>("translation://error", (e) => {
        if (e.payload.session_id !== activeSession.current) return;
        setMessage(e.payload.message);
        setPhase("error");
      }),
    ];

    const onKeyDown = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") void invoke("close_panel");
    };
    window.addEventListener("keydown", onKeyDown);

    return () => {
      disposers.forEach((p) => void p.then((off) => off()));
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  // selectionchange 同时覆盖鼠标拖选、双击和键盘调整选区。
  useEffect(() => {
    const readSelection = (force = false) => {
      if (pointerSelecting.current) return;
      const selection = window.getSelection();
      let next: TextSelection | null = null;
      if (selection && selection.rangeCount > 0) {
        const range = selection.getRangeAt(0);
        // 查看或复制失败详情不应清掉待重试的原始选词；跨正文的选区仍正常处理。
        if (alignmentFeedbackRef.current?.contains(range.commonAncestorContainer)) return;
        if (!selection.isCollapsed) {
          for (const column of ["src", "dst"] as const) {
            const container = column === "src" ? sourceRef.current : resultRef.current;
            if (!container?.contains(range.commonAncestorContainer)) continue;
            const start = offsetIn(container, range.startContainer, range.startOffset);
            const end = offsetIn(container, range.endContainer, range.endOffset);
            if (start >= 0 && end > start) {
              next = { sessionId: activeSession.current, column, range: [start, end] };
            }
            break;
          }
        }
      }

      const previous = selectionRef.current;
      if (
        !force && previous?.sessionId === next?.sessionId &&
        previous?.column === next?.column &&
        previous?.range[0] === next?.range[0] &&
        previous?.range[1] === next?.range[1]
      ) return;

      selectionRef.current = next;
      selectionVersion.current += 1;
      setSelectionColumn(next?.column ?? null);
      setTextSelection(next);
    };
    const finishSelection = () => {
      const force = pointerSelecting.current;
      pointerSelecting.current = false;
      readSelection(force);
    };
    const onSelectionChange = () => readSelection();
    document.addEventListener("selectionchange", onSelectionChange);
    document.addEventListener("mouseup", finishSelection);
    window.addEventListener("blur", finishSelection);
    document.addEventListener("pointercancel", finishSelection);
    return () => {
      document.removeEventListener("selectionchange", onSelectionChange);
      document.removeEventListener("mouseup", finishSelection);
      window.removeEventListener("blur", finishSelection);
      document.removeEventListener("pointercancel", finishSelection);
    };
  }, []);

  useEffect(() => {
    // 拖选期间即使整段对齐表迟到，也不能根据上一次选区重绘当前正在操作的一栏。
    if (pointerSelecting.current) return;
    if (!textSelection) {
      setHlSrc([]);
      setHlDst([]);
      setWordAlignment({ kind: "idle" });
      return;
    }
    if (phase !== "done" || textSelection.sessionId !== activeSession.current) return;

    const { column, range, sessionId } = textSelection;
    // 只重绘另一栏，保留正在操作的原生选区。
    const highlight = column === "src" ? setHlDst : setHlSrc;
    const ranges = mapAlignedSelection(source, result, alignment, column, range);
    if (ranges !== null) {
      highlight(ranges);
      setWordAlignment({ kind: ranges.length ? "matched" : "unmatched" });
      return;
    }

    highlight([]);
    setWordAlignment({ kind: "pending", attempt: 1, maxAttempts: 3 });
    const version = selectionVersion.current;
    const key = selectionRequestKey(textSelection);
    let cancelled = false;
    // 等拖选/键盘调整停下再请求；重复选择复用正在进行或已完成的请求。
    const timer = window.setTimeout(() => {
      if (version !== selectionVersion.current || pointerSelecting.current) return;
      let request = selectionRequests.current.get(key);
      if (!request) {
        request = invoke<Span[]>("align_selection", {
          sessionId, source, translation: result, column, start: range[0], end: range[1],
        });
        selectionRequests.current.set(key, request);
        // 网络失败不缓存，用户重选时可以重试。
        void request.catch(() => {
          if (selectionRequests.current.get(key) === request) selectionRequests.current.delete(key);
        });
      }
      const isCurrent = () =>
        !cancelled && version === selectionVersion.current && sessionId === activeSession.current;
      void request.then((matched) => {
        if (!isCurrent()) return;
        highlight(matched);
        setWordAlignment({ kind: matched.length ? "matched" : "unmatched" });
      }).catch((error: unknown) => {
        if (!isCurrent()) return;
        setWordAlignment({ kind: "error", message: alignmentErrorMessage(error) });
      });
    }, 250);

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [textSelection, phase, source, result, alignment, retryRevision]);

  const retrySelectionAlignment = () => {
    const current = selectionRef.current;
    if (!current || pointerSelecting.current || phase !== "done" ||
        current.sessionId !== activeSession.current) return;
    const container = current.column === "src" ? sourceRef.current : resultRef.current;
    const selection = window.getSelection();
    const range = container ? rangeInText(container, current.range) : null;
    if (!selection || !range) return;
    // 重试会卸载反馈框，先把复制详情时的选区还原到原词，避免卸载让选区塌缩。
    selection.removeAllRanges();
    selection.addRange(range);
    selectionRequests.current.delete(selectionRequestKey(current));
    selectionVersion.current += 1;
    setWordAlignment({ kind: "pending", attempt: 1, maxAttempts: 3 });
    setTextSelection(current);
    setRetryRevision((revision) => revision + 1);
  };

  /**
   * 拖动面板。
   *
   * 用显式 API 而不是 `data-tauri-drag-region`：查过 tauri 2.12.1 的 Rust 源码，
   * 那个属性在这一版里根本没有被处理，靠不住。
   *
   * `startDragging()` 会一直阻塞到拖动结束才返回（内部走的是 SendMessage 的模态拖动循环），
   * 所以 await 之后固定位置正好是"用户松手"的时刻。
   */
  const onHeaderMouseDown = async (event: ReactMouseEvent) => {
    if (event.button !== 0) return;
    // 标题栏上的按钮不触发拖动
    if ((event.target as HTMLElement).closest("button")) return;

    await getCurrentWindow().startDragging();
    await invoke("pin_panel");
    setPinned(true);
  };

  const togglePin = async () => {
    if (pinned) {
      await invoke("unpin_panel");
      setPinned(false);
    } else {
      await invoke("pin_panel");
      setPinned(true);
    }
  };

  const copy = async () => {
    if (!result) return;
    await invoke("copy_result", { text: result });
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  };

  // 选中栏只隐藏标记的颜色，保留所有文本节点，让原生拖选和双击的端点保持有效。
  const prepareSelection = (column: Column) => {
    pointerSelecting.current = true;
    selectionVersion.current += 1;
    setSelectionColumn(column);
    if (column === "src") setHlDst([]);
    else setHlSrc([]);
    setWordAlignment({ kind: "idle" });
  };

  /** 把文本按高亮区间渲染成若干 span；没有高亮时就是一个整段。 */
  const renderSlices = (text: string, ranges: Span[]) =>
    sliceByRanges(text, ranges).map((slice, index) => (
      <span key={index} className={slice.hit ? "hl" : undefined}>
        {slice.text}
      </span>
    ));

  // 状态点的文字说明。纯彩色圆点没人看得懂，必须自带标注。
  const phaseLabel = {
    idle: "等待划词",
    streaming: "正在翻译",
    done: "翻译完成",
    error: "翻译出错",
  }[phase];

  // 只有"原文 + 译文"同时在时才排两栏。
  // 否则（例如还没配 Key 时同时出现"原文 + 错误"）网格自动排布会把错误挤到下一行，
  // 宽窗口里单栏反而更清楚。
  const twoColumn = wide && Boolean(source) && (phase === "streaming" || phase === "done");

  /*
   * 把对齐状态直接显示出来。
   * 出问题时这一眼就能区分两种完全不同的原因：
   *   「对齐分析中…」= 还没回来；「对齐不可用」= 模型没给出能定位的对齐表。
   */
  const alignmentLabels = {
    error: "词语对齐失败",
    unmatched: "未找到对应词语",
    matched: "词语已对齐",
  };
  const alignBadgeText = wordAlignment.kind === "pending"
    ? wordAlignment.attempt > 1
      ? `重试对齐 ${wordAlignment.attempt}/${wordAlignment.maxAttempts}…`
      : "词语对齐中…"
    : wordAlignment.kind !== "idle"
      ? alignmentLabels[wordAlignment.kind]
    : !alignReady
      ? "对齐分析中…"
      : alignment.length > 0 ? `对齐 ${alignment.length} 段` : "对齐不可用";
  const alignBadgeClass =
    wordAlignment.kind === "error" || wordAlignment.kind === "unmatched" ||
    (wordAlignment.kind === "idle" && alignReady && alignment.length === 0)
      ? "align-hint align-hint-warn" : "align-hint";
  const alignBadgeTitle =
    wordAlignment.kind === "error"
      ? wordAlignment.message
      : alignment.length > 0
      ? `${rawPairs.length} 对里有 ${alignment.length} 对能在原文/译文里定位到`
      : "选中文字后会单独查找对应词语；没有可靠对应时不显示高亮";
  const alignmentFeedback = wordAlignment.kind === "error"
    ? wordAlignment.message
    : wordAlignment.kind === "unmatched"
      ? "没有找到可确认的对应词语，译文可能有所改写或省略。可以重试，或扩大选区。"
      : null;

  return (
    <div className="card">
      <header className="bar" onMouseDown={(e) => void onHeaderMouseDown(e)} title="按住可拖动面板">
        <span className="title">WordRay</span>
        <span
          className={"dot dot-" + phase}
          role="status"
          aria-label={phaseLabel}
          title={phaseLabel}
        />
        <button
          className={"icon-btn" + (wide ? " is-on" : "")}
          title={wide ? "切换为竖排（原文在上、译文在下）" : "切换为左右分栏（原文在左、译文在右）"}
          aria-label="切换面板布局"
          aria-pressed={wide}
          onClick={() => void invoke("toggle_panel_layout")}
        >
          {wide ? <IconLayoutRows /> : <IconLayoutColumns />}
        </button>
        <button
          className={"icon-btn" + (pinned ? " is-on" : "")}
          title={pinned ? "位置已固定，点击恢复跟随选区" : "固定当前位置"}
          aria-label={pinned ? "恢复跟随选区" : "固定当前位置"}
          aria-pressed={pinned}
          onClick={() => void togglePin()}
        >
          {pinned ? <IconPinOff /> : <IconPin />}
        </button>
        <button
          className="icon-btn"
          title="隐藏到托盘 (Esc)"
          aria-label="隐藏到托盘"
          onClick={() => void invoke("close_panel")}
        >
          <IconClose />
        </button>
      </header>

      <section className={"body" + (twoColumn ? " is-wide" : "")}>
        {phase === "idle" && (
          <p className="hint">
            {hotkey
              ? `在任意应用里选中中文，然后按 ${hotkey}`
              : "没有可用的全局热键：候选键都被其他程序占用了"}
          </p>
        )}

        {phase !== "idle" && source && (
          <div className="block block-source">
            <div className="label">原文</div>
            <div
              className={"source" + (selectionColumn === "src" ? " is-selecting" : "")}
              ref={sourceRef}
              onMouseDown={() => prepareSelection("src")}
              title="选中这里的一段文字，右侧会高亮对应的译文"
            >
              {renderSlices(source, hlSrc)}
            </div>
          </div>
        )}

        {phase === "error" && <div className="error">{message}</div>}

        {(phase === "streaming" || phase === "done") && (
          <div className="block block-result">
            <div className="label">
              译文{phase === "streaming" && <span className="caret">▌</span>}
              {phase === "done" && (
                <span className={alignBadgeClass} title={alignBadgeTitle} role="status">
                  {alignBadgeText}
                </span>
              )}
            </div>
            {phase === "done" && alignmentFeedback && (
              <div className="alignment-feedback" ref={alignmentFeedbackRef}>
                <p
                  className="alignment-feedback-message"
                  role={wordAlignment.kind === "error" ? "alert" : "status"}
                >
                  {alignmentFeedback}
                </p>
                <button
                  type="button"
                  className="alignment-retry"
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={retrySelectionAlignment}
                  aria-label="重试当前选区的词语对齐"
                >
                  重试
                </button>
              </div>
            )}
            <div
              className={"result" + (selectionColumn === "dst" ? " is-selecting" : "")}
              ref={resultRef}
              onMouseDown={() => prepareSelection("dst")}
              title="选中这里的一段文字，左侧会高亮对应的原文"
            >
              {result ? renderSlices(result, hlDst) : "…"}
            </div>
          </div>
        )}
      </section>

      {historyError && (
        <p className="history-notice" role="status">历史记录暂未保存：{historyError}</p>
      )}

      <footer className="foot">
        <div className="foot-actions">
          <button
            className={"icon-btn" + (copied ? " is-ok" : "")}
            title={copied ? "已复制" : "复制译文"}
            aria-label="复制译文"
            onClick={() => void copy()}
            disabled={!result}
          >
            {copied ? <IconCopied /> : <IconCopy />}
          </button>
          <button
            className="icon-btn"
            title="设置"
            aria-label="设置"
            onClick={() => void invoke("open_settings")}
          >
            <IconSettings />
          </button>
          <button
            className="icon-btn"
            title="历史记录（最近 50 条）"
            aria-label="历史记录"
            onClick={() => void invoke("open_history")}
          >
            <IconHistory />
          </button>
        </div>
        <span className="foot-hint">{pinned ? "位置已固定" : "Esc 隐藏到托盘"}</span>
      </footer>
    </div>
  );
}
