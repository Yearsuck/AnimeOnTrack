// Run with: node --test tools/sandbox
import test from 'node:test';
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { stripCloudSettings } from './sanitize.mjs';

test('strips only the Google Drive / backup settings', () => {
  const db = new DatabaseSync(':memory:');
  db.exec('CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT)');
  const insert = db.prepare('INSERT INTO settings VALUES (?, ?)');
  const doomed = ['google_client_id', 'google_client_secret', 'google_refresh_token', 'gdrive_file_id', 'backup_signature', 'backup_last_at'];
  // Look-alikes that must survive: `_` is a LIKE wildcard, and a prefix match must not be a substring match.
  const kept = ['active_site_id', 'banned_genres', 'mirror_urls:animeytx', 'googleish', 'gdrivefoo', 'my_backup_note', 'catalog_sync_state'];
  for (const k of [...doomed, ...kept]) insert.run(k, 'v');

  assert.equal(stripCloudSettings(db), doomed.length);
  const left = db.prepare('SELECT key FROM settings ORDER BY key').all().map((r) => r.key).sort();
  assert.deepEqual(left, [...kept].sort());
  db.close();
});

test('is a no-op on a DB without cloud settings', () => {
  const db = new DatabaseSync(':memory:');
  db.exec('CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT)');
  db.prepare('INSERT INTO settings VALUES (?, ?)').run('active_site_id', 'tioanime');
  assert.equal(stripCloudSettings(db), 0);
  db.close();
});
