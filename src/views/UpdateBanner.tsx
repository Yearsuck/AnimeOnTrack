import { useEffect, useRef, useState } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { useT } from "../i18n";
import { checkForUpdate, installUpdate, readUpdatePref } from "../lib/updater";

/** Asks for a newer release once at startup (if enabled) and offers to install it. */
export function UpdateBanner() {
  const t = useT();
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [percent, setPercent] = useState<number | null>(null);
  const [failed, setFailed] = useState(false);
  const startedRef = useRef(false);

  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    if (!readUpdatePref()) return;
    // Let the first screen load before touching the network.
    const timer = window.setTimeout(() => {
      checkForUpdate()
        .then(setUpdate)
        .catch(() => {});
    }, 8000);
    return () => window.clearTimeout(timer);
  }, []);

  if (!update || dismissed) return null;

  const install = () => {
    setInstalling(true);
    setFailed(false);
    installUpdate(update, setPercent).catch(() => {
      setInstalling(false);
      setFailed(true);
    });
  };

  return (
    <div className="update-banner" role="status">
      <span className="update-banner-text">
        {installing
          ? percent === null
            ? t("update.installing")
            : t("update.downloading", { percent })
          : failed
            ? t("update.failed")
            : t("update.available", { version: update.version })}
      </span>
      {!installing && (
        <>
          <button type="button" className="btn btn-primary" onClick={install}>
            {failed ? t("update.retry") : t("update.install")}
          </button>
          <button type="button" className="btn" onClick={() => setDismissed(true)}>
            {t("update.later")}
          </button>
        </>
      )}
    </div>
  );
}
