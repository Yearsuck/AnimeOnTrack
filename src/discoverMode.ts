// Minimal, dependency-free persisted-choice layer for the Descubrir deck
// mode, mirroring src/theme.ts's / src/i18n's localStorage pattern (see
// docs/superpowers/specs/2026-07-13-discover-recommendation-toggle-design.md).
//
// Unlike theme (which stamps a DOM attribute other CSS depends on), this has
// no side effect to apply on load — it's read directly by Descubrir.tsx's
// fillQueue before each discover_catalog_card call. No context/provider
// needed; a tiny custom-event bridge is enough to make `useDiscoverMode`
// re-render every subscriber when one of them calls the setter (same shape
// as useLang/useTheme's re-render-on-change contract, without requiring every
// caller to sit under a new Provider).
import { useCallback, useEffect, useState } from "react";

export type DiscoverMode = "recommended" | "random";

export const DISCOVER_MODES: { code: DiscoverMode; labelKey: "discover.modeRecommended" | "discover.modeRandom" }[] = [
  { code: "recommended", labelKey: "discover.modeRecommended" },
  { code: "random", labelKey: "discover.modeRandom" },
];

const STORAGE_KEY = "aot.discoverMode";
const CHANGE_EVENT = "aot:discoverModeChange";

function isDiscoverMode(value: unknown): value is DiscoverMode {
  return value === "recommended" || value === "random";
}

// Default "recommended" — the engine is the point; a first-time user should
// see it working, not a plain-random deck.
export function getDiscoverMode(): DiscoverMode {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isDiscoverMode(stored)) return stored;
  } catch {
    // localStorage unavailable (privacy mode, etc.) — fall through to default.
  }
  return "recommended";
}

export function setDiscoverMode(next: DiscoverMode): void {
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    // localStorage unavailable — the choice just won't persist.
  }
  window.dispatchEvent(new CustomEvent<DiscoverMode>(CHANGE_EVENT, { detail: next }));
}

/// Returns `[mode, setMode]`, re-rendering the caller whenever any caller
/// (including a different component instance) sets a new mode — same shape
/// as `useLang`/`useTheme`.
export function useDiscoverMode(): [DiscoverMode, (next: DiscoverMode) => void] {
  const [mode, setModeState] = useState<DiscoverMode>(getInitialFromCurrent);

  useEffect(() => {
    function onChange(e: Event) {
      const detail = (e as CustomEvent<DiscoverMode>).detail;
      if (isDiscoverMode(detail)) setModeState(detail);
    }
    window.addEventListener(CHANGE_EVENT, onChange);
    return () => window.removeEventListener(CHANGE_EVENT, onChange);
  }, []);

  const setMode = useCallback((next: DiscoverMode) => {
    setDiscoverMode(next);
  }, []);

  return [mode, setMode];
}

function getInitialFromCurrent(): DiscoverMode {
  return getDiscoverMode();
}
