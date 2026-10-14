# New additions (branch `feature/new-additions`, 2026-10-14)

Source: three read-only research reports on https://github.com/snozxyx/tatakai (MPL-2.0: ideas only, nothing
copied). Claims were checked against the code before use; two were wrong (an HTML cache exists:
`html_cache.rs`; the `schema.rs` path does not exist). Rule from the owner: everything lands on this branch
only (child branches `feature/na-<slug>` merge into it with `--no-ff`); nothing goes to `develop`/`main`;
the owner reviews the UI/functionality changes himself.

## Batch 1 (robustness + exact seasons, no UI)
1. `na-frontend-tests` - frontend tests with Vitest (runner added in `chore(test)`): extract the episode
   number parser out of `SeriesDetail.tsx`, test it with `catalogLink`, `interpolate` and i18n key parity;
   drop the now-unused `parseReleasedAt.ts`.
2. `na-anilist-relations` - store AniList `relations` (PREQUEL/SEQUEL) in the catalog and use them in
   `releasing_sequel_id` instead of the title-prefix match. Fixes multi-season identity at the root.
3. `na-domain-breaker` - per-domain failure cool-down (5 min) in `scrape_via_mirrors` + do not start the
   missing-episode fallback for an episode whose air time is still in the future.

## Batch 2 (visible changes, for the owner's review)
- Keyboard shortcuts overlay, actionable empty states, continue-watching card (S effort each).
- Tag + studio scoring and a niche/popular slider in Descubrir (`recommend.rs`).
- Windows toast when a scan finds new episodes; taskbar pending badge.
- Export-diagnostics button + rotating file log.

## Batch 3 (needs decisions or live checks)
- ani.zip (absolute vs per-season numbers) to fix cross-site merge numbering: external API, needs a live
  look at rate limits first.
- One-way progress sync to AniList: needs an AniList OAuth client registered by the owner and a check
  that AniList accepts a loopback redirect.
- Tauri updater: needs a signing key pair (the private key must stay with the owner) and a `latest.json`
  step in `release.yml`.
- Real download engine (`commands/download.rs` is a stub that always fails): large, separate project.

## Validation (every batch)
Read the whole diff, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `npx tsc --noEmit`,
`npm test`; atomic commits without trailers; agents never run git.
