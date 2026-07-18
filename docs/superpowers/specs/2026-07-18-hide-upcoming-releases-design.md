# Hide upcoming (not-yet-released) titles in the Descubrir swipe deck

## Goal

Add a toggle to the Descubrir swipe deck's existing bans panel (genre/format bans) that excludes not-yet-released titles from the catalog deck — you can't watch something with no episodes yet, so it shouldn't come up to decide on.

## Current state

- `discover_catalog_card` (`src-tauri/src/commands/discover.rs`) picks a taste-weighted genre, then calls `Db::random_catalog_anime_in_genre` (`src-tauri/src/db/catalog.rs`) for a candidate, excluding banned genres/formats.
- `get_deck_bans`/`set_deck_bans` round-trip a `DeckBans { genres, formats }` struct, backed by `db.get_setting`/`set_setting`. The frontend's `DeckPanel.tsx` loads/saves this on open/save.
- `anilist_catalog` (SQLite) does **not** store AniList's `status` field (`RELEASING`/`NOT_YET_RELEASED`/`FINISHED`/...). The GraphQL sync only uses `status` transiently, as a partition query variable — it's discarded after fetch, never persisted per-row.

## Change

### 1. Schema + sync (persist `status`)

- `db.rs` (`init_schema`): `ensure_column(&self.conn, "anilist_catalog", "status", "TEXT")?;` — nullable, additive migration (existing behavior for every prior column addition in this table).
- `anilist.rs`: add `status` to the GraphQL query's `media { ... }` selection set. `MediaEntry` gains `status: Option<String>`. `CatalogAnime` gains `pub status: Option<String>`.
- `db/catalog.rs`: `upsert_catalog_anime` writes `status` on insert/update (same `ON CONFLICT DO UPDATE` pattern as `format`/`episodes`/etc.). `row_to_catalog_anime` reads it back.
- **Existing rows stay `NULL` until the next sync** (full or incremental, user-triggered from the Catálogo tab). This is expected, not an error — the deck-exclusion query treats `NULL` as "don't hide" so nothing silently vanishes from the deck before a resync.

### 2. Deck bans (persist the toggle)

- `DeckBans` struct (`commands/discover.rs`) gains `pub hide_upcoming: bool`.
- `get_deck_bans` reads a new settings key `hide_upcoming_releases` (`"true"`/absent → `false`), alongside the existing banned-genres/-formats reads.
- `set_deck_bans` command signature gains a `hide_upcoming: bool` param, writes it via `db.set_setting("hide_upcoming_releases", ...)`.

### 3. Deck exclusion (apply the toggle)

- `Db::random_catalog_anime_in_genre` (`db/catalog.rs`) gains one new parameter: `hide_upcoming: bool`. When `true`, its SQL `WHERE` gains `AND (status IS NULL OR status != 'NOT_YET_RELEASED')`.
- `discover_catalog_card` reads the setting (same place it already reads banned genres/formats) and passes it through.

### 4. Frontend

- `src/types.ts`: `DeckBans` interface gains `hideUpcoming: boolean`.
- `src/api.ts`: `getDeckBans`/`setDeckBans` thread the new field through (`setDeckBans(genres, formats, hideUpcoming)`).
- `src/views/Descubrir/DeckPanel.tsx`: one more checkbox row, same visual pattern as the existing genre/format ban checkboxes, wired to load/save with the rest of the panel state.
- i18n: new key in `src/i18n/catalog/{es,en}.ts` for the checkbox label (Spanish: "Ocultar próximos estrenos").

## Testing

New Rust unit tests in `db/catalog.rs`:
- `random_catalog_anime_in_genre_excludes_not_yet_released_when_hide_upcoming_true`
- `random_catalog_anime_in_genre_includes_null_status_when_hide_upcoming_true` (unsynced rows aren't hidden)
- `random_catalog_anime_in_genre_includes_upcoming_when_hide_upcoming_false` (default/off behavior unchanged)

New test in `commands/discover.rs` or wherever `DeckBans` round-trips: confirm `set_deck_bans`/`get_deck_bans` carries `hide_upcoming` correctly (extend/adjacent to any existing bans round-trip test).

No test coverage exists for `DeckPanel.tsx` (consistent with the rest of Descubrir's frontend) — manual check after implementation: open the bans panel, toggle the new checkbox, save, confirm it persists across a panel close/reopen and (after a resync) actually removes unreleased titles from the deck.

## Out of scope

- No change to the Catálogo tab's `CatalogFilter`/`list_catalog_filtered` (separate view, separate filter model, not mentioned by the request).
- No automatic resync — the user triggers it manually from the existing Catálogo sync button when ready.
- No UI indicator elsewhere (Library, Pending, etc.) — this is purely a Descubrir deck exclusion.
