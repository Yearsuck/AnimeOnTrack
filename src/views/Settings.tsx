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
import { LANGS, useLang, useT } from "../i18n";
import type { SiteSummary } from "../types";

export function Settings({ onSiteChanged }: { onSiteChanged?: (site: SiteSummary) => void }) {
  const t = useT();
  const { lang, setLang } = useLang();
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
      setMsg(t("settings.siteChanged", { site: result.site.name, count: s.length }));
      onSiteChanged?.(result.site);
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setSwitchingSite(false);
    }
  }

  async function addFirstUrl() {
    setBusy(true);
    setMsg(null);
    try {
      const s = await scanAiring(firstUrl.trim());
      setMsg(t("settings.foundSeries", { count: s.length }));
      await loadMirrors();
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setBusy(false);
    }
  }

  async function saveMirrors() {
    setSavingMirrors(true);
    try {
      const urls = mirrorsText.split("\n").map((u) => u.trim()).filter(Boolean);
      await setMirrors(urls);
      setMsg(t("settings.mirrorsSaved"));
    } finally {
      setSavingMirrors(false);
    }
  }

  async function doRescan() {
    setBusy(true);
    setMsg(null);
    try {
      const s = await rescanAiring();
      setMsg(t("settings.foundSeries", { count: s.length }));
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
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
      setMsg(t("settings.forceRefreshDone", { count: n }));
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setForcing(false);
    }
  }

  return (
    <div className="page" style={{ maxWidth: 560 }}>
      <div className="page-head">
        <h2 className="page-title">{t("nav.settings")}</h2>
      </div>

      <div className="series-block" style={{ padding: 16, marginBottom: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          {t("settings.language")}
        </label>
        <select
          className="input"
          style={{ maxWidth: 280 }}
          value={lang}
          onChange={(e) => setLang(e.target.value as typeof lang)}
        >
          {LANGS.map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>
      </div>

      <div className="series-block" style={{ padding: 16, marginBottom: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          {t("settings.activeSiteHelp")}
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
              {s.id === activeSite?.id ? t("settings.activeSuffix") : ""}
            </option>
          ))}
        </select>

        {pendingSite && (
          <div
            className="series-block"
            style={{ padding: 12, marginTop: 12, background: "var(--surface-2, rgba(255,255,255,0.04))" }}
          >
            <p style={{ margin: 0, marginBottom: 10 }}>
              {t("settings.switchSiteConfirm", {
                site: pendingSite.name,
                currentSite: activeSite?.name ?? t("settings.currentSiteFallback"),
              })}
            </p>
            <div className="row">
              <button className="btn btn-primary" onClick={confirmSiteSwitch} disabled={switchingSite}>
                {switchingSite ? t("settings.switching") : t("settings.switchTo", { site: pendingSite.name })}
              </button>
              <button className="btn" onClick={() => setPendingSiteId(null)} disabled={switchingSite}>
                {t("common.cancel")}
              </button>
            </div>
          </div>
        )}
      </div>

      <div className="series-block" style={{ padding: 16, marginBottom: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          {t("settings.addNewSiteLabel")}
        </label>
        <div className="row">
          <input className="input" value={firstUrl} onChange={(e) => setFirstUrl(e.target.value)} />
          <button className="btn btn-primary" onClick={addFirstUrl} disabled={busy}>
            {busy ? t("common.scanning") : t("common.scan")}
          </button>
        </div>
      </div>

      <div className="series-block" style={{ padding: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          {t("settings.mirrorsLabel", { site: activeSite?.name ?? t("settings.activeSiteFallback") })}
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
            {savingMirrors ? t("settings.saving") : t("settings.saveMirrors")}
          </button>
          <button className="btn btn-primary" onClick={doRescan} disabled={busy}>
            {busy ? t("common.scanning") : t("settings.rescan")}
          </button>
        </div>
        {msg && <p className="muted" style={{ marginTop: 12 }}>{msg}</p>}
      </div>

      <div className="series-block" style={{ padding: 16, marginTop: 16 }}>
        <label className="muted" style={{ display: "block", marginBottom: 6, fontSize: 12.5 }}>
          {t("settings.forceRefreshHelp")}
        </label>
        <button className="btn" onClick={doForceRefresh} disabled={forcing}>
          {forcing ? t("settings.forcing") : t("settings.forceRefresh")}
        </button>
      </div>
    </div>
  );
}
