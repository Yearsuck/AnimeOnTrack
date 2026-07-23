# Google Drive backup setup

The cloud-backup feature stores a copy of your database in a hidden per-app
folder in **your** Google Drive (`appDataFolder`, invisible in the Drive UI —
manageable under Drive → Settings → Manage apps). It needs an OAuth client you
create once, for free.

## 1. Create the OAuth client

1. Go to <https://console.cloud.google.com/>, create a project (any name).
2. **APIs & Services → Library →** enable **Google Drive API**.
3. **APIs & Services → OAuth consent screen:** choose *External*, fill the app
   name + your email, add your Google account under *Test users*. You do NOT
   need to publish/verify the app for personal use.
4. **APIs & Services → Credentials → Create credentials → OAuth client ID →**
   Application type **Desktop app**. Copy the **Client ID** and **Client secret**.

## 2. Paste the credentials into the app

Settings → **Copia de seguridad**. While no client is configured the card shows
two fields; paste the Client ID and Client secret and press **Guardar
credenciales**. No rebuild, no config file.

They're stored in the app's local SQLite database, next to the Drive refresh
token that's already kept there. A Desktop-type OAuth client's secret is not
confidential in the usual sense — Google documents it as non-secret for
installed apps, which is precisely why this flow uses PKCE — so it buys an
attacker nothing without also having your Google account.

Changing the client clears any existing Drive connection: a refresh token is
issued to one specific client ID and means nothing to another, so you'll be
asked to connect again.

### Alternative: bake them into the build

For a build that ships with its own client, copy `.cargo/config.toml.example`
to `.cargo/config.toml` and paste your values:

```toml
[env]
AOT_GOOGLE_CLIENT_ID = "1234....apps.googleusercontent.com"
AOT_GOOGLE_CLIENT_SECRET = "GOCSPX-...."
```

Then rebuild (`npm run tauri dev`). `.cargo/config.toml` is gitignored — your
secret is not committed. Anything entered in Settings takes precedence over
this.

## 3. Use it

Settings → **Copia de seguridad → Conectar con Google Drive**. A browser opens;
approve access (you'll see an "unverified app" warning — expected for a personal
Desktop client; continue). After that, backups run automatically (once a day if
something changed) and you can press **Hacer copia ahora** any time. On a new
machine, connect the same Google account and press **Restaurar última copia**.
