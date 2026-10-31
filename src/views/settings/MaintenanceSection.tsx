import { useState } from "react";
import { exportDiagnostics, refresh, revealInFolder } from "../../api";
import { useT } from "../../i18n";
import { BusyLabel } from "../../components/BusyLabel";

export function MaintenanceSection() {
  const t = useT();
  const [forcing, setForcing] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const [exporting, setExporting] = useState(false);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const [diagError, setDiagError] = useState<string | null>(null);

  // Escape hatch for the refresh skip logic: re-fetch every followed
  // series' episode list, ignoring the "can't have changed" rules. Slow
  // (one page per followed series) but guaranteed exhaustive.
  async function doForceRefresh() {
    setForcing(true);
    setMsg(null);
    try {
      const n = await refresh(true);
      setMsg(t("settings.forceRefreshDone", { count: n }));
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setForcing(false);
    }
  }

  async function doExportDiagnostics() {
    setExporting(true);
    setDiagError(null);
    try {
      const path = await exportDiagnostics();
      setSavedPath(path);
    } catch (e) {
      setDiagError(t("errors.generic", { detail: String(e) }));
    } finally {
      setExporting(false);
    }
  }

  async function doRevealInFolder() {
    if (!savedPath) return;
    try {
      await revealInFolder(savedPath);
    } catch (e) {
      setDiagError(t("errors.generic", { detail: String(e) }));
    }
  }

  return (
    <>
      <div className="settings-section">
        <h3 className="settings-section-title">{t("settings.maintenanceHeading")}</h3>
        <p className="settings-section-desc">{t("settings.forceRefreshDesc")}</p>
        <button className="btn" onClick={doForceRefresh} disabled={forcing} aria-busy={forcing}>
          <BusyLabel busy={forcing} idle={t("settings.forceRefresh")} busyText={t("settings.forcing")} />
        </button>
        {msg && <p className="muted settings-msg">{msg}</p>}
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">{t("settings.diagnosticsHeading")}</h3>
        <p className="settings-section-desc">{t("settings.diagnosticsDesc")}</p>
        <button className="btn" onClick={doExportDiagnostics} disabled={exporting}>
          {exporting ? t("settings.diagnosticsExporting") : t("settings.diagnosticsExport")}
        </button>
        {savedPath && (
          <div>
            <p className="muted settings-msg">{t("settings.diagnosticsSaved", { path: savedPath })}</p>
            <button className="btn" onClick={doRevealInFolder}>
              {t("settings.diagnosticsReveal")}
            </button>
          </div>
        )}
        {diagError && <p className="muted settings-msg">{diagError}</p>}
      </div>
    </>
  );
}
