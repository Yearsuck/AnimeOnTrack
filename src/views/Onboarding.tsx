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
    <div style={{ padding: 24, maxWidth: 480 }}>
      <h1>AnimeOnTrack</h1>
      <p>Enter the site URL to scan airing anime.</p>
      <input
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        style={{ width: "100%", padding: 8 }}
      />
      <button disabled={busy} onClick={submit} style={{ marginTop: 12 }}>
        {busy ? "Scanning…" : "Scan"}
      </button>
      {error && <p style={{ color: "crimson" }}>{error}</p>}
    </div>
  );
}
