import { useEffect, useState } from "react";
import { scanAiring, rescanAiring, getMirrors, setMirrors, refresh } from "../api";

export function Settings() {
  const [firstUrl, setFirstUrl] = useState("https://wwv.animeytx.net");
  const [mirrorsText, setMirrorsText] = useState("");
  const [busy, setBusy] = useState(false);
  const [savingMirrors, setSavingMirrors] = useState(false);
  const [forcing, setForcing] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  async function loadMirrors() {
    const list = await getMirrors();
    setMirrorsText(list.join("\n"));
  }
  useEffect(() => {
    loadMirrors();
  }, []);

  async function addFirstUrl() {
    setBusy(true);
    setMsg(null);
    try {
      const s = await scanAiring(firstUrl.trim());
      setMsg(`Encontradas ${s.length} series en emisión.`);
      await loadMirrors();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function saveMirrors() {
    setSavingMirrors(true);
    try {
      const urls = mirrorsText.split("\n").map((u) => u.trim()).filter(Boolean);
      await setMirrors(urls);
      setMsg("Lista de webs guardada.");
    } finally {
      setSavingMirrors(false);
    }
  }

  async function doRescan() {
    setBusy(true);
    setMsg(null);
    try {
      const s = await rescanAiring();
      setMsg(`Encontradas ${s.length} series en emisión.`);
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  }

  // Escape hatch for the refresh skip logic: re-fetch every followed
  // series' episode list, ignoring the "can't have changed" rules. Slow
  // (one page per followed series) but guaranteed exhaustive.
  async function doForceRefresh() {
    setForcing(true);
    setMsg(null);
    try {
      const n = await refresh(true);
      setMsg(`Recomprobación completa terminada: ${n} episodio(s) nuevo(s).`);
    } catch (e) {
      setMsg(String(e));
    } finally {
      setForcing(false);
    }
  }

  return (
    <div className="page" style={{ maxWidth: 560 }}>
      <div className="page-head">
        <h2 className="page-title">Ajustes</h2>
      </div>

      <div className="series-block" style={{ padding: 16, marginBottom: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          Añadir una web nueva
        </label>
        <div className="row">
          <input className="input" value={firstUrl} onChange={(e) => setFirstUrl(e.target.value)} />
          <button className="btn btn-primary" onClick={addFirstUrl} disabled={busy}>
            {busy ? "Escaneando…" : "Escanear"}
          </button>
        </div>
      </div>

      <div className="series-block" style={{ padding: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          Webs configuradas (una por línea, en orden de preferencia). Si la primera falla,
          se prueba la siguiente automáticamente — útil porque los mirrors suelen tener el
          mismo contenido.
        </label>
        <textarea
          className="input"
          rows={5}
          style={{ fontFamily: "monospace", fontSize: 12.5, resize: "vertical" }}
          value={mirrorsText}
          onChange={(e) => setMirrorsText(e.target.value)}
        />
        <div className="row" style={{ marginTop: 10 }}>
          <button className="btn" onClick={saveMirrors} disabled={savingMirrors}>
            {savingMirrors ? "Guardando…" : "Guardar lista"}
          </button>
          <button className="btn btn-primary" onClick={doRescan} disabled={busy}>
            {busy ? "Escaneando…" : "Reescanear en emisión"}
          </button>
        </div>
        {msg && <p className="muted" style={{ marginTop: 12 }}>{msg}</p>}
      </div>

      <div className="series-block" style={{ padding: 16, marginTop: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          Actualizar normalmente se salta las series que no pueden tener episodios nuevos
          (según el propio calendario de la web). Si sospechas que falta algún episodio,
          esto vuelve a comprobar TODAS las series seguidas, una a una. Es lento.
        </label>
        <button className="btn" onClick={doForceRefresh} disabled={forcing}>
          {forcing ? "Recomprobando…" : "Forzar recomprobación completa"}
        </button>
      </div>
    </div>
  );
}
