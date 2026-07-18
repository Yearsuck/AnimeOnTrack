import { useSyncExternalStore } from "react";

// Shared "chart shape" preference for the whole Estadísticas screen — Barras
// (horizontal bar chart) <-> Círculos (donut ring grid). Persisted the same
// way as language/theme (see src/i18n, src/theme.ts).
//
// This is a tiny module-level store (not just a `useState` per component)
// because more than one component on the Estadísticas screen needs to react
// to the same choice live: StatsRings.tsx (géneros/tipos) and
// StatsInsights.tsx (embudo, en emisión vs finalizadas) render at the same
// time but aren't parent/child, and StatsRings can mount/unmount as the
// user switches the Grafo/Barras tab while StatsInsights stays mounted. A
// plain per-component `useState(getInitialShape)` would only read
// localStorage once at that component's own mount time, so toggling the
// shape in one block wouldn't repaint the other until it happened to
// remount. Subscribing to one shared store keeps every mounted consumer in
// sync immediately, no prop drilling required.
export type Shape = "bars" | "rings";

const SHAPE_KEY = "aot.statsShape";

function readInitial(): Shape {
  try {
    const s = localStorage.getItem(SHAPE_KEY);
    if (s === "bars" || s === "rings") return s;
  } catch {
    // localStorage unavailable — fall through to default.
  }
  return "bars";
}

let currentShape: Shape = readInitial();
const listeners = new Set<() => void>();

function setShape(next: Shape) {
  if (next === currentShape) return;
  currentShape = next;
  try {
    localStorage.setItem(SHAPE_KEY, next);
  } catch {
    // choice just won't persist
  }
  listeners.forEach((l) => l());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): Shape {
  return currentShape;
}

export function useStatsShape(): [Shape, (next: Shape) => void] {
  const shape = useSyncExternalStore(subscribe, getSnapshot);
  return [shape, setShape];
}
