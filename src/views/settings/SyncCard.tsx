import { useEffect, useState } from "react";
import { SettingsRow } from "./SettingsRow";
import { Chip } from "../../components/Chip";
import { setSyncTransport, syncNow, syncStatus } from "../../api";
import { useT } from "../../i18n";
import type { SyncStatus, SyncTransport } from "../../types";

export function SyncCard() {
  const t = useT();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [selectedTransport, setSelectedTransport] = useState<SyncTransport | null>(null);
  const [folderInput, setFolderInput] = useState("");
  const [savingFolder, setSavingFolder] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    syncStatus()
      .then((s) => {
        setStatus(s);
        setFolderInput(s.folder ?? "");
      })
      .catch((e) => setError(String(e)));
  }, []);

  if (!status) return null;

  const currentTransport = selectedTransport ?? status.transport;

  const handleSelectTransport = async (transport: SyncTransport) => {
    setError(null);
    if (transport === "folder") {
      setSelectedTransport("folder");
      if (status.folder) {
        try {
          const res = await setSyncTransport("folder", status.folder);
          setStatus(res);
          setSelectedTransport(null);
        } catch (e) {
          setError(String(e));
        }
      }
      return;
    }

    setSelectedTransport(transport);
    try {
      const res = await setSyncTransport(transport, status.folder);
      setStatus(res);
      setSelectedTransport(null);
    } catch (e) {
      setError(String(e));
      setSelectedTransport(null);
    }
  };

  const handleSaveFolder = async () => {
    setSavingFolder(true);
    setError(null);
    try {
      const res = await setSyncTransport("folder", folderInput.trim());
      setStatus(res);
      setSelectedTransport(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingFolder(false);
    }
  };

  const handleSyncNow = async () => {
    setSyncing(true);
    setError(null);
    try {
      const report = await syncNow();
      const updated = await syncStatus().catch(() => ({
        ...status,
        last_at: report.at,
        last_report: report,
        last_error: null,
        running: false,
      }));
      setStatus(updated);
    } catch (e) {
      setError(String(e));
      void syncStatus().then(setStatus).catch(() => {});
    } finally {
      setSyncing(false);
    }
  };

  const isBusy = syncing || status.running;
  const canSync = status.transport !== "off" && !isBusy;
  const lastSyncDate = status.last_at ? new Date(status.last_at * 1000).toLocaleString() : null;

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{t("settings.syncHeading")}</h3>
      <p className="settings-section-desc">{t("settings.syncDesc")}</p>

      <SettingsRow label={t("settings.syncTransportLabel")} labelId="sf-settings-syncTransportLabel">
        <div role="group" aria-labelledby="sf-settings-syncTransportLabel" className="row">
          <Chip
            active={currentTransport === "off"}
            disabled={isBusy}
            onClick={() => handleSelectTransport("off")}
          >
            {t("settings.syncTransportOff")}
          </Chip>
          <Chip
            active={currentTransport === "folder"}
            disabled={isBusy}
            onClick={() => handleSelectTransport("folder")}
          >
            {t("settings.syncTransportFolder")}
          </Chip>
          <Chip
            active={currentTransport === "drive"}
            disabled={isBusy || !status.drive_connected}
            onClick={() => handleSelectTransport("drive")}
          >
            {t("settings.syncTransportDrive")}
          </Chip>
        </div>
        {!status.drive_connected && (
          <p className="muted text-sm">{t("settings.syncDriveNotConnected")}</p>
        )}
      </SettingsRow>

      {currentTransport === "folder" && (
        <SettingsRow label={t("settings.syncFolderLabel")} labelId="sf-settings-syncFolderLabel">
          <div role="group" aria-labelledby="sf-settings-syncFolderLabel" className="row settings-cred-row">
            <input
              className="input settings-url-input"
              value={folderInput}
              onChange={(e) => setFolderInput(e.target.value)}
              placeholder={t("settings.syncFolderPlaceholder")}
              aria-label={t("settings.syncFolderLabel")}
              spellCheck={false}
              autoComplete="off"
            />
            <button
              className="btn btn-primary"
              disabled={savingFolder || !folderInput.trim()}
              onClick={handleSaveFolder}
            >
              {savingFolder ? t("common.saving") : t("settings.syncFolderSave")}
            </button>
          </div>
          <p className="muted text-sm">{t("settings.syncFolderHint")}</p>
        </SettingsRow>
      )}

      <div className="row">
        <button
          className="btn btn-primary"
          disabled={!canSync}
          onClick={handleSyncNow}
        >
          {isBusy ? t("settings.syncWorking") : t("settings.syncNow")}
        </button>
      </div>

      <p className="muted text-sm">
        {lastSyncDate ? t("settings.syncLast", { when: lastSyncDate }) : t("settings.syncNever")}
        {status.last_report
          ? ` · ${t("settings.syncReportSummary", {
              local: status.last_report.local_changes,
              remote: status.last_report.remote_changes,
            })}`
          : ""}
      </p>

      {status.last_error && (
        <p className="muted text-sm text-danger">
          {t("settings.syncLastError", { error: status.last_error })}
        </p>
      )}

      <p className="muted text-xs">
        {t("settings.syncDeviceId", { id: status.device_id })}
      </p>

      {error && (
        <p className="muted text-sm text-danger">
          {t("settings.syncError", { msg: error })}
        </p>
      )}
    </div>
  );
}
