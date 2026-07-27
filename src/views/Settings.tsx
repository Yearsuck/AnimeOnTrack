import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  scanAiring,
  rescanAiring,
  getMirrors,
  setMirrors,
  refresh,
  listSites,
  getActiveSite,
  setActiveSite,
  backupStatus,
  connectDrive,
  disconnectDrive,
  backupNow,
  restoreLatest,
  setGoogleCredentials,
  importLibraryToActiveSite,
  uninstallApp,
} from "../api";
import { LANGS, useLang, useT } from "../i18n";
import { THEMES, useTheme } from "../theme";
import type { SiteSummary, BackupStatus } from "../types";

/// Setup form shown until a Google OAuth client is configured.
///
/// The credentials used to be compile-time only, which meant the backup was
/// unreachable for anyone who hadn't edited `.cargo/config.toml` and rebuilt —
/// i.e. everyone. A Desktop-type OAuth client's id and secret are documented
/// by Google as non-confidential for installed apps (the reason this flow uses
/// PKCE), so accepting them at runtime and storing them next to the refresh
/// token adds no exposure the app didn't already have.
function GoogleCredentialsForm({ onConfigured }: { onConfigured: (s: BackupStatus) => void }) {
  const t = useT();
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      onConfigured(await setGoogleCredentials(clientId, clientSecret));
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <p className="muted text-sm">
        {t("settings.backupSetupSteps")}
      </p>
      <div className="row settings-cred-row">
        <input
          className="input"
          value={clientId}
          onChange={(e) => setClientId(e.target.value)}
          placeholder={t("settings.backupClientId")}
          aria-label={t("settings.backupClientId")}
          spellCheck={false}
          autoComplete="off"
        />
        <input
          className="input"
          type="password"
          value={clientSecret}
          onChange={(e) => setClientSecret(e.target.value)}
          placeholder={t("settings.backupClientSecret")}
          aria-label={t("settings.backupClientSecret")}
          spellCheck={false}
          autoComplete="off"
        />
        <button
          className="btn btn-primary"
          disabled={saving || !clientId.trim() || !clientSecret.trim()}
          onClick={save}
        >
          {saving ? t("common.saving") : t("settings.backupSaveCredentials")}
        </button>
      </div>
      {error && (
        <p className="muted text-sm text-danger">
          {t("settings.backupError", { msg: error })}
        </p>
      )}
    </div>
  );
}

/// "Copia de seguridad" card: connect/disconnect Google Drive, back up now,
/// restore the latest backup. Self-contained (fetches its own status) so it
/// drops into Settings without threading state through the parent.
function BackupCard() {
  const t = useT();
  const [status, setStatus] = useState<BackupStatus | null>(null);
  const [busy, setBusy] = useState<null | "connect" | "backup" | "restore">(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    backupStatus().then(setStatus).catch((e) => setError(String(e)));
  }, []);

  const run = async (kind: "connect" | "backup" | "restore", fn: () => Promise<unknown>) => {
    setBusy(kind);
    setError(null);
    try {
      const r = await fn();
      if (kind !== "restore") setStatus(r as BackupStatus);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  if (!status) return null;

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.backupHeading")}</h3>
      <p className="settings-section-desc">{t("settings.backupIntro")}</p>

      {!status.configured ? (
        <GoogleCredentialsForm onConfigured={setStatus} />
      ) : !status.connected ? (
        <button
          className="btn btn-primary"
          disabled={busy !== null}
          onClick={() => run("connect", connectDrive)}
        >
          {busy === "connect" ? t("settings.backupConnecting") : t("settings.backupConnect")}
        </button>
      ) : (
        <>
          <p className="muted">{t("settings.backupConnected")}</p>
          <p className="muted text-sm">
            {status.last_at ? t("settings.backupLast", { when: status.last_at }) : t("settings.backupNever")}
            {status.size_bytes
              ? ` · ${t("settings.backupSize", { size: `${Math.round(status.size_bytes / 1024)} KB` })}`
              : ""}
          </p>
          <div className="row">
            <button className="btn btn-primary" disabled={busy !== null} onClick={() => run("backup", backupNow)}>
              {busy === "backup" ? t("settings.backupWorking") : t("settings.backupNow")}
            </button>
            <button
              className="btn"
              disabled={busy !== null}
              onClick={() => {
                if (confirm(t("settings.backupRestoreConfirm"))) run("restore", restoreLatest);
              }}
            >
              {busy === "restore" ? t("settings.backupRestoring") : t("settings.backupRestore")}
            </button>
            <button className="btn btn-ghost" disabled={busy !== null} onClick={() => run("connect", disconnectDrive)}>
              {t("settings.backupDisconnect")}
            </button>
          </div>
        </>
      )}
      {error && (
        <p className="muted text-sm text-danger">
          {t("settings.backupError", { msg: error })}
        </p>
      )}
    </div>
  );
}

