import { useEffect, useState } from "react";
import { listMonitors, getPlayerSettings, setPlayerSettings } from "../../api";
import { useT } from "../../i18n";
import { useSettingSave } from "../../lib/useSettingSave";
import type { MonitorOption } from "../../types";

type PlayerPrefs = { monitor: string; fullscreen: boolean; clean: boolean; adblock: boolean };

/// Player preferences card: select target screen and windowed vs full-screen mode.
export function PlayerSettingsCard() {
  const t = useT();
  const [monitors, setMonitors] = useState<MonitorOption[]>([]);
  const { value: prefs, replace, save, failed, saving } = useSettingSave<PlayerPrefs>({
    monitor: "",
    fullscreen: false,
    clean: true,
    adblock: true,
  });
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);

  useEffect(() => {
    Promise.all([listMonitors(), getPlayerSettings()])
      .then(([mons, settings]) => {
        setMonitors(mons);
        replace({
          monitor: settings.monitor ?? "",
          fullscreen: settings.fullscreen,
          clean: settings.clean,
          adblock: settings.adblock,
        });
      })
      .catch((err) => {
        console.error("Error loading player settings:", err);
        setLoadFailed(true);
      })
      .finally(() => setLoading(false));
  }, [replace]);

  const update = (patch: Partial<PlayerPrefs>) =>
    save(patch, (n) =>
      setPlayerSettings(n.monitor.trim() === "" ? null : n.monitor, n.fullscreen, n.clean, n.adblock),
    );

  const targetMonitor = prefs.monitor;
  const { fullscreen, clean, adblock } = prefs;
  const handleMonitorChange = (val: string) => update({ monitor: val });
  const handleFullscreenChange = (fs: boolean) => update({ fullscreen: fs });
  const handleCleanChange = (next: boolean) => update({ clean: next });
  const handleAdblockChange = (next: boolean) => update({ adblock: next });
  const disabled = loading || loadFailed || saving;

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.playerHeading")}</h3>
      <p className="settings-section-desc">{t("settings.playerDesc")}</p>
      {(failed || loadFailed) && (
        <p className="settings-error" role="alert">
          {t(loadFailed ? "settings.loadError" : "settings.saveError")}
        </p>
      )}

      <div className="settings-field">
        <label className="settings-field-label" htmlFor="sf-settings-playerMonitor">{t("settings.playerMonitor")}</label>
        <select
          id="sf-settings-playerMonitor"
          disabled={disabled}
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
        <span className="settings-field-label" id="sf-settings-playerMode">{t("settings.playerMode")}</span>
        <div role="group" aria-labelledby="sf-settings-playerMode" className="row">
          <button
            type="button"
            disabled={disabled}
            className={`chip-toggle ${!fullscreen ? "active" : ""}`}
            onClick={() => handleFullscreenChange(false)}
          >
            🗗 {t("settings.playerWindowed")}
          </button>
          <button
            type="button"
            disabled={disabled}
            className={`chip-toggle ${fullscreen ? "active" : ""}`}
            onClick={() => handleFullscreenChange(true)}
          >
            ⛶ {t("settings.playerFullscreen")}
          </button>
        </div>
      </div>

      <div className="settings-field" role="group" aria-labelledby="sf-settings-playerClean">
        <span className="settings-field-label" id="sf-settings-playerClean">{t("settings.playerClean")}</span>
        <p className="settings-section-desc">{t("settings.playerCleanHelp")}</p>
        <div className="row">
          <button
            type="button"
            disabled={disabled}
            className={`chip-toggle ${clean ? "active" : ""}`}
            aria-pressed={clean}
            onClick={() => handleCleanChange(!clean)}
          >
            {clean ? "✓ " : ""}
            {t("settings.playerClean")}
          </button>
        </div>
      </div>

      <div className="settings-field" role="group" aria-labelledby="sf-settings-playerAdblock">
        <span className="settings-field-label" id="sf-settings-playerAdblock">{t("settings.playerAdblock")}</span>
        <p className="settings-section-desc">{t("settings.playerAdblockHelp")}</p>
        <div className="row">
          <button
            type="button"
            disabled={disabled}
            className={`chip-toggle ${adblock ? "active" : ""}`}
            aria-pressed={adblock}
            onClick={() => handleAdblockChange(!adblock)}
          >
            {adblock ? "✓ " : ""}
            {t("settings.playerAdblock")}
          </button>
        </div>
      </div>
    </div>
  );
}
