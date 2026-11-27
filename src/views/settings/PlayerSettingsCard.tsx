import { useEffect, useState } from "react";
import { SettingsRow } from "./SettingsRow";
import { Chip } from "../../components/Chip";
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

      <SettingsRow label={t("settings.playerMonitor")} htmlFor="sf-settings-playerMonitor">
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
      </SettingsRow>

      <SettingsRow label={t("settings.playerMode")} labelId="sf-settings-playerMode">
        <div role="group" aria-labelledby="sf-settings-playerMode" className="row">
          <Chip
            disabled={disabled}
            active={!fullscreen}
            onClick={() => handleFullscreenChange(false)}
          >
            🗗 {t("settings.playerWindowed")}
          </Chip>
          <Chip
            disabled={disabled}
            active={fullscreen}
            onClick={() => handleFullscreenChange(true)}
          >
            ⛶ {t("settings.playerFullscreen")}
          </Chip>
        </div>
      </SettingsRow>

      <SettingsRow label={t("settings.playerClean")} labelId="sf-settings-playerClean" group>
        <p className="settings-section-desc">{t("settings.playerCleanHelp")}</p>
        <div className="row">
          <Chip
            disabled={disabled}
            active={clean}
            aria-pressed={clean}
            onClick={() => handleCleanChange(!clean)}
          >
            {clean ? "✓ " : ""}
            {t("settings.playerClean")}
          </Chip>
        </div>
      </SettingsRow>

      <SettingsRow label={t("settings.playerAdblock")} labelId="sf-settings-playerAdblock" group>
        <p className="settings-section-desc">{t("settings.playerAdblockHelp")}</p>
        <div className="row">
          <Chip
            disabled={disabled}
            active={adblock}
            aria-pressed={adblock}
            onClick={() => handleAdblockChange(!adblock)}
          >
            {adblock ? "✓ " : ""}
            {t("settings.playerAdblock")}
          </Chip>
        </div>
      </SettingsRow>
    </div>
  );
}