/// "Traer mi biblioteca a este sitio": resolves your whole canonical library
/// onto the active site (searches + links the ones not present here, replaying
/// watched progress). Self-contained; listens to the paced import's progress.
function LibraryImportCard() {
  const t = useT();
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number; title: string } | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    const un = listen<{ done: number; total: number; title: string }>(
      "library-import-progress",
      (e) => setProgress(e.payload)
    );
    return () => {
      un.then((f) => f());
    };
  }, []);

  async function run() {
    setRunning(true);
    setResult(null);
    setProgress(null);
    try {
      const r = await importLibraryToActiveSite();
      setResult(t("settings.importDone", { linked: r.linked, total: r.total, skipped: r.skipped }));
    } catch (e) {
      setResult(String(e));
    } finally {
      setRunning(false);
      setProgress(null);
    }
  }

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.importHeading")}</h3>
      <p className="settings-section-desc">{t("settings.importHelp")}</p>
      <button className="btn btn-primary" onClick={run} disabled={running}>
        {running ? t("settings.importing") : t("settings.importButton")}
      </button>
      {running && progress && progress.total > 0 && (
        <p className="muted settings-msg">
          {t("settings.importProgress", {
            done: progress.done + 1,
            total: progress.total,
            title: progress.title,
          })}
        </p>
      )}
      {result && <p className="muted settings-msg">{result}</p>}
    </div>
  );
}

function UninstallCard() {
  const t = useT();
  // Two-step confirm: the first click reveals the confirmation panel, so the
  // irreversible action is never one stray click away.
  const [confirming, setConfirming] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function doUninstall() {
    setRunning(true);
    setError(null);
    try {
      // On success the backend quits the app, so control never returns here;
      // a rejection means there was nothing to uninstall (dev/unpacked build).
      await uninstallApp();
    } catch (e) {
      setError(String(e));
      setRunning(false);
      setConfirming(false);
    }
  }

  return (
    <div className="settings-section danger-zone">
      <h3 className="settings-section-title">{t("settings.uninstallHeading")}</h3>
      <p className="settings-section-desc">{t("settings.uninstallHelp")}</p>
      {!confirming ? (
        <button className="btn btn-danger" onClick={() => setConfirming(true)}>
          {t("settings.uninstallButton")}
        </button>
      ) : (
        <div className="settings-inline-panel">
          <p className="settings-confirm-text">{t("settings.uninstallConfirm")}</p>
          <div className="row">
            <button className="btn btn-danger" onClick={doUninstall} disabled={running}>
              {running ? t("settings.uninstalling") : t("settings.uninstallConfirmYes")}
            </button>
            <button className="btn" onClick={() => setConfirming(false)} disabled={running}>
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
      {error && <p className="muted settings-msg text-danger">{error}</p>}
    </div>
  );
}

export function Settings({ onSiteChanged }: { onSiteChanged?: (site: SiteSummary) => void }) {
  const t = useT();
  const { lang, setLang } = useLang();
  const { theme, setTheme } = useTheme();
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
    <div className="page settings-page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.settings")}</h2>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">{t("settings.appearanceHeading")}</h3>
        <div className="settings-field">
          <label className="settings-field-label">{t("settings.theme")}</label>
          <div className="row">
            {THEMES.map((th) => (
              <button
                key={th.code}
                type="button"
                className={`chip-toggle ${theme === th.code ? "active" : ""}`}
                onClick={() => setTheme(th.code)}
              >
                {t(th.labelKey)}
              </button>
            ))}
          </div>
        </div>
        <div className="settings-field">
          <label className="settings-field-label">{t("settings.language")}</label>
          <div className="row">
            {LANGS.map((l) => (
              <button
                key={l.code}
                type="button"
                className={`chip-toggle ${lang === l.code ? "active" : ""}`}
                onClick={() => setLang(l.code)}
              >
                {l.label}
              </button>
            ))}
          </div>
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">{t("settings.siteHeading")}</h3>
        <div className="settings-field">
          <label className="settings-field-label">{t("settings.activeSiteHelp")}</label>
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
          <label className="settings-field-label">{t("settings.addNewSiteLabel")}</label>
          <div className="row">
            <input className="input settings-url-input" value={firstUrl} onChange={(e) => setFirstUrl(e.target.value)} />
            <button className="btn btn-primary" onClick={addFirstUrl} disabled={busy}>
              {busy ? t("common.scanning") : t("common.scan")}
            </button>
          </div>
        </div>

        <div className="settings-field">
          <label className="settings-field-label">
            {t("settings.mirrorsLabel", { site: activeSite?.name ?? t("settings.activeSiteFallback") })}
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
            <button className="btn btn-primary" onClick={doRescan} disabled={busy}>
              {busy ? t("common.scanning") : t("settings.rescan")}
            </button>
          </div>
          {msg && <p className="muted settings-msg">{msg}</p>}
        </div>
      </div>

      <LibraryImportCard />

      <BackupCard />

      <div className="settings-section danger-zone">
        <h3 className="settings-section-title">{t("settings.maintenanceHeading")}</h3>
        <p className="settings-section-desc">{t("settings.forceRefreshHelp")}</p>
        <button className="btn" onClick={doForceRefresh} disabled={forcing}>
          {forcing ? t("settings.forcing") : t("settings.forceRefresh")}
        </button>
      </div>

      <UninstallCard />
    </div>
  );
}
