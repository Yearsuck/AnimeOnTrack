import type { MessageKey } from "../i18n";

export type TutorialIllustration = "welcome" | "airing" | "pending" | "library" | "catalog" | "settings";

export interface TutorialStep {
  id: string;
  /** Top-tab to ring (matches the `data-tour` attribute on the tab), or null for a centered card. */
  target: string | null;
  illustration: TutorialIllustration;
  titleKey: MessageKey;
  bodyKey: MessageKey;
}

export const TUTORIAL_STEPS: readonly TutorialStep[] = [
  { id: "welcome", target: null, illustration: "welcome", titleKey: "tutorial.welcome.title", bodyKey: "tutorial.welcome.body" },
  { id: "airing", target: "airing", illustration: "airing", titleKey: "tutorial.airing.title", bodyKey: "tutorial.airing.body" },
  { id: "pending", target: "pending", illustration: "pending", titleKey: "tutorial.pending.title", bodyKey: "tutorial.pending.body" },
  { id: "library", target: "library", illustration: "library", titleKey: "tutorial.library.title", bodyKey: "tutorial.library.body" },
  { id: "catalog", target: "catalog", illustration: "catalog", titleKey: "tutorial.catalog.title", bodyKey: "tutorial.catalog.body" },
  { id: "settings", target: "settings", illustration: "settings", titleKey: "tutorial.settings.title", bodyKey: "tutorial.settings.body" },
];

export const TUTORIAL_SEEN_KEY = "aot.tutorial.seen";

/** Minimal Storage surface so tests can pass a fake (or a throwing one). */
export interface KeyValueStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function defaultStore(): KeyValueStore | null {
  try {
    return localStorage;
  } catch {
    return null;
  }
}

export function readTutorialSeen(store: KeyValueStore | null = defaultStore()): boolean {
  try {
    return store?.getItem(TUTORIAL_SEEN_KEY) === "1";
  } catch {
    return false;
  }
}

export function writeTutorialSeen(store: KeyValueStore | null = defaultStore()): void {
  try {
    store?.setItem(TUTORIAL_SEEN_KEY, "1");
  } catch {
    /* storage unavailable: the tutorial just may show again next first-run */
  }
}

/**
 * Auto-show rule. A brand-new install does not go through Onboarding (a
 * default source exists), so "fresh profile" is detected from data: nothing
 * followed, nothing pending, nothing in the library. Show once when unseen and
 * (fresh profile or just finished Onboarding). Existing users (any data,
 * marker unset) must never see it: the caller silently marks them as seen.
 */
export function shouldAutoShowTutorial(opts: {
  justCompletedOnboarding: boolean;
  seen: boolean;
  hasUserData: boolean;
}): boolean {
  if (opts.seen) return false;
  return opts.justCompletedOnboarding || !opts.hasUserData;
}

/** Wait this long after launch so the tutorial never opens over the splash. */
export const TUTORIAL_AFTER_SPLASH_MS = 1700;

export function clampStep(index: number): number {
  return Math.min(Math.max(0, index), TUTORIAL_STEPS.length - 1);
}
