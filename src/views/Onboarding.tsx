import { useState } from "react";
import { scanAiring } from "../api";

export function Onboarding({ onDone }: { onDone: () => void }) {
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
      setError(String(e));
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
      <h1>Sigue tus animes en emisión</h1>
      <p>Introduce la URL de la web para escanear los animes en estreno.</p>
      <div className="row">
        <input
          className="input"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://…"
          onKeyDown={(e) => e.key === "Enter" && submit()}
        />
        <button className="btn btn-primary" disabled={busy} onClick={submit}>
          {busy ? "Escaneando…" : "Escanear"}
        </button>
      </div>
      {busy && (
        <p className="muted" style={{ marginTop: 14 }}>
          Abriendo la web y pasando la verificación… puede tardar unos segundos.
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
