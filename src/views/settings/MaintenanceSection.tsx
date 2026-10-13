import { useState } from "react";
import { refresh } from "../../api";
import { useT } from "../../i18n";

export function MaintenanceSection() {
  const t = useT();
  const [forcing, setForcing] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

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

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.maintenanceHeading")}</h3>
      <p className="settings-section-desc">{t("settings.forceRefreshDesc")}</p>
      <button className="btn" onClick={doForceRefresh} disabled={forcing}>
        {forcing ? t("settings.forcing") : t("settings.forceRefresh")}
      </button>
      {msg && <p className="muted settings-msg">{msg}</p>}
    </div>
  );
}
