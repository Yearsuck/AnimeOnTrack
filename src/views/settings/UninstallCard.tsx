import { useState } from "react";
import { uninstallApp } from "../../api";
import { useT } from "../../i18n";

export function UninstallCard() {
  const t = useT();
  // Two-step confirm: the first click reveals the confirmation panel, so the
  // irreversible action is never one stray click away.
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function doUninstall() {
    setRunning(true);
    setError(null);
    try {
      // On success the backend quits the app, so control never returns here;
      // a rejection means there was nothing to uninstall (dev/unpacked build).
      await uninstallApp();
    } catch (e) {
      setError(String(e));
      setRunning(false);
      setConfirming(false);
    }
  }

  return (
    <div className="settings-section danger-zone">
      <h3 className="settings-section-title">{t("settings.uninstallHeading")}</h3>
      <p className="settings-section-desc">{t("settings.uninstallDesc")}</p>
      {!confirming ? (
        <button className="btn btn-danger" onClick={() => setConfirming(true)}>
          {t("settings.uninstallButton")}
        </button>
      ) : (
        <div className="settings-inline-panel">
          <p className="settings-confirm-text">{t("settings.uninstallConfirm")}</p>
          <div className="row">
            <button className="btn btn-danger" onClick={doUninstall} disabled={running}>
              {running ? t("settings.uninstalling") : t("settings.uninstallConfirmYes")}
            </button>
            <button className="btn" onClick={() => setConfirming(false)} disabled={running}>
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
      {error && <p className="muted settings-msg text-danger">{error}</p>}
    </div>
  );
}
