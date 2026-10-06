import ReactDOM from "react-dom/client";
import { HistoryView } from "./HistoryView";
import "./history.css";

ReactDOM.createRoot(document.getElementById("history-root") as HTMLElement).render(
  <HistoryView />,
);
