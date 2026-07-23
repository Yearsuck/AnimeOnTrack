# Descubrir: catalog-only swipe deck

Follow-up to `2026-07-10-full-anilist-catalog-sync-design.md`. With the full ~22,400-title AniList catalog synced locally, the `Del sitio | Catálogo completo` source toggle in `src/views/Descubrir.tsx` is redundant: the catalog is a strict superset of what the scraped site offers, and every catalog card is served from SQLite (instant, zero Cloudflare exposure) while site cards require a real scrape per deck refill.

## Problem

1. The toggle exists and forces two incompatible code paths (`discover_swipe_card`/`decide_swipe` vs `discover_catalog_card`/`decide_catalog_card`), including a disabled "Ya lo vi" button on the catalog side.
2. **Genre affinity is silently broken across sources.** Verified against the live DB: `series_genres` holds Spanish genres for scraped rows (`Fantasía` 97, `Acción` 88, `Aventura` 59…) and English genres for catalog-added rows (AniList's closed vocabulary: `Action`, `Fantasy`, `Adventure`…). `get_genre_affinity` groups by raw string, so `Acción` and `Action` score as unrelated genres. The user's taste signal is split in half.
3. **The catalog deck has no quality floor.** `db.rs::random_catalog_anime` is `ORDER BY RANDOM() LIMIT 1` over all 22,419 rows — which includes 1,652 `Hentai` entries, plus thousands of shorts/ONAs/specials with no popularity. It also **does not exclude already-decided titles** (`decide_catalog_card` writes a `series` row with slug `anilist-{id}`, but the picker never checks), so decided cards can reappear. The genre-affinity weighting (`get_genre_affinity` + `weighted_pick_index`) is only wired into the *site* path today; the catalog path is uniform-random.

## Technical context

- `commands.rs`: `discover_swipe_card` (scrape path, genre-weighted via `weighted_pick_index` over `get_genre_affinity`), `discover_catalog_card` (uniform random over local catalog), `decide_swipe`, `decide_catalog_card` (writes `series` row slug `anilist-{id}`, sets `last_swiped_series_id` for `undo_last_swipe`).
- `AppState.swipe_buffer` / `swipe_last_page` / `swipe_served` exist only to amortize scrape fetches for the site path.
- `swipe.rs`: `weighted_pick_index` (uniform fallback when all weights ≤ 0), `pick_index`, `shuffle`, `undecided_cards`.
- `db.rs`: `get_genre_affinity(source_id) -> HashMap<String, f64>` (+2 followed, +1 want, −1.5 discarded), `random_catalog_anime`, `list_catalog_genres`.
- `Descubrir.tsx`: `SwipeSource` state, a 10-card prefetch queue, `anilistIdFromUrl`, keyboard bindings (↑ = Seen, disabled for catalog).

## Design

### 1. Genre vocabulary normalization (fixes the split taste signal)

AniList's genre list is **closed and fixed** (19 values: Action, Adventure, Comedy, Drama, Ecchi, Fantasy, Hentai, Horror, Mahou Shoujo, Mecha, Music, Mystery, Psychological, Romance, Sci-Fi, Slice of Life, Sports, Supernatural, Thriller). Adopt it as the canonical vocabulary.

New `src-tauri/src/genres.rs`: `pub fn canonical_genre(raw: &str) -> Option<&'static str>` — a static, case/accent-insensitive lookup mapping the site's Spanish terms onto AniList's canon (`Acción`→`Action`, `Fantasía`→`Fantasy`, `Aventura`→`Adventure`, `Comedia`→`Comedy`, `Ciencia ficción`→`Sci-Fi`, `Recuentos de la vida`→`Slice of Life`, `Misterio`→`Mystery`, `Terror`→`Horror`, `Deportes`→`Sports`, `Música`→`Music`, `Psicológico`→`Psychological`, plus already-canonical passthrough for `Drama`/`Romance`/`Supernatural`/`Mecha`/`Ecchi`/`Horror`/`Thriller`). Site-only tags with no AniList equivalent (`Shounen`, `Isekai`, `Reencarnación`, `Escolar`, `Seinen`, `Josei`…) return `None`.

`get_genre_affinity` maps each raw genre through `canonical_genre` and folds scores into the canonical key; unmapped genres are **kept under their raw name** (they still carry real signal for the site path's genre pages — dropping them would lose the `Isekai`/`Shounen` preference entirely). The catalog picker only ever looks up canonical keys, so unmapped keys simply never match and cost nothing.

Do **not** rewrite existing `series_genres` rows — normalization happens at read time. Storage stays a faithful record of what each source said.

### 2. Weighted, quality-floored catalog picker

Replace `random_catalog_anime` with `db.rs::random_catalog_anime_weighted(source_id, exclude_genres: &[&str]) -> Result<Option<CatalogAnime>>`, and have the **command** do the genre pick (mirroring how the site path works — DB stays dumb, `swipe.rs` owns the randomness):

- `commands.rs::discover_catalog_card`: read `get_genre_affinity(source_id)`; build the candidate genre list from `get_catalog_facets`-style `SELECT DISTINCT genre` **minus the excluded set**; weight each by its affinity score; pick one via `weighted_pick_index` (uniform fallback preserved for cold start); then ask the DB for a random catalog row **in that genre** matching the quality floor and not already decided.
- **Excluded genres**: `Hentai` and `Ecchi` never enter the deck (1,652 Hentai rows would otherwise dominate ~7% of a uniform deck). Hardcoded const, not a setting.
- **Quality floor** (SQL `WHERE`): `format IN ('TV','MOVIE','OVA','ONA','SPECIAL')` (drops `MUSIC`, `TV_SHORT`, manga-side formats) **and** `popularity >= 500` (the sync now stores popularity — this cuts the long tail of essentially-unknown entries while leaving tens of thousands of real titles; tune the constant if the deck feels too mainstream, but keep it explicit).
- **Exclude decided**: `id NOT IN (SELECT CAST(SUBSTR(slug, 9) AS INTEGER) FROM series WHERE slug LIKE 'anilist-%')`. Fixes the reappearing-card bug. (The synthetic-slug parse is ugly; alternative is a real `anilist_id` column on `series` — implementer's call, but if you add the column, add it via `ensure_column` and backfill from the slug, and use it here and in `decide_catalog_card`.)
- If the weighted genre yields no undecided candidate, retry with the next-best genre (bounded, e.g. 5 attempts) before returning `None` (deck exhausted).
- `matched_genre` on the returned `FinishedCard` is the genre that was picked (so the UI's "Género: X" line stays meaningful).

### 3. "Ya lo vi" for catalog cards

The toggle's removal makes the disabled ↑ button unacceptable — mark-as-seen is a core swipe action. AniList has no episode list, so "Seen" cannot mean "mark all episodes watched". Define it as: **`decide_catalog_card` with a new `seen: bool`** → writes the same `series` row but with `backlog_status = NULL` and `followed = 0`, plus a new `series.watched_externally = 1` flag (`ensure_column`, INTEGER DEFAULT 0). Semantics: "I've watched this, don't show it to me again, don't put it in my backlog." It's excluded from the deck by the existing not-already-decided filter, appears nowhere else in the UI for now.

This keeps the three-way swipe intact and is honest about the data we have. Episode-level tracking for a catalog title only becomes possible once it's linked to a real site URL — which is task 4's job, not this one.

### 4. Frontend (`Descubrir.tsx`)

- Delete `SwipeSource`, `sourceRef`, the `Del sitio | Catálogo completo` `.tabs` block, and the source-switch `useEffect`.
- `fillQueue` always calls `discoverCatalogCard`.
- `decide()` always calls `decideCatalogCard` (now with `seen`), keeping `anilistIdFromUrl`.
- ↑ / "Ya lo vi" enabled unconditionally; drop the `source === "catalog"` guards and the "No disponible" tooltip.
- `TasteChips` unchanged (`getTopGenres` reads the now-normalized affinity).

`discover_swipe_card` / `decide_swipe` and the `swipe_buffer`/`swipe_last_page`/`swipe_served` state stay in `commands.rs` for now — they're dead from the UI's perspective but are the natural substrate for task 4's site search. Do not delete them; do not leave them wired to Descubrir.

## Acceptance criteria (verifiable)

1. `cargo test` passes with new tests: `canonical_genre` (accent/case-insensitive, `Acción`→`Action`, `Shounen`→`None`); `get_genre_affinity` folds `Acción` and `Action` into one `Action` score and preserves `Isekai` under its raw name; catalog picker excludes Hentai/Ecchi, respects the format+popularity floor, excludes titles with an `anilist-{id}` series row, and returns `None` only when genuinely exhausted; `decide_catalog_card(seen: true)` sets `watched_externally=1`, `followed=0`, `backlog_status=NULL`.
2. Deck never serves a decided card twice in a session (test + live).
3. `npx tsc --noEmit`, `npm run build` pass.
4. Live: Descubrir shows no source toggle; ~20 consecutive swipes serve only recognizable titles (no Hentai, no obscure `MUSIC` entries), each with a genre line; ↑ works; Ctrl+Z undoes; decided titles don't reappear after a full app restart (check `SELECT slug, followed, backlog_status, watched_externally FROM series WHERE slug LIKE 'anilist-%'` in `%APPDATA%\com.ernes.aot-scaffold\animeontrack.sqlite`).

## Live verification required

- Screenshot of Descubrir with the toggle gone and a real card rendered.
- SQL dump of the `anilist-%` series rows after a handful of swipes (one of each: discard / want / seen) showing the three distinct states.

## Explicitly out of scope

- Linking catalog cards to real site URLs / episodes (task 4).
- Removing `discover_swipe_card`/`decide_swipe` (task 4 reuses the scrape substrate).
- Surfacing `watched_externally` titles anywhere in the UI (Library redesign, task 9, may pick them up).
- Making the excluded-genre set or popularity floor user-configurable.
