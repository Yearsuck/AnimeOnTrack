import { useState } from "react";
import { scanAiring } from "../api";

export function Settings() {
  const [url, setUrl] = useState("https://wwv.animeytx.net");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function rescan() {
    setBusy(true);
    setMsg(null);
    try {
      const s = await scanAiring(url.trim());
      setMsg(`Encontradas ${s.length} series en emisión.`);
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="page" style={{ maxWidth: 560 }}>
      <div className="page-head">
        <h2 className="page-title">Ajustes</h2>
      </div>

      <div className="series-block" style={{ padding: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          URL de la fuente
        </label>
        <input className="input" value={url} onChange={(e) => setUrl(e.target.value)} />
        <button className="btn btn-primary" onClick={rescan} disabled={busy} style={{ marginTop: 12 }}>
          {busy ? "Escaneando…" : "Reescanear en emisión"}
        </button>
        {msg && <p className="muted" style={{ marginTop: 12 }}>{msg}</p>}
      </div>
    </div>
  );
}
