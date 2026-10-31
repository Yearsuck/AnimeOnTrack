/** Pure helper to test whether a user agent represents a mobile platform. */
export function detectMobile(userAgent?: string): boolean {
  if (userAgent != null) {
    return /Android/i.test(userAgent);
  }
  return typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
}

// Single place to tell whether the app runs on a phone (Tauri Android WebView).
export const isMobile: boolean = detectMobile();
