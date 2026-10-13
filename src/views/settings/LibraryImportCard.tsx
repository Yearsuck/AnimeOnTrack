import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { importLibraryToActiveSite } from "../../api";
import { useT } from "../../i18n";

/// "Traer mi biblioteca a este sitio": resolves your whole canonical library
/// onto the active site (searches + links the ones not present here, replaying
/// watched progress). Self-contained; listens to the paced import's progress.
export function LibraryImportCard() {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number; title: string } | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    const un = listen<{ done: number; total: number; title: string }>(
      "library-import-progress",
      (e) => setProgress(e.payload)
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function run() {
    setRunning(true);
    setResult(null);
    setProgress(null);
    try {
      const r = await importLibraryToActiveSite();
      setResult(t("settings.importDone", { linked: r.linked, total: r.total, skipped: r.skipped }));
    } catch (e) {
      setResult(String(e));
    } finally {
      setRunning(false);
      setProgress(null);
    }
  }

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.importHeading")}</h3>
      <p className="settings-section-desc">{t("settings.importDesc")}</p>
      <button className="btn btn-primary" onClick={run} disabled={running}>
        {running ? t("settings.importing") : t("settings.importButton")}
      </button>
      {running && progress && progress.total > 0 && (
        <p className="muted settings-msg">
          {t("settings.importProgress", {
            done: progress.done + 1,
            total: progress.total,
            title: progress.title,
          })}
        </p>
      )}
      {result && <p className="muted settings-msg">{result}</p>}
    </div>
  );
}
