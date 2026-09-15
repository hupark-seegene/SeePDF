import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/shell.css";
import { detectOs } from "./ipc/env";
import { resolveLocale } from "./i18n";

// Stamp the OS and the startup locale before the first paint: the 78 px traffic-light gutter and
// the Korean font stack must be right in frame one (UI_SPEC §1).
document.documentElement.dataset.os = detectOs();
document.documentElement.lang = resolveLocale();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
