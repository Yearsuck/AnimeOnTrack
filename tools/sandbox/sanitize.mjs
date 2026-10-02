// Settings that make the app talk to YOUR Google Drive: the OAuth client id/secret and tokens
// (`google_*`), the remembered backup file id (`gdrive_*`), and the backup bookkeeping such as the
// "changed since last backup" signature (`backup_*`). On startup the app runs an opportunistic
// cloud backup, so a sandbox copy that kept them could upload sandbox data over the real cloud
// backup, or restore from it. The sandbox DB must never carry them.
//
// `_` is a LIKE wildcard, so it is escaped; `ESCAPE '\'` makes the prefixes exact.
export const CLOUD_SETTING_PATTERNS = ['google\\_%', 'gdrive\\_%', 'backup\\_%'];

/** Delete every cloud-backup setting from an open `node:sqlite` DatabaseSync. Returns how many. */
export function stripCloudSettings(db) {
  const del = db.prepare("DELETE FROM settings WHERE key LIKE ? ESCAPE '\\'");
  let removed = 0;
  for (const pattern of CLOUD_SETTING_PATTERNS) removed += Number(del.run(pattern).changes);
  return removed;
}
