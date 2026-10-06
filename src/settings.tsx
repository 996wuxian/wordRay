import ReactDOM from "react-dom/client";
import { SettingsView } from "./SettingsView";
import "./settings.css";

// 设置窗入口。与 App（翻译面板）、TranslateIcon 一样，是同一个 Vite 工程的一个入口，
// 由 tauri.conf.json 里的 settings 窗口加载。
ReactDOM.createRoot(document.getElementById("settings-root") as HTMLElement).render(
  <SettingsView />,
);
