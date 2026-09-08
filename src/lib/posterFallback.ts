// Up to two initials from a title, for the poster-less fallback block —
// shared by every grid that can show a series with no usable cover_url
// (null, or a remote URL the app's CSP blocks — see AiringGrid/Library's
// showFallback usage).
export function initials(title: string): string {
  const chars = title
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");
  return chars || "?";
}
