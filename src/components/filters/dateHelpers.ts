// "YYYY-MM-DD" <-> Unix seconds, both as UTC midnight of that calendar date
// -- matches src-tauri/src/anilist.rs's FuzzyDate::to_timestamp(), which
// stores AniList start_date as UTC midnight, not local time.
export function dateInputToEpoch(iso: string): number {
  const [y, m, d] = iso.split("-").map(Number);
  return Math.floor(Date.UTC(y, m - 1, d) / 1000);
}

export function epochToDateInput(epoch: number): string {
  return new Date(epoch * 1000).toISOString().slice(0, 10);
}
