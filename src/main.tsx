import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { I18nProvider } from "./i18n";
import { ThemeProvider } from "./theme";
// Inter, bundled as a local variable-weight woff2 rather than pulled from a
// CDN: this is an offline desktop app, and a webfont that fails to load would
// silently fall back to whatever Windows happens to have. The variable file is
// one request covering every weight the UI uses.
import "@fontsource-variable/inter";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <I18nProvider>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </I18nProvider>
  </React.StrictMode>,
);
