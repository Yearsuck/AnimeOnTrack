import { useEffect, useState } from "react";
import {
  getStorageReport,
  getDownloadQuality,
  setDownloadQuality,
  getAutoDeleteSeenDownloads,
  setAutoDeleteSeenDownloads,
  scanDownloadOrphans,
  deleteDownloadOrphans,
} from "../../api";
import type { StorageReport, OrphanFile } from "../../api";
import { useLang, useT } from "../../i18n";
import { formatBytes } from "../../lib/formatBytes";

function getBasename(filePath: string): string {
  const normalized = filePath.replace(/\\/g, "/");
  const idx = normalized.lastIndexOf("/");
  return idx >= 0 ? normalized.slice(idx + 1) : normalized;
}

export function StorageCard() {
  const t = useT();
  const { lang } = useLang();

  const [initialLoading, setInitialLoading] = useState(true);
  // A control whose stored value could not be read stays disabled rather
  // than showing a default as if it were the saved setting.
  const [qualityLoaded, setQualityLoaded] = useState(false);
  const [autoDeleteLoaded, setAutoDeleteLoaded] = useState(false);

  // Storage report state
  const [report, setReport] = useState<StorageReport | null>(null);
  const [loadingReport, setLoadingReport] = useState(false);
  const [reportError, setReportError] = useState<string | null>(null);

  // Quality preference state
  const [quality, setQuality] = useState("best");
  const [savingQuality, setSavingQuality] = useState(false);
  const [qualityError, setQualityError] = useState<string | null>(null);

  // Auto-delete toggle state
  const [autoDelete, setAutoDelete] = useState(true);
  const [savingAutoDelete, setSavingAutoDelete] = useState(false);
  const [autoDeleteError, setAutoDeleteError] = useState<string | null>(null);

  // Orphan files state
  const [orphanFiles, setOrphanFiles] = useState<OrphanFile[] | null>(null);
  const [scanningOrphans, setScanningOrphans] = useState(false);
  const [deletingOrphans, setDeletingOrphans] = useState(false);
  const [confirmDeleteOrphans, setConfirmDeleteOrphans] = useState(false);
  const [orphansMessage, setOrphansMessage] = useState<string | null>(null);
  const [orphansError, setOrphansError] = useState<string | null>(null);

  const loadReport = async () => {
    setLoadingReport(true);
    setReportError(null);
    try {
      const rep = await getStorageReport();
      setReport(rep);
    } catch (err) {
      setReportError(String(err));
    } finally {
      setLoadingReport(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    Promise.allSettled([
      getStorageReport(),
      getDownloadQuality(),
      getAutoDeleteSeenDownloads(),
    ]).then(([rep, qual, ad]) => {
      if (cancelled) return;
      if (rep.status === "fulfilled") setReport(rep.value);
      else setReportError(String(rep.reason));
      if (qual.status === "fulfilled") {
        setQuality(qual.value || "best");
        setQualityLoaded(true);
      } else setQualityError(String(qual.reason));
      if (ad.status === "fulfilled") {
        setAutoDelete(ad.value);
        setAutoDeleteLoaded(true);
      } else setAutoDeleteError(String(ad.reason));
      setInitialLoading(false);
    });

    return () => {
      cancelled = true;
    };
  }, []);

  const handleQualityChange = async (newQuality: string) => {
    const prev = quality;
    setQuality(newQuality);
    setSavingQuality(true);
    setQualityError(null);
    try {
      await setDownloadQuality(newQuality);
    } catch (err) {
      setQuality(prev);
      setQualityError(String(err));
    } finally {
      setSavingQuality(false);
    }
  };

  const handleAutoDeleteChange = async (enabled: boolean) => {
    const prev = autoDelete;
    setAutoDelete(enabled);
    setSavingAutoDelete(true);
    setAutoDeleteError(null);
    try {
      await setAutoDeleteSeenDownloads(enabled);
    } catch (err) {
      setAutoDelete(prev);
      setAutoDeleteError(String(err));
    } finally {
      setSavingAutoDelete(false);
    }
  };

  const handleScanOrphans = async () => {
    setScanningOrphans(true);
    setOrphansError(null);
    setOrphansMessage(null);
    setConfirmDeleteOrphans(false);
    try {
      const files = await scanDownloadOrphans();
      setOrphanFiles(files);
    } catch (err) {
      setOrphansError(String(err));
    } finally {
      setScanningOrphans(false);
    }
  };

  const handleDeleteOrphans = async () => {
    if (!confirmDeleteOrphans) {
      setConfirmDeleteOrphans(true);
      return;
    }
    if (!orphanFiles || orphanFiles.length === 0) return;

    setDeletingOrphans(true);
    setConfirmDeleteOrphans(false);
    setOrphansError(null);
    setOrphansMessage(null);
    try {
      const paths = orphanFiles.map((f) => f.path);
      const reclaimedBytes = await deleteDownloadOrphans(paths);
      setOrphansMessage(t("storage.orphansReclaimed", { size: formatBytes(reclaimedBytes, lang) }));
      // Rescan rather than assume every file went: the backend skips any
      // that stopped being an orphan or could not be removed.
      setOrphanFiles(await scanDownloadOrphans());
      await loadReport();
    } catch (err) {
      setOrphansError(String(err));
    } finally {
      setDeletingOrphans(false);
    }
  };

  if (initialLoading) return null;

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("storage.heading")}</h3>
      <p className="settings-section-desc">{t("storage.desc")}</p>

      {/* Storage usage breakdown */}
      <div className="settings-field" role="group" aria-labelledby="storage-usageHeading">
        <span className="settings-field-label" id="storage-usageHeading">{t("storage.usageHeading")}</span>
        {report && (
          <div className="storage-usage-list">
            <div className="storage-usage-row">
              <span>{t("storage.usageDownloads")}</span>
              <span className="storage-usage-val">{formatBytes(report.downloads_bytes, lang)}</span>
            </div>
            <div className="storage-usage-row">
              <span>{t("storage.usageDatabase")}</span>
              <span className="storage-usage-val">{formatBytes(report.sqlite_bytes, lang)}</span>
            </div>
            <div className="storage-usage-row">
              <span>{t("storage.usageCovers")}</span>
              <span className="storage-usage-val">{formatBytes(report.covers_bytes, lang)}</span>
            </div>
            <div className="storage-usage-row">
              <span>{t("storage.usageLogs")}</span>
              <span className="storage-usage-val">{formatBytes(report.logs_bytes, lang)}</span>
            </div>
          </div>
        )}
        <div className="row">
          <button
            type="button"
            className="btn btn-sm"
            disabled={loadingReport}
            onClick={loadReport}
          >
            {loadingReport ? t("storage.refreshing") : t("storage.refresh")}
          </button>
        </div>
        {reportError && <p className="muted text-sm text-danger">{reportError}</p>}
      </div>

      {/* Download quality selection */}
      <div className="settings-field">
        <label className="settings-field-label" htmlFor="storage-quality-select">
          {t("storage.qualityLabel")}
        </label>
        <select
          id="storage-quality-select"
          className="input settings-select"
          value={quality}
          disabled={savingQuality || !qualityLoaded}
          onChange={(e) => handleQualityChange(e.target.value)}
        >
          <option value="best">{t("storage.qualityBest")}</option>
          <option value="1080">{t("storage.quality1080")}</option>
          <option value="720">{t("storage.quality720")}</option>
          <option value="480">{t("storage.quality480")}</option>
        </select>
        <p className="settings-field-hint">{t("storage.qualityHint")}</p>
        {qualityError && <p className="muted text-sm text-danger">{qualityError}</p>}
      </div>

      {/* Auto-delete watched downloads toggle */}
      <div className="settings-field" role="group" aria-labelledby="storage-autoDeleteLabel">
        <span className="settings-field-label" id="storage-autoDeleteLabel">{t("storage.autoDeleteLabel")}</span>
        <div className="row">
          <button
            type="button"
            className={`chip-toggle ${autoDelete ? "active" : ""}`}
            aria-pressed={autoDelete}
            disabled={savingAutoDelete || !autoDeleteLoaded}
            onClick={() => handleAutoDeleteChange(true)}
          >
            {t("storage.autoDeleteOn")}
          </button>
          <button
            type="button"
            className={`chip-toggle ${!autoDelete ? "active" : ""}`}
            aria-pressed={!autoDelete}
            disabled={savingAutoDelete || !autoDeleteLoaded}
            onClick={() => handleAutoDeleteChange(false)}
          >
            {t("storage.autoDeleteOff")}
          </button>
        </div>
        <p className="settings-field-hint">{t("download.autoDeleteHint")}</p>
        {autoDeleteError && <p className="muted text-sm text-danger">{autoDeleteError}</p>}
      </div>

      {/* Orphan files scan & deletion */}
      <div className="settings-field" role="group" aria-labelledby="storage-orphansHeading">
        <span className="settings-field-label" id="storage-orphansHeading">{t("storage.orphansHeading")}</span>
        <p className="settings-field-hint">{t("storage.orphansDesc")}</p>
        <div className="row">
          <button
            type="button"
            className="btn"
            disabled={scanningOrphans || deletingOrphans}
            onClick={handleScanOrphans}
          >
            {scanningOrphans ? t("storage.orphansScanning") : t("storage.orphansScan")}
          </button>
        </div>

        {orphanFiles !== null && (
          <div className="settings-inline-panel">
            {orphanFiles.length === 0 ? (
              <p className="muted text-sm">{t("storage.orphansNone")}</p>
            ) : (
              <>
                <p className="text-sm">
                  {t("storage.orphansFound", {
                    count: orphanFiles.length,
                    size: formatBytes(
                      orphanFiles.reduce((acc, f) => acc + f.size_bytes, 0),
                      lang
                    ),
                  })}
                </p>
                <div className="storage-orphans-list">
                  {orphanFiles.map((f) => (
                    <div key={f.path} className="storage-orphan-item">
                      <span className="storage-orphan-name" title={f.path}>
                        {getBasename(f.path)}
                      </span>
                      <span className="tabular">{formatBytes(f.size_bytes, lang)}</span>
                    </div>
                  ))}
                </div>
                <div className="row">
                  <button
                    type="button"
                    className={`btn ${confirmDeleteOrphans ? "btn-danger" : ""}`}
                    disabled={deletingOrphans}
                    onClick={handleDeleteOrphans}
                  >
                    {deletingOrphans
                      ? t("storage.orphansDeleting")
                      : confirmDeleteOrphans
                      ? t("storage.orphansConfirmDelete")
                      : t("storage.orphansDelete")}
                  </button>
                  {confirmDeleteOrphans && (
                    <button
                      type="button"
                      className="btn btn-ghost"
                      disabled={deletingOrphans}
                      onClick={() => setConfirmDeleteOrphans(false)}
                    >
                      {t("common.cancel")}
                    </button>
                  )}
                </div>
              </>
            )}
          </div>
        )}

        {orphansMessage && <p className="muted settings-msg">{orphansMessage}</p>}
        {orphansError && <p className="muted text-sm text-danger">{orphansError}</p>}
      </div>
    </div>
  );
}
