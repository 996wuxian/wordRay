import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

// 刻意不使用 StrictMode：它会重复挂载 effect，对"每轮翻译只应订阅一次"的事件流没有好处
ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(<App />);
