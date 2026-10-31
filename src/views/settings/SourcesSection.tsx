import { useEffect, useState } from "react";
import {
  scanAiring,
  rescanAiring,
  getMirrors,
  setMirrors,
  listSites,
  getActiveSite,
  setActiveSite,
  getSitePriority,
  setSitePriority,
} from "../../api";
import { useT } from "../../i18n";
import { BusyLabel } from "../../components/BusyLabel";
import type { SiteSummary } from "../../types";

interface SourcesSectionProps {
  onSiteChanged?: (site: SiteSummary) => void;
}

export function SourcesSection({ onSiteChanged }: SourcesSectionProps) {
  const t = useT();
  const [firstUrl, setFirstUrl] = useState("https://animeyt.cc");
  const [mirrorsText, setMirrorsText] = useState("");
  const [busy, setBusy] = useState(false);
  const [savingMirrors, setSavingMirrors] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  const [sites, setSites] = useState<SiteSummary[]>([]);
  const [activeSite, setActiveSiteState] = useState<SiteSummary | null>(null);
  // The site picked in the <select> but not yet confirmed — drives the
  // confirmation panel below it. Separate from activeSite so changing the
  // dropdown never switches anything by itself.
  const [pendingSiteId, setPendingSiteId] = useState<string | null>(null);
  const [switchingSite, setSwitchingSite] = useState(false);
  const [priorityOrder, setPriorityOrder] = useState<string[]>([]);
  const [savingPriority, setSavingPriority] = useState(false);

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

  async function loadPriority() {
    try {
      const p = await getSitePriority();
      setPriorityOrder(p);
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    }
  }

  useEffect(() => {
    loadMirrors();
    loadSites();
    loadPriority();
  }, []);

  async function movePriority(index: number, direction: -1 | 1) {
    const targetIndex = index + direction;
    if (targetIndex < 0 || targetIndex >= priorityOrder.length) return;
    const newOrder = [...priorityOrder];
    const [moved] = newOrder.splice(index, 1);
    newOrder.splice(targetIndex, 0, moved);
    setSavingPriority(true);
    setMsg(null);
    try {
      await setSitePriority(newOrder);
      await loadPriority();
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
    } finally {
      setSavingPriority(false);
    }
  }

  const pendingSite = sites.find((s) => s.id === pendingSiteId) ?? null;

  async function confirmSiteSwitch() {
    if (!pendingSite) return;
    setSwitchingSite(true);
    setMsg(null);
    try {
      const result = await setActiveSite(pendingSite.id);
      // The backend is already on the new site: align the local state first so
      // a failing scan cannot leave Settings showing the old one.
      setActiveSiteState(result.site);
      setPendingSiteId(null);
      try {
        await loadMirrors();
      } catch (mirrorErr) {
        console.error("loadMirrors failed after a site switch", mirrorErr);
      }
      try {
        const s = await scanAiring(result.site.default_base_url);
        setMsg(t("settings.siteChanged", { site: result.site.name, count: s.length }));
      } catch (scanErr) {
        setMsg(t("errors.generic", { detail: String(scanErr) }));
      }
      // After the scan: App's handler navigates to the airing grid, which must
      // load the freshly scanned listing rather than race it.
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
    // Cleared BEFORE the await, not only on success: `set_mirrors` rejects
    // the whole list if any entry fails `is_safe_external_url` (a bare
    // domain typed without `https://` is enough — the URL parser needs a
    // scheme), and without this the previous "Lista de webs guardada."
    // stayed on screen under the textarea claiming a save that never
    // happened.
    setMsg(null);
    try {
      const urls = mirrorsText.split("\n").map((u) => u.trim()).filter(Boolean);
      await setMirrors(urls);
      // Even a successful save can persist something other than what was
      // typed: `set_mirrors` force-reinserts the active site's `base_url`
      // when the edit dropped it (commands/mirrors.rs refuses to strand the
      // active site). Re-read the stored list — same call the mount does —
      // so the textarea shows what is actually saved instead of the edit
      // the backend partly overrode.
      await loadMirrors();
      setMsg(t("settings.mirrorsSaved"));
    } catch (e) {
      setMsg(t("errors.generic", { detail: String(e) }));
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

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.siteHeading")}</h3>
      <p className="settings-section-desc">{t("settings.sourcesIntro")}</p>

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.sourcesActiveLabel")}</label>
        <select
          className="input settings-select"
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
          <div className="settings-inline-panel">
            <p className="settings-confirm-text">
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

      <div className="settings-field">
        <label className="settings-field-label">{t("settings.sitePriorityHeading")}</label>
        <p className="settings-section-desc">{t("settings.sitePriorityHint")}</p>
        {priorityOrder.map((siteId, index) => {
          const site = sites.find((s) => s.id === siteId);
          const name = site ? site.name : siteId;
          return (
            <div key={siteId} className="row settings-cred-row">
              <span>{name}</span>
              <span className="spacer" />
              <button
                type="button"
                className="btn btn-sm"
                disabled={index === 0 || savingPriority}
                onClick={() => movePriority(index, -1)}
                aria-label={t("settings.sitePriorityMoveUp", { site: name })}
              >
                {t("settings.sitePriorityUp")}
              </button>
              <button
                type="button"
                className="btn btn-sm"
                disabled={index === priorityOrder.length - 1 || savingPriority}
                onClick={() => movePriority(index, 1)}
                aria-label={t("settings.sitePriorityMoveDown", { site: name })}
              >
                {t("settings.sitePriorityDown")}
              </button>
            </div>
          );
        })}
      </div>

      <details className="settings-details">
        <summary className="settings-details-summary">{t("settings.sourcesAdvanced")}</summary>
        <div className="settings-details-body">
          <div className="settings-field">
            <label className="settings-field-label">{t("settings.addNewSiteDesc")}</label>
            <div className="row">
              <input
                className="input settings-url-input"
                value={firstUrl}
                onChange={(e) => setFirstUrl(e.target.value)}
                placeholder="https://..."
                aria-label={t("settings.addNewSiteLabel")}
              />
              <button className="btn btn-primary" onClick={addFirstUrl} disabled={busy} aria-busy={busy}>
                <BusyLabel busy={busy} idle={t("common.scan")} busyText={t("common.scanning")} />
              </button>
            </div>
          </div>

          <div className="settings-field">
            <label className="settings-field-label">
              {t("settings.mirrorsDesc", { site: activeSite?.name ?? t("settings.activeSiteFallback") })}
            </label>
            <textarea
              className="input textarea-mono"
              rows={5}
              value={mirrorsText}
              onChange={(e) => setMirrorsText(e.target.value)}
            />
            <div className="row settings-actions">
              <button className="btn" onClick={saveMirrors} disabled={savingMirrors}>
                {savingMirrors ? t("settings.saving") : t("settings.saveMirrors")}
              </button>
              <button className="btn btn-primary" onClick={doRescan} disabled={busy} aria-busy={busy}>
                <BusyLabel busy={busy} idle={t("settings.rescan")} busyText={t("common.scanning")} />
              </button>
            </div>
          </div>
        </div>
      </details>

      {msg && <p className="muted settings-msg">{msg}</p>}
    </div>
  );
}
