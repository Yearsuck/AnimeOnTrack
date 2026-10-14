import { useState } from "react";
import { THEMES, useTheme } from "../../theme";
import { LANGS, useLang, useT } from "../../i18n";
import { readNotifyPref, writeNotifyPref } from "../../lib/notify";

export function AppearanceSection() {
  const t = useT();
  const { lang, setLang } = useLang();
  const { theme, setTheme } = useTheme();
  const [notifyEnabled, setNotifyEnabled] = useState(readNotifyPref);

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
    </div>
  );
}
