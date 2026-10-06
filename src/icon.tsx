import ReactDOM from "react-dom/client";
import { TranslateIcon } from "./TranslateIcon";
import "./icon.css";

// 图标窗入口。与 src/App.tsx（翻译面板）是同一个 Vite 工程的两个入口，
// 分别由 tauri.conf.json 里的 icon / panel 两个窗口加载。
ReactDOM.createRoot(document.getElementById("icon-root") as HTMLElement).render(
  <TranslateIcon />,
);
