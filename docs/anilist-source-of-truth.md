# AniList is the source of truth

Decision (2026-10-03): every fact about an anime that is not "where do I watch episode N" comes
from AniList. The streaming sites only supply **episode links** (and, for shows AniList cannot be
matched to, a fallback title/cover so the row is still usable).

This refines `project-anilist-agnostic-model` (v0.4.0): that release made lists canonical per
`anilist_id`; this one moves the *metrics, tags and status* onto the same footing.

## Field ownership

| Fact | Source of truth | Fallback when the row is not linked | Status |
|---|---|---|---|
| Identity | `series.anilist_id` | normalized title (`canon_key`) | linking: season-aware, synonyms (done), AniList search (this plan) |
| Title, cover | `anilist_catalog` | site title/cover | done (list_airing / library) |
| Airing status (`is_airing`) | `anilist_catalog.status` | site listing | done, both directions |
| Next episode date / number | AniList `nextAiringEpisode` | site `next_episode_at` | **new**: `next_airing_at`, `next_episode` in the catalog |
| Total episodes | `anilist_catalog.episodes`, else aired count from `next_episode - 1` | count of site episode rows | **new** (stats denominator) |
| Genres | `anilist_catalog_genres` | `series_genres` (site/discover) | stats still read `series_genres` |
| Tags (themes, setting) | AniList `tags` (name, rank) | none | **new** table `anilist_catalog_tags` |
| Format / kind | `anilist_catalog.format` | site `kind` | stats read site `kind` |
| Duration, studio, start date, score, popularity | `anilist_catalog` | defaults | present, partly unused |
| Episode urls, numbers, release time | streaming site | - | stays site-owned |
| Seen / followed / backlog state | local user data | - | local, never AniList |

## Rules

1. Read metadata through one place. A series' *effective* metadata is the catalog row when
   `anilist_id` is set, otherwise the site values. Consumers (stats, tags, filters, status) must not
   each re-implement that choice.
2. A wrong link is worse than no link (see `season_consistent`): a series whose link is doubtful
   keeps the site fallback instead of inheriting another season's facts.
3. Franchise roll-ups key on `anilist_id` when present and fall back to the title heuristic only for
   unlinked rows, so cross-site duplicates and split arcs stop being a title-matching problem.
4. AniList access is rate limited (~28.6 req/min, 429 retry): every new fetch is paced, resumable and
   never holds the DB mutex across a request. Synchronous commands run on the UI thread, keep them short.
5. The unlinked remainder must stay visible and shrinking: the airing view and Settings should be able
   to tell how many rows still rely on the fallback.

## Delivery plan (one PR each, gitflow)

1. **Linking completeness**: AniList search for rows still unlinked (<= 30 req/min, high confidence,
   misses remembered). Without this the rest has holes.
2. **Catalog fields**: `nextAiringEpisode` and `tags` fetched and stored (`CATALOG_METADATA_VERSION` 3).
3. **Effective-metadata layer + readers**: one helper for "catalog-else-site", then switch `is_airing`
   countdown, genres, format and episode totals in stats / filters / airing to it.
4. **Stats corrections** on top: "Completado %" denominator and average episodes per series via the
   canonical total, "Dias para terminar" ignoring 0-day spans.
5. **New metrics**: time to clear the backlog, top studios, release era, series behind the binge record.
6. **UI accessibility** of the stats charts (lists, SVG titles, empty states).
