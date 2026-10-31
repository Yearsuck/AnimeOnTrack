import { useEffect, useRef } from "react";

/**
 * One back stack for the whole app, on top of the History API.
 *
 * Navigation is React state, so without this the WebView history is empty and
 * Android's back button (wry calls `webView.goBack()` when `canGoBack()`, else
 * finishes the activity) closes the app from any screen. Every open "layer"
 * (menu, drawer, sheet, series detail, sub-tab, non-home view) owns one history
 * entry with the same URL: system back (and desktop mouse back / Alt+Left)
 * pops it, and we close the top layer. With nothing open no entry is left, so
 * back at home exits the app as Android expects.
 *
 * History operations are reconciled, not mirrored: push/release only change
 * the layer list, and a microtask moves history to `layers.length` entries in
 * one step (pushState n times, or a single go(-n)). That makes a UI close
 * (button, Escape, click outside) consume its own entry exactly once, lets a
 * close + open in the same tick cost nothing, and never issues two traversals
 * at once. The popstate that our own go(-n) fires is recognised and ignored.
 */

export interface HistoryLike {
  readonly state: unknown;
  pushState(state: unknown, unused: string): void;
  go(delta: number): void;
}

export interface PopstateTarget {
  addEventListener(type: "popstate", listener: () => void): void;
  removeEventListener(type: "popstate", listener: () => void): void;
}

export interface BackStack {
  /** Registers an open layer; returns its release (idempotent), to call when
   *  the layer closes by any other means than the back button. */
  push(close: () => void, level?: BackLevel): () => void;
  size(): number;
  dispose(): void;
}

/** What back closes first: overlays, then sub-tabs, then the main view. A
 *  layer is stacked above every open layer of its level or lower, so a drawer
 *  restored open in the same render that opens its view still closes first
 *  (React runs a child's effects before its parent's). */
export const BackLevel = { view: 0, tab: 1, overlay: 2 } as const;
export type BackLevel = (typeof BackLevel)[keyof typeof BackLevel];

const KEY = "aotBack";
/** If our own traversal has not reported back by then, stop waiting for it. */
const TRAVERSAL_TIMEOUT_MS = 1000;

function depthOf(state: unknown): number {
  if (state && typeof state === "object" && KEY in state) {
    const d = (state as Record<string, unknown>)[KEY];
    if (typeof d === "number" && Number.isInteger(d) && d > 0) return d;
  }
  return 0;
}

export function createBackStack(
  history: HistoryLike,
  target: PopstateTarget,
  { traversalTimeoutMs = TRAVERSAL_TIMEOUT_MS }: { traversalTimeoutMs?: number } = {}
): BackStack {
  type Layer = { close: () => void; level: BackLevel };
  const layers: Layer[] = [];
  // Our entries at or below the current history entry.
  let committed = depthOf(history.state);
  let traversing = false;
  let traversalTimer: ReturnType<typeof setTimeout> | undefined;
  let scheduled = false;
  let disposed = false;

  function reconcile() {
    scheduled = false;
    if (disposed || traversing) return;
    const diff = layers.length - committed;
    if (diff > 0) {
      for (let i = 0; i < diff; i++) {
        committed++;
        history.pushState({ [KEY]: committed }, "");
      }
    } else if (diff < 0) {
      traversing = true;
      traversalTimer = setTimeout(gaveUp, traversalTimeoutMs);
      history.go(diff);
    }
  }

  function schedule() {
    if (scheduled || disposed) return;
    scheduled = true;
    queueMicrotask(reconcile);
  }

  function landed() {
    traversing = false;
    clearTimeout(traversalTimer);
    committed = depthOf(history.state);
    schedule();
  }

  // Our traversal is late (stalled main thread) or went nowhere: stop waiting
  // but issue no new traversal. history.state may still be the old entry, so a
  // second go() here would overshoot, and the late popstate, read as a user
  // back, would then close a layer nobody asked to close. When it does arrive
  // it lands exactly at layers.length and closes nothing.
  function gaveUp() {
    traversing = false;
  }

  function onPopstate() {
    if (traversing) {
      landed();
      return;
    }
    // The user went back (or forward): close every layer above where we landed.
    committed = depthOf(history.state);
    while (layers.length > committed) {
      const top = layers.pop()!;
      try {
        top.close();
      } catch (err) {
        console.error("back layer close failed", err);
      }
    }
    // Forward onto an entry no layer owns any more: step back to the real top.
    schedule();
  }

  target.addEventListener("popstate", onPopstate);
  // A reload keeps the previous page's entries: drop them.
  schedule();

  return {
    push(close, level = BackLevel.overlay) {
      const layer: Layer = { close, level };
      const above = layers.findIndex((l) => l.level > level);
      if (above < 0) layers.push(layer);
      else layers.splice(above, 0, layer);
      schedule();
      return () => {
        const i = layers.indexOf(layer);
        if (i < 0) return; // already closed by back, or released before
        layers.splice(i, 1);
        schedule();
      };
    },
    size: () => layers.length,
    dispose() {
      disposed = true;
      clearTimeout(traversalTimer);
      target.removeEventListener("popstate", onPopstate);
    },
  };
}

let shared: BackStack | null = null;

function sharedStack(): BackStack | null {
  if (typeof window === "undefined") return null;
  shared ??= createBackStack(window.history, window);
  return shared;
}

/** Starts listening at app start, so entries left by a reload are dropped even
 *  before any layer opens (the stack is otherwise created on first use). */
export function initBackStack(): void {
  sharedStack();
}

/**
 * While `open` is true the layer owns one history entry; the back button calls
 * `close` when it is the top layer. Closing it any other way (setting `open`
 * false, or unmounting) consumes the entry without touching other layers.
 * `close` may change identity every render; only `open` matters.
 */
export function useBackLayer(open: boolean, close: () => void, level: BackLevel = BackLevel.overlay): void {
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    if (!open) return;
    return sharedStack()?.push(() => closeRef.current(), level);
  }, [open, level]);
}
