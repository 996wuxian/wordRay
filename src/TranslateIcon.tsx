import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * 划词后出现在选区右侧的小图标。
 *
 * 注意文件名：**不要**把这个组件命名为 `Icon.tsx`——它与入口 `icon.tsx`
 * 在 Windows 的大小写不敏感文件系统上是同一个路径，会互相覆盖。
 *
 * 图像与应用、安装器使用同一套 WordRay 图标资源。
 */
export function TranslateIcon() {
  const [busy, setBusy] = useState(false);

  const onClick = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await invoke("open_panel");
    } finally {
      setBusy(false);
    }
  };

  return (
    <button
      className="pebble"
      title="WordRay · 翻译选中的文字"
      aria-label="WordRay · 翻译选中的文字"
      aria-busy={busy}
      onClick={() => void onClick()}
      onContextMenu={(e) => {
        e.preventDefault();
        void invoke("dismiss_icon");
      }}
    >
      <img className="glyph" src="/logo.png" alt="" draggable={false} />
    </button>
  );
}
