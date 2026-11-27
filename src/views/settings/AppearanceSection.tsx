import { useState } from "react";
import { SettingsRow } from "./SettingsRow";
import { Chip } from "../../components/Chip";
import type { Update } from "@tauri-apps/plugin-updater";
import { THEMES, useTheme } from "../../theme";
import { LANGS, useLang, useT } from "../../i18n";
import { readNotifyPref, writeNotifyPref } from "../../lib/notify";
import { isMobile } from "../../lib/platform";
import { checkForUpdate, installUpdate, readUpdatePref, writeUpdatePref } from "../../lib/updater";

export function AppearanceSection({ onReplayTutorial }: { onReplayTutorial?: () => void }) {
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

      <SettingsRow label={t("settings.theme")} labelId="sf-settings-theme">
        <div role="group" aria-labelledby="sf-settings-theme" className="row">
          {THEMES.map((th) => (
            <Chip
              key={th.code}
              active={theme === th.code}
              onClick={() => setTheme(th.code)}
            >
              {t(th.labelKey)}
            </Chip>
          ))}
        </div>
      </SettingsRow>

      <SettingsRow label={t("settings.language")} labelId="sf-settings-language">
        <div role="group" aria-labelledby="sf-settings-language" className="row">
          {LANGS.map((l) => (
            <Chip
              key={l.code}
              active={lang === l.code}
              onClick={() => setLang(l.code)}
            >
              {l.label}
            </Chip>
          ))}
        </div>
      </SettingsRow>

      <SettingsRow label={t("settings.notifyLabel")} labelId="sf-settings-notifyLabel">
        <div role="group" aria-labelledby="sf-settings-notifyLabel" className="row">
          <Chip
            active={notifyEnabled}
            onClick={() => handleNotifyToggle(true)}
          >
            {t("settings.notifyOn")}
          </Chip>
          <Chip
            active={!notifyEnabled}
            onClick={() => handleNotifyToggle(false)}
          >
            {t("settings.notifyOff")}
          </Chip>
        </div>
        <p className="settings-field-hint">{t(isMobile ? "settings.notifyHintMobile" : "settings.notifyHint")}</p>
      </SettingsRow>

      {!isMobile && (
        <SettingsRow label={t("settings.updateLabel")} labelId="sf-settings-updateLabel">
          <div role="group" aria-labelledby="sf-settings-updateLabel" className="row">
            <Chip
              active={updateAuto}
              onClick={() => handleUpdateToggle(true)}
            >
              {t("settings.updateOn")}
            </Chip>
            <Chip
              active={!updateAuto}
              onClick={() => handleUpdateToggle(false)}
            >
              {t("settings.updateOff")}
            </Chip>
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
        </SettingsRow>
      )}

      {onReplayTutorial && (
        <SettingsRow label={t("settings.tutorialLabel")} labelId="sf-settings-tutorialLabel">
          <div role="group" aria-labelledby="sf-settings-tutorialLabel" className="row">
            <button type="button" className="btn" onClick={onReplayTutorial}>
              {t("settings.tutorialButton")}
            </button>
          </div>
          <p className="settings-field-hint">{t("settings.tutorialHint")}</p>
        </SettingsRow>
      )}
    </div>
  );
}
