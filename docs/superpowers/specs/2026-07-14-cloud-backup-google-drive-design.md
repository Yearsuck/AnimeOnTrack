# Copia de seguridad en la nube (Google Drive, estilo WhatsApp)

Fecha: 2026-07-14 · Estado: diseñado (aprobado por el usuario vía respuestas de diseño)

## Problema / objetivo

Los datos de la app viven en un único SQLite local
(`app_data_dir()/animeontrack.sqlite`, hoy `%APPDATA%\com.ernes.aot-scaffold\`). Si el usuario
cambia de equipo o reinstala, lo pierde todo (135 seguidas, 2000+ episodios vistos, 566 "Ya vistas",
listas de Descubrir). Se quiere un backup/restore en la nube **estilo WhatsApp**: copia en una
carpeta oculta propia de la app en el Drive del usuario, automática, y restaurable en una instalación
nueva.

## Decisiones (del usuario, 2026-07-14)

1. **Proveedor:** Google Drive `appDataFolder` (carpeta oculta por-app, invisible en la UI de Drive).
   El usuario crea el OAuth Client tipo *Desktop* en Google Cloud y aporta `client_id`/`client_secret`.
2. **Cifrado:** ninguno en v1 (el sqlite se sube tal cual; está en el Drive privado del usuario y la
   app no guarda secretos serios). Arquitectura dejada de forma que añadir cifrado luego sea local.
3. **Cadencia:** automático (arranque + tras `refresh`, throttled 24h, solo si cambió) **más** botón
   manual.
4. **Retención:** una sola copia, sobrescrita (un `fileId` estable).

## Contexto técnico REAL (verificado en el código)

- `src-tauri/Cargo.toml`: ya están `reqwest = { 0.12, rustls-tls, json }`, `tokio = { time, sync,
  macros }`, `rusqlite = { bundled }`, `serde`. **reqwest contra Google es correcto** — la regla del
  CLAUDE.md prohíbe reqwest solo contra el sitio pirata tras Cloudflare, no contra APIs normales.
- `src-tauri/src/lib.rs:24-49` `.setup(|app|)`: resuelve `app.path().app_data_dir()`, `Db::open`,
  y `app.manage(AppState { db: Mutex<Db>, ... })`. Aquí es donde hay que aplicar un restore staged
  **antes** de `Db::open`.
- `AppState` (`src-tauri/src/commands.rs:31`) = `{ db: Mutex<Db>, source_id, active_site_id, ... }`.
- `db.rs` ya tiene `settings` (tabla key/value) con `get_setting`/`set_setting` — reutilizada para
  `gdrive_refresh_token`, `gdrive_file_id`, `backup_last_at`, `backup_signature`.
- `tauri-plugin-opener` (`lib.rs:23`) ya inicializado → abre el navegador del sistema para OAuth.
- El SQLite se abre en modo normal; el backup NO debe copiar bytes de un fichero con conexión
  abierta (journal/WAL inconsistente) → usar `VACUUM INTO`.

## Diseño

### Módulos nuevos (`src-tauri/src/backup/`)

- **`credentials.rs`** — `pub fn client_id() -> Option<&'static str>` / `client_secret()` vía
  `option_env!("AOT_GOOGLE_CLIENT_ID")` / `option_env!("AOT_GOOGLE_CLIENT_SECRET")`;
  `pub fn is_configured() -> bool`. Las credenciales se ponen en `.cargo/config.toml` (`[env]`),
  gitignoreado; se versiona `.cargo/config.toml.example`.
- **`oauth.rs`** — flujo OAuth Desktop con loopback + PKCE. Partes **puras** (testeables):
  - `pkce_pair() -> (verifier, challenge)` (verifier = 43–128 chars base64url; challenge =
    base64url(SHA256(verifier)) sin padding).
  - `build_auth_url(client_id, redirect_uri, challenge) -> String` (scope
    `https://www.googleapis.com/auth/drive.appdata`, `response_type=code`, `access_type=offline`,
    `prompt=consent`, `code_challenge_method=S256`).
  - `parse_redirect_line(&str) -> Option<AuthCode|Error>` (parsea `GET /?code=...&scope=...` o
    `GET /?error=access_denied`).
  - `parse_token_response(json) -> TokenSet` (access_token, refresh_token, expires_in).
  Parte **con red** (aislada, no testeada offline): `run_loopback_and_get_code(...)` (levanta un
  `tokio::net::TcpListener` en `127.0.0.1:0`, abre el navegador, lee UNA petición, responde un HTML
  "puedes cerrar esta pestaña", devuelve el code), `exchange_code(...)` y `refresh_access_token(...)`
  (reqwest POST a `https://oauth2.googleapis.com/token`, con `client_id`+`client_secret`+PKCE).
- **`drive.rs`** — REST de Drive (reqwest, `Authorization: Bearer`):
  - `find_backup_file(token) -> Option<fileId>` (query `spaces=appDataFolder`,
    `q=name='animeontrack.sqlite'`).
  - `create_backup(token, bytes) -> fileId` (multipart: metadata `{name, parents:['appDataFolder']}`
    + media) y `update_backup(token, fileId, bytes)` (PATCH `uploadType=media`).
  - `get_metadata(token, fileId) -> {size, modifiedTime}` para mostrar en la UI.
  - `download_backup(token, fileId) -> Vec<u8>` (`alt=media`).
- **`mod.rs`** — orquestación:
  - `snapshot_bytes(db_path) -> Vec<u8>`: hace `VACUUM INTO` a un fichero temporal en el scratchpad
    de la app, lo lee y lo borra.
  - `db_signature(&Db) -> String`: `format!("{series}:{eps}:{max_ep_id}:{max_seen_at}")` — barato,
    detecta "cambió desde el último backup".
  - `validate_restore_bytes(&[u8]) -> Result<()>`: escribe a temp, abre con rusqlite,
    `PRAGMA integrity_check` debe ser `ok` **y** deben existir las tablas `sources` y `series`;
    rechaza cualquier otra cosa (fichero corrupto o ajeno). **Test crítico.**
  - `backup_now`, `restore_latest`, `auto_backup_if_due` (comprueba token + 24h + firma).

`Db` (db.rs) gana:
- `pub fn snapshot_to(&self, path: &str) -> Result<()>` → `VACUUM INTO ?1`.
- `pub fn signature_counts(&self, source_id) -> Result<(i64,i64,i64,Option<String>)>` para
  `db_signature`.

### Comandos (`commands.rs`, registrados en `lib.rs`)

- `backup_status() -> BackupStatus { configured: bool, connected: bool, last_at: Option<String>,
  size_bytes: Option<i64> }`.
- `connect_drive(app) -> Result<(), String>` (async): lanza el flujo OAuth, guarda el refresh_token.
- `disconnect_drive()`: borra `gdrive_refresh_token`/`gdrive_file_id` de settings (no toca Drive).
- `backup_now(app) -> Result<BackupStatus, String>` (async).
- `restore_latest(app) -> Result<(), String>` (async): descarga + valida + stage + `app.restart()`.

Todos async (patrón de `open_episode` — comandos que crean ventanas/hacen I/O deben ser async, ver
memoria `project-2026-07-12-batch`).

### Restore staged (seguro en Windows)

`restore_latest`:
1. refresh token → `download_backup` → `validate_restore_bytes`.
2. Escribe `app_data_dir()/animeontrack.sqlite.restored` + `app_data_dir()/.restore_pending`.
3. `app.restart()`.

`lib.rs .setup`, **antes** de `Db::open`:
- Si existe `.restore_pending` y `animeontrack.sqlite.restored`: `fs::rename` (o copy+remove) del
  `.restored` sobre `animeontrack.sqlite`, borra ambos ficheros de control, y sigue con `Db::open`.
- Si algo falla (falta el `.restored`), borra el marcador y arranca con la DB actual (nunca deja la
  app sin abrir).

Esto evita el file-lock de la conexión abierta (no se toca el fichero mientras hay `Connection`).

### Auto-backup

En `.setup`, tras `app.manage`, `tokio::spawn` una tarea que llama `auto_backup_if_due` (silenciosa;
loguea, no molesta al usuario). Idem al final de `refresh()` en commands.rs. `auto_backup_if_due`
no hace nada si `!is_configured()` o no hay refresh_token o `now - last_at < 24h` o la firma no
cambió.

### Frontend

`src/views/Settings.tsx`: tarjeta "Copia de seguridad" (nuevo bloque, mismo design system):
- Si `!configured`: aviso "Configura las credenciales de Google (ver README)".
- Si `configured && !connected`: botón "Conectar con Google Drive".
- Si `connected`: "Última copia: {fecha} · {tamaño}", botones "Hacer copia ahora", "Restaurar
  última copia" (con confirmación: reemplaza los datos y reinicia), "Desconectar".
`src/api.ts`: wrappers `backupStatus`/`connectDrive`/`disconnectDrive`/`backupNow`/`restoreLatest`.
`src/types.ts`: `BackupStatus`. i18n: claves `settings.backup*` en `es.ts` y `en.ts`.

### Documentación

`README` (o `docs/google-drive-setup.md`): pasos para crear el OAuth Client Desktop, habilitar la
Drive API, y rellenar `.cargo/config.toml`. `.cargo/config.toml.example` versionado;
`.cargo/config.toml` en `.gitignore`.

## Criterios de aceptación (verificables)

1. `cargo test` verde con tests nuevos: `pkce_pair` (formato + challenge = SHA256/base64url del
   verifier), `build_auth_url` (contiene scope/redirect/challenge/S256), `parse_redirect_line`
   (code, error, basura), `parse_token_response`, `db_signature` (cambia al añadir episodio visto),
   y **`validate_restore_bytes`** (acepta un sqlite creado por `snapshot_to`; rechaza bytes random y
   un sqlite sin la tabla `sources`).
2. `npx tsc --noEmit` limpio y `npm run build` OK (paridad i18n es/en).
3. Sin credenciales (`option_env!` = None): la app compila, arranca, y la tarjeta muestra el aviso
   de configuración; ningún comando revienta.
4. `snapshot_to` produce un fichero que abre y pasa `integrity_check`, distinto de copiar el fichero
   vivo (consistente aunque haya journal).
5. Restore staged: con un `.restored` válido + marcador, el arranque lo aplica y abre la DB nueva;
   con un `.restored` corrupto la validación (en `restore_latest`, antes de stagear) lo impide, así
   que el marcador nunca se escribe con basura.
6. El auto-backup respeta el throttle de 24h y la firma (no sube si nada cambió) — testeable en la
   lógica pura de `auto_backup_if_due` (inyectando `last_at`/`signature`/`now`).
7. Cero acoplamiento con el scraper: `backup/` no importa `scraper_engine`; reqwest solo apunta a
   `oauth2.googleapis.com` y `www.googleapis.com`.

## Qué verificar en vivo (y qué NO)

- **Verificable con herramientas:** compilación, tests, tsc, build; `snapshot_to` + `validate` sobre
  un `.backup` de solo-lectura de la DB real; harness HTML de la tarjeta de Ajustes (oscuro/claro).
- **NO verificable automáticamente:** el flujo OAuth real (requiere el navegador y la sesión Google
  del usuario) y la subida/descarga real a Drive. El usuario debe: (a) crear el OAuth Client y poner
  las credenciales, (b) relanzar la app, pulsar "Conectar con Google Drive", completar el consent,
  (c) "Hacer copia ahora", verificar en `drive.google.com` (la carpeta appData no se ve, pero sí en
  *Configuración → Administrar aplicaciones* aparece la app con datos), y (d) probar "Restaurar".
  La ventana Tauri no es alcanzable por herramientas (memoria del proyecto).
