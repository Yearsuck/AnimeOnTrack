import { useState } from "react";
import { scanAiring } from "../api";

export function Settings() {
  const [url, setUrl] = useState("https://wwv.animeytx.net");
  const [msg, setMsg] = useState<string | null>(null);

  async function rescan() {
    setMsg("Scanning…");
    try {
      const s = await scanAiring(url.trim());
      setMsg(`Found ${s.length} airing series.`);
    } catch (e) {
      setMsg(String(e));
    }
  }

  return (
    <div style={{ padding: 16 }}>
      <h2>Settings</h2>
      <label>Source URL</label>
      <input value={url} onChange={(e) => setUrl(e.target.value)} style={{ width: "100%", padding: 8 }} />
      <button onClick={rescan} style={{ marginTop: 8 }}>Re-scan airing</button>
      {msg && <p>{msg}</p>}
    </div>
  );
}
