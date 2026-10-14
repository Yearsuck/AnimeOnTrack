import { useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { THEMES, useTheme } from "../../theme";
import { LANGS, useLang, useT } from "../../i18n";
import { readNotifyPref, writeNotifyPref } from "../../lib/notify";
import { checkForUpdate, installUpdate, readUpdatePref, writeUpdatePref } from "../../lib/updater";

export function AppearanceSection() {
  const t = useT();
  const { lang, setLang } = useLang();
  const { theme, setTheme } = useTheme();
  const [notifyEnabled, setNotifyEnabled] = useState(readNotifyPref);

  const [updateAuto, setUpdateAuto] = useState(readUpdatePref);
  const [updateState, setUpdateState] = useState<
    | { kind: "idle" }
    | { kind: "checking" }
    | { kind: "upToDate" }
    | { kind: "found"; update: Update }
    | { kind: "installing"; percent: number | null }
    | { kind: "failed" }
  >({ kind: "idle" });

  const handleUpdateToggle = (enabled: boolean) => {
    setUpdateAuto(enabled);
    writeUpdatePref(enabled);
  };

  const handleUpdateCheck = () => {
    setUpdateState({ kind: "checking" });
    checkForUpdate()
      .then((u) => setUpdateState(u ? { kind: "found", update: u } : { kind: "upToDate" }))
      .catch(() => setUpdateState({ kind: "failed" }));
  };

  const handleUpdateInstall = (update: Update) => {
    setUpdateState({ kind: "installing", percent: null });
    installUpdate(update, (percent) => setUpdateState({ kind: "installing", percent })).catch(() =>
      setUpdateState({ kind: "failed" }),
    );
  };

  const handleNotifyToggle = (enabled: boolean) => {
    setNotifyEnabled(enabled);
    writeNotifyPref(enabled);
  };

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.appearanceHeading")}</h3>
      <p className="settings-section-desc">{t("settings.appearanceDesc")}</p>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.theme")}</label>
        <div className="row">
          {THEMES.map((th) => (
            <button
              key={th.code}
              type="button"
              className={`chip-toggle ${theme === th.code ? "active" : ""}`}
              onClick={() => setTheme(th.code)}
            >
              {t(th.labelKey)}
            </button>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.language")}</label>
        <div className="row">
          {LANGS.map((l) => (
            <button
              key={l.code}
              type="button"
              className={`chip-toggle ${lang === l.code ? "active" : ""}`}
              onClick={() => setLang(l.code)}
            >
              {l.label}
            </button>
          ))}
        </div>
      </div>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.notifyLabel")}</label>
        <div className="row">
          <button
            type="button"
            className={`chip-toggle ${notifyEnabled ? "active" : ""}`}
            onClick={() => handleNotifyToggle(true)}
          >
            {t("settings.notifyOn")}
          </button>
          <button
            type="button"
            className={`chip-toggle ${!notifyEnabled ? "active" : ""}`}
            onClick={() => handleNotifyToggle(false)}
          >
            {t("settings.notifyOff")}
          </button>
        </div>
        <p className="settings-field-hint">{t("settings.notifyHint")}</p>
      </div>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.updateLabel")}</label>
        <div className="row">
          <button
            type="button"
            className={`chip-toggle ${updateAuto ? "active" : ""}`}
            onClick={() => handleUpdateToggle(true)}
          >
            {t("settings.updateOn")}
          </button>
          <button
            type="button"
            className={`chip-toggle ${!updateAuto ? "active" : ""}`}
            onClick={() => handleUpdateToggle(false)}
          >
            {t("settings.updateOff")}
          </button>
          {updateState.kind === "found" ? (
            <button type="button" className="btn btn-primary" onClick={() => handleUpdateInstall(updateState.update)}>
              {t("update.install")}
            </button>
          ) : (
            <button
              type="button"
              className="btn"
              disabled={updateState.kind === "checking" || updateState.kind === "installing"}
              onClick={handleUpdateCheck}
            >
              {updateState.kind === "checking" ? t("settings.updateChecking") : t("settings.updateCheck")}
            </button>
          )}
        </div>
        {updateState.kind === "upToDate" && <p className="settings-field-hint">{t("settings.updateUpToDate")}</p>}
        {updateState.kind === "found" && (
          <p className="settings-field-hint">{t("settings.updateFound", { version: updateState.update.version })}</p>
        )}
        {updateState.kind === "installing" && (
          <p className="settings-field-hint">
            {updateState.percent === null ? t("update.installing") : t("update.downloading", { percent: updateState.percent })}
          </p>
        )}
        {updateState.kind === "failed" && <p className="settings-field-hint">{t("update.failed")}</p>}
        <p className="settings-field-hint">{t("settings.updateHint")}</p>
      </div>
    </div>
  );
}
