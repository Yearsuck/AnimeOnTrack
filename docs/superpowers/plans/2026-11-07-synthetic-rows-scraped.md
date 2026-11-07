# Unlinked catalog rows scraped as if they were on the site

Date: 2026-11-07. Status: implemented in the PR that carries this file.

## Problem
A followed catalog row (synthetic slug `anilist-<id>`, url `https://anilist.co/anime/<id>`) has no page on any
site. `list_followed` returned it to `refresh()`, the episode backfill and the genre backfill, which take the
url's path (`/anime/<id>`) to the active site. Since the animeyt.cc relaunch that path answers with a feed of
other shows' recent episodes, which `new_episodes` stored as the row's episodes while `apply_episode_diff`
rewrote its url. Evidence (copy of the user's DB, 2026-11-07): 15 followed `anilist-%` rows on animeytx with
unseen foreign episodes (Goblin id 1060, Magic Repo Man id 3931, Black Clover S2, Frieren 3...), 510 such
episodes in total (389 seen, 121 unseen), 25 rewritten urls.

## Changes (3 commits on `fix/synthetic-rows-not-scraped`)
1. `fix(pending)`: `merged_pending_episodes_at` ignores the episodes of synthetic rows (they still count for the
   followed / watched-externally signals of their group).
2. `fix(refresh)`: `list_followed` and `followed_series_without_episodes` skip `anilist-%` rows.
3. `fix(db)`: `repair_scraped_synthetic_rows`, run once from `Db::open`: backup via `VACUUM INTO`
   (`<db>.before-synthetic-repair`), delete the episodes of `anilist-%` rows, restore their anilist.co url.

## Not covered
Why only some of the 4627 synthetic rows on animeytx were followed and hence scraped (carry_follow of watched
shows) was not traced. A real site row whose slug is rewritten by linking is unaffected by all three.
