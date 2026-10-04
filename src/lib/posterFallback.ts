import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

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

/**
 * Resolves a stored cover URL or local file path into a displayable image source.
 * Cached covers on disk are stored in SQLite as:
 * `file:<forward-slash-path>` (e.g. `file:C:/Users/.../AppData/Roaming/com.animeontrack.app/covers/<hash>.jpg`).
 * Converts `file:` URIs and raw local filesystem paths into Tauri asset protocol URLs
 * via `convertFileSrc`, while passing through `data:`, `asset:`, and remote HTTP/HTTPS URLs.
 */
export function resolveCoverUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  if (url.startsWith("file:")) {
    let filePath = url.slice(5);
    if (filePath.startsWith("//")) {
      filePath = filePath.slice(2);
    }
    if (/^\/[a-zA-Z]:/.test(filePath)) {
      filePath = filePath.slice(1);
    }
    return convertFileSrc(filePath);
  }
  if (/^[a-zA-Z]:[\\/]/.test(url) || url.startsWith("/")) {
    return convertFileSrc(url);
  }
  return url;
}

/**
 * "This cover failed to load" state, tied to the URL that failed.
 *
 * A plain `useState(false)` flag is wrong for cards keyed by series id: the card
 * keeps its component instance when the list reloads, so a flag set by the
 * CSP-blocked scraped thumbnail would stay true after the real cover (a cached
 * `file:` path, an AniList URL) arrives and the card would show initials for
 * ever. Remembering WHICH url failed makes a new url get its own chance.
 */
export function useCoverFailure(url: string | null | undefined): [boolean, () => void] {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  return [url != null && failedUrl === url, () => setFailedUrl(url ?? null)];
}
