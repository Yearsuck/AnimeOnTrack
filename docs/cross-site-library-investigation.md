# Cross-site library — investigation & improvement options

Status: **investigation, not implemented.** Written after a user hit the pain
directly: their primary site (AnimeYT) had a server outage, they switched to
TioAnime, and their library looked almost empty.

## The complaint, and what's actually true

Switching sites makes the library look like it lost data. **It didn't** —
verified against the live database:

| Site | Followed | Watched-externally | Want |
|---|---|---|---|
| AnimeYT | **133** | 612 | 199 |
| TioAnime (active) | 6 | 0 | 0 |
| AnimeFLV | 1 | 0 | 0 |

Everything is preserved per site. Switching back to AnimeYT restores it all.
The felt problem is real though: **a site outage strands your whole library**,
because the library is keyed to the site.

## How it works today

- **Library is per-site.** Each `series` row belongs to a `source_id`. Switching
  the active site (`set_active_site` → `switch_site_core`) only flips the
  `active_site_id` setting and re-scans; it never deletes another site's rows.
- **Carry-over on scan** (`scan.rs` → `plan_carryover`): when the new site's
  airing list is scanned, each scanned series is matched against your
  follows-on-other-sites (`followed_titles_with_watermark`) by **fuzzy title**
  (`matching::best_match`, above `MATCH_THRESHOLD`). A match inherits the follow
  plus a "seen up to N" watermark (`carry_follow` / `carried_seen_number`),
  applied once when that series' episodes are next fetched.

## Why so little carried over (the gap, with numbers)

Two hard limits, both visible in the data:

1. **Carry-over only sees the new site's airing listing** (~one page, currently
   airing shows). Of the 133 AnimeYT follows: **32 are airing, 101 are
   finished.** The 101 finished follows are *not on any airing page*, so they
   can **never** carry over this way. Only the airing overlap the new site also
   lists carries — here, 6.
2. **Matching is fuzzy title**, not the canonical id — even though **102 of 133**
   follows already carry an `anilist_id` (a reliable cross-site key). Fuzzy
   title is both less accurate and the reason carry-over stays conservative.

Root cause: **library identity is a per-site `series` row**, and carry-over is
deliberately limited to the cheap airing page to avoid mass-scraping the new
site (every finished title would need its own search scrape → Cloudflare abuse).

## Options

### A — Match cross-site by `anilist_id` (small, foundational)
`followed_titles_with_watermark` + `plan_carryover` match on `anilist_id` when
both rows have one, falling back to fuzzy title. Cheap, strictly more accurate,
and the basis every other option benefits from (102/133 follows already qualify).
Doesn't fix the finished-shows gap by itself.

### B — "Bring my library to this site" (medium; the direct fix)
A user-initiated action (button on site switch, or in Settings): for every
follow-elsewhere not already matched by the airing carry-over, **search the new
site** via the adapter's search, match the result (by `anilist_id`/title), and
carry the follow + watermark. Paced (~1 request / 2 s, like the catalog sync —
Cloudflare-safe), with a progress bar and resumable state. ~127 searches ≈ 4–5
minutes. Only resolves titles the site actually hosts; misses are reported, not
guessed. This is what makes a switched-to site usable during an outage.

### C — Site-agnostic library keyed on a canonical id (large; the north star)
Re-key the **library** (follows, seen progress, backlog) to a canonical
identity — `anilist_id`, with a normalized-title fallback for the ~23% without
one — instead of a per-site `series` row. A per-site `series` row becomes just a
*scraping target* for a canonical title, resolved on demand and cached. Then:
a site outage never strands the library; switching sites is instant; "what I
follow / what I've watched" is genuinely yours, not the site's. Requires a
schema/model change + migration and on-demand site resolution when you actually
open an episode. Biggest change, but the only one that removes the root cause.

### D — Merged library view (UX face of C)
Show **all** your follows regardless of the active site; the active site only
decides *where episodes are fetched from*. Identity merged by `anilist_id`/title.
Same end-user result as C with a lighter first step (a merged read model over the
existing per-site rows), deferring the full re-key.

## Tradeoffs

| Option | Effort | Fixes finished-shows gap | Scraping cost | Risk |
|---|---|---|---|---|
| A · id-based match | low | no (accuracy only) | none | low |
| B · full import on demand | medium | yes | N searches, paced | Cloudflare pacing, per-site coverage |
| C · site-agnostic library | high | yes | on-demand only | schema migration, biggest surface |
| D · merged view | medium-high | yes (display) | on-demand when watching | reconciling identity across sites |

## Recommendation

Incremental, each step shippable and useful on its own:

1. **A now** — cheap reliability win and the matching foundation for everything
   else (match on `anilist_id`, fuzzy title fallback).
2. **B next** — delivers the felt value: a paced, user-initiated "bring my whole
   library to this site" so a switched-to site isn't empty. This is the highest
   value-for-effort and directly answers the outage scenario.
3. **C/D later** — if site outages keep hurting, move to a site-agnostic library
   (library = yours, site = backend). The `anilist_id` linking added recently
   (102/133 follows) already lays the groundwork.

Non-negotiable constraint throughout: any new site scraping must be **paced and
user-initiated** (never a silent bulk crawl), exactly like the catalog sync, or
Cloudflare will rate-limit/ban — the same discipline the rest of the scraper
already follows.
