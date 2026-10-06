import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { IconBusy, IconCopied, IconCopy } from "./components/icons";

interface HistoryEntry {
  id: string;
  created_at: number;
  source: string;
  translation: string;
}

interface HistoryPayload {
  entries: HistoryEntry[];
  limit: number;
  error: string | null;
}

type LoadState = "loading" | "ready" | "error";

const dateFormat = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
});

function HistoryItem({ entry, onOpen, opening }: {
  entry: HistoryEntry;
  onOpen: (id: string) => void;
  opening: boolean;
}) {
  const [copyState, setCopyState] = useState<"idle" | "busy" | "copied">("idle");
  const [copyError, setCopyError] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout>>();
  const active = useRef(true);

  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
      clearTimeout(timer.current);
    };
  }, []);

  const copy = async () => {
    clearTimeout(timer.current);
    setCopyError("");
    setCopyState("busy");
    try {
      await invoke("copy_result", { text: entry.translation });
      if (!active.current) return;
      setCopyState("copied");
      timer.current = setTimeout(() => setCopyState("idle"), 1600);
    } catch (error) {
      if (!active.current) return;
      setCopyState("idle");
      setCopyError(String(error));
    }
  };

  const date = new Date(entry.created_at);
  return (
    <li className="history-item">
      <article>
        <div className="history-item-bar">
          <time dateTime={date.toISOString()} title={date.toLocaleString("zh-CN")}>
            {dateFormat.format(date)}
          </time>
          <div className="history-item-actions">
            <button
              className="history-icon-button"
              aria-label={copyState === "copied" ? "已复制译文" : "复制译文"}
              title={copyState === "copied" ? "已复制译文" : "复制译文"}
              disabled={copyState === "busy"}
              onClick={() => void copy()}
            >
              {copyState === "busy" ? <IconBusy /> : copyState === "copied" ? <IconCopied /> : <IconCopy />}
            </button>
            <button className="history-open-button" disabled={opening} onClick={() => onOpen(entry.id)}>
              {opening ? <IconBusy /> : null}
              在面板中查看
            </button>
          </div>
        </div>
        <h2 className="history-text-label">原文</h2>
        <p className="history-source">{entry.source}</p>
        <h2 className="history-text-label">译文</h2>
        <p className="history-translation">{entry.translation}</p>
        {copyError && <p className="history-action-error" role="alert">复制失败：{copyError}</p>}
      </article>
    </li>
  );
}

export function HistoryView() {
  const [history, setHistory] = useState<HistoryPayload>({ entries: [], limit: 50, error: null });
  const [loadState, setLoadState] = useState<LoadState>("loading");
  const [loadError, setLoadError] = useState("");
  const [eventError, setEventError] = useState("");
  const [openingId, setOpeningId] = useState<string | null>(null);
  const [openError, setOpenError] = useState("");
  const requestVersion = useRef(0);
  const active = useRef(true);

  const reload = useCallback(async () => {
    const version = ++requestVersion.current;
    setLoadState("loading");
    try {
      const payload = await invoke<HistoryPayload>("get_history");
      if (!active.current || version !== requestVersion.current) return;
      setHistory(payload);
      setLoadState("ready");
      setLoadError("");
    } catch (error) {
      if (!active.current || version !== requestVersion.current) return;
      setLoadState("error");
      setLoadError(String(error));
    }
  }, []);

  useEffect(() => {
    active.current = true;
    let disposed = false;
    const subscriptions = ["history://changed", "history://refresh"].map((event) =>
      listen(event, () => { if (!disposed) void reload(); }).catch((error: unknown) => {
        if (!disposed) setEventError(`无法接收历史更新：${String(error)}`);
        return () => {};
      }),
    );
    // 先订阅，再读取快照，避免首次打开时漏掉恰好完成的翻译。
    void Promise.all(subscriptions).then(() => { if (!disposed) void reload(); });
    return () => {
      disposed = true;
      active.current = false;
      requestVersion.current += 1;
      for (const subscription of subscriptions) void subscription.then((unlisten) => unlisten());
    };
  }, [reload]);

  const open = async (id: string) => {
    if (openingId) return;
    setOpeningId(id);
    setOpenError("");
    try {
      await invoke("restore_history", { id });
    } catch (error) {
      if (active.current) setOpenError(String(error));
    } finally {
      if (active.current) setOpeningId(null);
    }
  };

  const error = loadError || history.error || eventError;
  return (
    <main className="history-page">
      <header className="history-header">
        <h1>翻译历史</h1>
        <p>保留最新 {history.limit} 条 · 新记录在前</p>
        <span className="history-count" aria-label={`已保留 ${history.entries.length} 条`}>
          {history.entries.length} / {history.limit}
        </span>
      </header>
      {(error || openError) && (
        <div className="history-error" role="alert">
          <p>{openError ? `无法打开记录：${openError}` : `历史记录：${error}`}</p>
          {error && <button disabled={loadState === "loading"} onClick={() => void reload()}>重试</button>}
        </div>
      )}
      <div className="history-content" aria-busy={loadState === "loading"}>
        {history.entries.length > 0 ? (
          <ol className="history-list">
            {history.entries.map((entry) => (
              <HistoryItem key={entry.id} entry={entry} onOpen={(id) => void open(id)} opening={openingId === entry.id} />
            ))}
          </ol>
        ) : loadState === "loading" ? (
          <div className="history-loading" role="status">
            <p>正在读取历史记录…</p>
            <div className="history-skeleton" aria-hidden="true" />
            <div className="history-skeleton" aria-hidden="true" />
          </div>
        ) : loadState === "ready" && !history.error ? (
          <div className="history-empty" role="status">
            <p>还没有翻译记录</p>
            <span>成功翻译后，原文和译文会自动保存在这里。</span>
          </div>
        ) : null}
      </div>
    </main>
  );
}
