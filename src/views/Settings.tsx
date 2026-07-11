import { useEffect, useState } from "react";
import {
  scanAiring,
  rescanAiring,
  getMirrors,
  setMirrors,
  refresh,
  listSites,
  getActiveSite,
  setActiveSite,
} from "../api";
import type { SiteSummary } from "../types";

export function Settings({ onSiteChanged }: { onSiteChanged?: (site: SiteSummary) => void }) {
  const [firstUrl, setFirstUrl] = useState("https://wwv.animeytx.net");
  const [mirrorsText, setMirrorsText] = useState("");
  const [busy, setBusy] = useState(false);
  const [savingMirrors, setSavingMirrors] = useState(false);
  const [forcing, setForcing] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const [sites, setSites] = useState<SiteSummary[]>([]);
  const [activeSite, setActiveSiteState] = useState<SiteSummary | null>(null);
  // The site picked in the <select> but not yet confirmed — drives the
  // confirmation panel below it. Separate from activeSite so changing the
  // dropdown never switches anything by itself.
  const [pendingSiteId, setPendingSiteId] = useState<string | null>(null);
  const [switchingSite, setSwitchingSite] = useState(false);

  async function loadMirrors() {
    const list = await getMirrors();
    setMirrorsText(list.join("\n"));
  }
  async function loadSites() {
    const [all, active] = await Promise.all([listSites(), getActiveSite()]);
    setSites(all);
    setActiveSiteState(active);
    setFirstUrl(active.default_base_url);
  }
  useEffect(() => {
    loadMirrors();
    loadSites();
  }, []);

  const pendingSite = sites.find((s) => s.id === pendingSiteId) ?? null;

  async function confirmSiteSwitch() {
    if (!pendingSite) return;
    setSwitchingSite(true);
    setMsg(null);
    try {
      const result = await setActiveSite(pendingSite.id);
      const s = await scanAiring(result.site.default_base_url);
      setActiveSiteState(result.site);
      setPendingSiteId(null);
      await loadMirrors();
      setMsg(`Cambiado a ${result.site.name}. Encontradas ${s.length} series en emisión.`);
      onSiteChanged?.(result.site);
    } catch (e) {
      setMsg(String(e));
    } finally {
      setSwitchingSite(false);
    }
  }

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
          Sitio activo — cada sitio tiene su propia biblioteca (series seguidas,
          progreso de visionado). Cambiar de sitio NO borra nada de los demás;
          simplemente muestra la biblioteca de ese sitio.
        </label>
        <select
          className="input"
          style={{ maxWidth: 280 }}
          value={pendingSiteId ?? activeSite?.id ?? ""}
          disabled={switchingSite || sites.length === 0}
          onChange={(e) => {
            const id = e.target.value;
            setPendingSiteId(id === activeSite?.id ? null : id);
          }}
        >
          {sites.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name}
              {s.id === activeSite?.id ? " (activo)" : ""}
            </option>
          ))}
        </select>

        {pendingSite && (
          <div
            className="series-block"
            style={{ padding: 12, marginTop: 12, background: "var(--surface-2, rgba(255,255,255,0.04))" }}
          >
            <p style={{ margin: 0, marginBottom: 10 }}>
              Cambiar de sitio mostrará la biblioteca de {pendingSite.name}. Tus series de{" "}
              {activeSite?.name ?? "el sitio actual"} se conservan y volverán al cambiar de vuelta.
            </p>
            <div className="row">
              <button className="btn btn-primary" onClick={confirmSiteSwitch} disabled={switchingSite}>
                {switchingSite ? "Cambiando…" : `Cambiar a ${pendingSite.name}`}
              </button>
              <button className="btn" onClick={() => setPendingSiteId(null)} disabled={switchingSite}>
                Cancelar
              </button>
            </div>
          </div>
        )}
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
          Webs configuradas para {activeSite?.name ?? "el sitio activo"} (una por línea, en
          orden de preferencia; cada sitio tiene su propia lista). Si la primera falla, se
          prueba la siguiente automáticamente — útil porque los mirrors suelen tener el
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
