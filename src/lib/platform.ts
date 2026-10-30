// Single place to tell whether the app runs on a phone (Tauri Android WebView).
export const isMobile: boolean =
  typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent);
