# Episode merge across sites + season relink (2026-10-13)

Builds on `docs/anilist-source-of-truth.md` and the site-priority setting (`site_priority`).
Owner's rule: sites only provide episode links; the user only orders the sites; when an anime or an
episode is missing on one site the next one in that order is used.

## 1. Merged pending (read side)  — slug `merged-pending`
Today `Db::list_pending`/`pending_count` pick ONE "winner" series row per canonical show
(`get_fallback_series_ids`: the active site if it has episodes, else the site with most episodes).
New: for each followed canonical show (`library::canon_key`, i.e. `anilist_id` else normalized title)
union the unseen episodes of ALL its member rows. For each episode number take the link from the
member whose site ranks first in `site_priority` (a member that lacks the number is skipped).
- Seen state: gap-free means seen episodes are `<= W`, W = max seen number over the members. An
  episode is pending when its number is `> W` and `watched_externally = 0`.
- The pending item's `Series` is the group representative (the highest-priority member that has any
  pending episode), so the UI groups one show into one block; the `Episode` keeps its own row id/url.
- Marking seen/unseen must hit every member of the group: `set_seen_cascade` command becomes canonical
  (new `Db::set_seen_cascade_canonical`), otherwise the other rows keep the episode pending.
- Known limitation (pre-existing in `sync_seen_progress_across_sites`): numbers are compared across
  sites as plain numbers; a site that numbers cumulatively (TioAnime "27") next to one that restarts per
  season ("3") can disagree. Not solved here.

## 2. Fetch what is missing — slug `missing-episode-fetch`
Today `run_episode_fallback` only runs for shows with NO episodes on any site. New candidates: followed,
airing (catalog `RELEASING`) shows whose highest known episode number over all member rows is below what
AniList says has aired (`next_episode - 1`). For those: (a) refetch the episode list of the member rows
on other sites (they have a stored `url`; `refresh()` only refreshes the active site), in priority
order, (b) then search the remaining sites in priority order as the fallback already does. Misses are
remembered per (show, site) with the existing marker so a show that is simply not out yet is not
re-searched every minute.

## 3. Season relink — slug `season-relink`
`sync_status_from_catalog` keeps a series airing when it was listed in the last 3 days and the catalog
has a RELEASING/HIATUS sequel (commit "keep a listed series airing when a later season is releasing").
That leaves the row linked to the FINISHED first season (wrong title/cover/next episode, and it does not
unify with other sites' rows linked to season 3). New: relink such a row (`anilist_id`) to the releasing
sequel, when exactly one candidate exists (or the one with the latest `start_date`) and its
`start_date` is later than the current entry's. The row stays airing because the new entry is RELEASING.

## Verification
Each slice: unit tests + `cargo clippy -D warnings` + `tsc`. Then sandbox on a copy of the real DB
(`tools/sandbox`): pending list for Tensei Kizoku, a show present on two sites with one site one episode
behind, and site priority changes reordering the link chosen.
