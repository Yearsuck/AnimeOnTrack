import { useEffect, useState } from "react";
import {
  backupStatus,
  connectDrive,
  disconnectDrive,
  backupNow,
  restoreLatest,
  setGoogleCredentials,
} from "../../api";
import { useT } from "../../i18n";
import type { BackupStatus } from "../../types";

/// Setup form for Google OAuth client credentials.
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
/// restore the latest backup. Self-contained so it drops into Settings cleanly.
export function BackupCard() {
  const t = useT();
  const [status, setStatus] = useState<BackupStatus | null>(null);
  type BusyKind = "connect" | "disconnect" | "backup" | "restore";
  const [busy, setBusy] = useState<null | BusyKind>(null);
  const [error, setError] = useState<string | null>(null);
  const [googleOpen, setGoogleOpen] = useState(false);

  useEffect(() => {
    backupStatus()
      .then((s) => {
        setStatus(s);
        if (!s.configured) {
          setGoogleOpen(true);
        }
      })
      .catch((e) => setError(String(e)));
  }, []);

  const run = async (kind: BusyKind, fn: () => Promise<unknown>) => {
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
      <p className="settings-section-desc">{t("settings.backupDesc")}</p>

      {status.configured && (
        <>
          {!status.connected ? (
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
              {status.last_error && (
                <p className="muted text-sm text-danger">
                  {t("settings.backupLastError", { error: status.last_error })}
                </p>
              )}
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
                <button className="btn btn-ghost" disabled={busy !== null} onClick={() => run("disconnect", disconnectDrive)}>
                  {t("settings.backupDisconnect")}
                </button>
              </div>
            </>
          )}
        </>
      )}

      <details
        className="settings-details"
        open={googleOpen}
        onToggle={(e) => setGoogleOpen((e.currentTarget as HTMLDetailsElement).open)}
      >
        <summary className="settings-details-summary">
          {t("settings.backupAdvancedGoogle")}
        </summary>
        <div className="settings-details-body">
          <GoogleCredentialsForm
            onConfigured={(newStatus) => {
              setStatus(newStatus);
              if (newStatus.configured) {
                setGoogleOpen(false);
              }
            }}
          />
        </div>
      </details>

      {error && (
        <p className="muted text-sm text-danger">
          {t("settings.backupError", { msg: error })}
        </p>
      )}
    </div>
  );
}
