// Native Windows toast notification support via @tauri-apps/plugin-notification.
// Used when a background refresh detects newly available episodes for followed series.

export const NOTIFY_KEY = "aot.notifyNewEpisodes";

export function shouldNotify({
  newCount,
  enabled,
  windowFocused,
}: {
  newCount: number;
  enabled: boolean;
  windowFocused: boolean;
}): boolean {
  return newCount > 0 && enabled && !windowFocused;
}

export function parseNotifyPref(raw: string | null): boolean {
  if (raw === "0" || raw === "false") {
    return false;
  }
  return true;
}

export function readNotifyPref(): boolean {
  try {
    return parseNotifyPref(localStorage.getItem(NOTIFY_KEY));
  } catch {
    return true;
  }
}

export function writeNotifyPref(v: boolean): void {
  try {
    localStorage.setItem(NOTIFY_KEY, v ? "1" : "0");
  } catch {
    // localStorage unavailable — ignore failure.
  }
}

export async function notifyNewEpisodes(
  count: number,
  title: string,
  body: string
): Promise<void> {
  if (count <= 0) return;
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    let granted = await isPermissionGranted();
    if (!granted) {
      const permission = await requestPermission();
      granted = permission === "granted";
    }
    if (granted) {
      await Promise.resolve(sendNotification({ title, body }));
    }
  } catch {
    // Swallowed: a missing notification must never break a refresh.
  }
}
