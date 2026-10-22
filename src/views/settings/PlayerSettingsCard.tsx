import { useEffect, useState } from "react";
import { listMonitors, getPlayerSettings, setPlayerSettings } from "../../api";
import { useT } from "../../i18n";
import type { MonitorOption } from "../../types";

/// Player preferences card: select target screen and windowed vs full-screen mode.
export function PlayerSettingsCard() {
  const t = useT();
  const [monitors, setMonitors] = useState<MonitorOption[]>([]);
  const [targetMonitor, setTargetMonitor] = useState<string>("");
  const [fullscreen, setFullscreen] = useState(false);
  const [clean, setClean] = useState(true);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    Promise.all([listMonitors(), getPlayerSettings()])
      .then(([mons, settings]) => {
        setMonitors(mons);
        setTargetMonitor(settings.monitor ?? "");
        setFullscreen(settings.fullscreen);
        setClean(settings.clean);
      })
      .catch((err) => console.error("Error loading player settings:", err))
      .finally(() => setLoading(false));
  }, []);

  const handleMonitorChange = async (val: string) => {
    setTargetMonitor(val);
    try {
      await setPlayerSettings(val.trim() === "" ? null : val, fullscreen, clean);
    } catch (err) {
      console.error("Failed to save monitor setting:", err);
    }
  };

  const handleFullscreenChange = async (fs: boolean) => {
    setFullscreen(fs);
    try {
      await setPlayerSettings(targetMonitor.trim() === "" ? null : targetMonitor, fs, clean);
    } catch (err) {
      console.error("Failed to save fullscreen setting:", err);
    }
  };

  const handleCleanChange = async (next: boolean) => {
    setClean(next);
    try {
      await setPlayerSettings(targetMonitor.trim() === "" ? null : targetMonitor, fullscreen, next);
    } catch (err) {
      console.error("Failed to save clean player setting:", err);
    }
  };

  if (loading) return null;

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.playerHeading")}</h3>
      <p className="settings-section-desc">{t("settings.playerDesc")}</p>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.playerMonitor")}</label>
        <select
          className="input settings-select"
          value={targetMonitor}
          onChange={(e) => handleMonitorChange(e.target.value)}
        >
          <option value="">{t("settings.playerMonitorDefault")}</option>
          {monitors.map((m) => {
            const label =
              t("settings.playerMonitorOption", {
                index: m.index + 1,
                width: m.width,
                height: m.height,
              }) + (m.is_primary ? t("settings.playerMonitorPrimaryTag") : "");
            return (
              <option key={m.id} value={m.id}>
                {label}
              </option>
            );
          })}
        </select>
      </div>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.playerMode")}</label>
        <div className="row">
          <button
            type="button"
            className={`chip-toggle ${!fullscreen ? "active" : ""}`}
            onClick={() => handleFullscreenChange(false)}
          >
            🗗 {t("settings.playerWindowed")}
          </button>
          <button
            type="button"
            className={`chip-toggle ${fullscreen ? "active" : ""}`}
            onClick={() => handleFullscreenChange(true)}
          >
            ⛶ {t("settings.playerFullscreen")}
          </button>
        </div>
      </div>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.playerClean")}</label>
        <p className="settings-section-desc">{t("settings.playerCleanHelp")}</p>
        <div className="row">
          <button
            type="button"
            className={`chip-toggle ${clean ? "active" : ""}`}
            aria-pressed={clean}
            onClick={() => handleCleanChange(!clean)}
          >
            {clean ? "✓ " : ""}
            {t("settings.playerClean")}
          </button>
        </div>
      </div>
    </div>
  );
}
