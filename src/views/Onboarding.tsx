import { useState } from "react";
import { scanAiring } from "../api";
import { useT } from "../i18n";

export function Onboarding({ onDone }: { onDone: () => void }) {
  const t = useT();
  const [url, setUrl] = useState("https://wwv.animeytx.net");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      await scanAiring(url.trim());
      onDone();
    } catch (e) {
      setError(t("errors.generic", { detail: String(e) }));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="onboard">
      <div
        className="brand"
        style={{ justifyContent: "center", fontSize: 22, marginBottom: 10 }}
      >
        <span className="dot" />
        AnimeOnTrack
      </div>
      <h1>{t("onboarding.title")}</h1>
      <p>{t("onboarding.subtitle")}</p>
      <div className="row">
        <input
          className="input"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder={t("onboarding.urlPlaceholder")}
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
        <button className="btn btn-primary" disabled={busy} onClick={submit}>
          {busy ? t("common.scanning") : t("common.scan")}
        </button>
      </div>
      {busy && (
        <p className="muted" style={{ marginTop: 14 }}>
          {t("onboarding.scanningHint")}
        </p>
      )}
      {error && (
        <p style={{ color: "var(--danger)", marginTop: 14, wordBreak: "break-word" }}>
          {error}
        </p>
      )}
    </div>
  );
}
