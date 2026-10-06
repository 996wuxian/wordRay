import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  IconBusy,
  IconHide,
  IconReset,
  IconReveal,
  IconSave,
  IconTest,
  IconTrash,
} from "./components/icons";

type KeySource = "settings" | "env" | "none";

interface SettingsPayload {
  base_url: string;
  model: string;
  api_key: string | null;
  key_source: KeySource;
  config_path: string;
  default_base_url: string;
  default_model: string;
}

interface Status {
  kind: "idle" | "busy" | "ok" | "error";
  text: string;
}

/**
 * 设置界面。
 *
 * 参数一律用**结构体**传给 Rust（`{ args: { base_url, model, api_key } }`），
 * 不依赖 Tauri 命令参数的大小写改名规则——那是个容易踩的坑。
 *
 * 按钮全部用图标 + 悬浮提示（`title`），与翻译面板保持一致。
 */
export function SettingsView() {
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [reveal, setReveal] = useState(false);
  const [keySource, setKeySource] = useState<KeySource>("none");
  const [configPath, setConfigPath] = useState("");
  const [defaults, setDefaults] = useState({ base: "", model: "" });
  const [status, setStatus] = useState<Status>({ kind: "idle", text: "" });

  const busy = status.kind === "busy";

  const applyPayload = (p: SettingsPayload) => {
    setBaseUrl(p.base_url);
    setModel(p.model);
    setApiKey(p.api_key ?? "");
    setKeySource(p.key_source);
    setConfigPath(p.config_path);
    setDefaults({ base: p.default_base_url, model: p.default_model });
  };

  useEffect(() => {
    void invoke<SettingsPayload>("get_settings")
      .then(applyPayload)
      .catch((e) => setStatus({ kind: "error", text: String(e) }));
  }, []);

  const args = () => ({ base_url: baseUrl, model, api_key: apiKey });

  const save = async () => {
    setStatus({ kind: "busy", text: "保存中…" });
    try {
      await invoke("save_settings", { args: args() });
      applyPayload(await invoke<SettingsPayload>("get_settings"));
      setStatus({ kind: "ok", text: "已保存" });
    } catch (e) {
      setStatus({ kind: "error", text: String(e) });
    }
  };

  const test = async () => {
    setStatus({ kind: "busy", text: "正在请求 DeepSeek…" });
    try {
      const message = await invoke<string>("test_settings", { args: args() });
      setStatus({ kind: "ok", text: message });
    } catch (e) {
      setStatus({ kind: "error", text: String(e) });
    }
  };

  const clearKey = async () => {
    setApiKey("");
    setStatus({ kind: "busy", text: "正在清除…" });
    try {
      await invoke("save_settings", {
        args: { base_url: baseUrl, model, api_key: "" },
      });
      applyPayload(await invoke<SettingsPayload>("get_settings"));
      setStatus({ kind: "ok", text: "已清除保存的密钥" });
    } catch (e) {
      setStatus({ kind: "error", text: String(e) });
    }
  };

  const sourceLabel = {
    settings: "来自本设置（DPAPI 加密后存在磁盘上）",
    env: "来自环境变量 DEEPSEEK_API_KEY（本设置里没有保存密钥）",
    none: "未配置 —— 现在还不能翻译",
  }[keySource];

  const sourceClass =
    keySource === "none"
      ? "source source-bad"
      : keySource === "env"
        ? "source source-env"
        : "source source-ok";

  return (
    <div className="page">
      <h1>WordRay · 设置</h1>

      <section className="block">
        <label className="field">
          <span className="name">DeepSeek API Key</span>
          <div className="row">
            <input
              type={reveal ? "text" : "password"}
              value={apiKey}
              placeholder="sk-..."
              spellCheck={false}
              onChange={(e) => setApiKey(e.target.value)}
            />
            <button
              className="mini icon-only"
              title={reveal ? "隐藏密钥" : "显示密钥"}
              aria-label={reveal ? "隐藏密钥" : "显示密钥"}
              onClick={() => setReveal((v) => !v)}
            >
              {reveal ? <IconHide /> : <IconReveal />}
            </button>
          </div>
        </label>
        <p className={sourceClass}>{sourceLabel}</p>
        <p className="note">
          密钥用 Windows DPAPI 加密后保存，密文只能被当前 Windows 用户解开；明文不写文件、不进日志。
        </p>
      </section>

      <section className="block">
        <label className="field">
          <span className="name">接口地址</span>
          <div className="row">
            <input
              type="text"
              value={baseUrl}
              spellCheck={false}
              onChange={(e) => setBaseUrl(e.target.value)}
            />
            <button
              className="mini icon-only"
              title="恢复默认地址"
              aria-label="恢复默认地址"
              onClick={() => setBaseUrl(defaults.base)}
            >
              <IconReset />
            </button>
          </div>
        </label>

        <label className="field">
          <span className="name">模型</span>
          <div className="row">
            <input
              type="text"
              value={model}
              spellCheck={false}
              onChange={(e) => setModel(e.target.value)}
            />
            <button
              className="mini icon-only"
              title="恢复默认模型"
              aria-label="恢复默认模型"
              onClick={() => setModel(defaults.model)}
            >
              <IconReset />
            </button>
          </div>
        </label>
        <p className="note">
          取值优先级：本设置 &gt; 环境变量 &gt; 默认值。地址要带版本前缀（如 <code>/v1</code>），
          不要带 <code>/chat/completions</code>。
        </p>
      </section>

      <section className="block actions">
        <button
          className="primary icon-only"
          title="保存"
          aria-label="保存"
          onClick={() => void save()}
          disabled={busy}
        >
          {busy ? <IconBusy /> : <IconSave />}
        </button>
        <button
          className="icon-only"
          title="测试连接（会真跑一次请求）"
          aria-label="测试连接"
          onClick={() => void test()}
          disabled={busy}
        >
          <IconTest />
        </button>
        <button
          className="icon-only"
          title="清除已保存的密钥"
          aria-label="清除已保存的密钥"
          onClick={() => void clearKey()}
          disabled={busy}
        >
          <IconTrash />
        </button>
      </section>

      {status.kind !== "idle" && <p className={"status status-" + status.kind}>{status.text}</p>}

      <footer className="foot">
        <span>配置文件：</span>
        <code>{configPath || "（未知）"}</code>
      </footer>
    </div>
  );
}
