// Poster -> series detail shared-element morph, built on the View Transitions
// API (Chromium / WebView2). The clicked card's poster and the detail's cover
// share one `view-transition-name` only while a transition runs; everything
// else crossfades quickly (see "View transitions" in styles.css). When the API
// is missing or the user prefers reduced motion, the state change is applied
// plainly.
import { flushSync } from "react-dom";

export const MORPH_NAME = "series-cover";
/** An interaction older than this is not what opened the detail (e.g. an async open). */
const MAX_INTERACTION_AGE_MS = 1500;

// ---- pure helpers (tested) ----

export function canMorph(env: { supported: boolean; reducedMotion: boolean }): boolean {
  return env.supported && !env.reducedMotion;
}

/** True when any part of the box is on screen (an off-screen name would morph from nowhere). */
export function rectVisible(
  r: { left: number; top: number; right: number; bottom: number; width: number; height: number },
  vw: number,
  vh: number
): boolean {
  if (r.width <= 0 || r.height <= 0) return false;
  return r.right > 0 && r.bottom > 0 && r.left < vw && r.top < vh;
}

export function isFreshInteraction(now: number, at: number, maxAge = MAX_INTERACTION_AGE_MS): boolean {
  return now - at >= 0 && now - at <= maxAge;
}

/** The slice of the DOM `findPoster` needs, so it can be tested without one. */
export interface PosterNode {
  parentElement: PosterNode | null;
  matches(sel: string): boolean;
  querySelector(sel: string): PosterNode | null;
}

const MAX_CLIMB = 6;
const STOP_AT = ".view-panel";

/**
 * The cover element of the card/row a click landed in: climbs from the click
 * target to the first ancestor that contains a poster image (preferring the
 * poster image itself, then the .poster box, then any img). Stops at the tab
 * panel so a click on, say, a chart never picks an unrelated image.
 */
export function findPoster(target: PosterNode | null): PosterNode | null {
  let n = target;
  for (let i = 0; n && i <= MAX_CLIMB; i++) {
    if (n.matches(STOP_AT)) return null;
    if (n.matches("img")) return n;
    const hit = n.querySelector(".poster img") ?? n.querySelector(".poster") ?? n.querySelector("img");
    if (hit) return hit;
    n = n.parentElement;
  }
  return null;
}

// ---- last interaction tracking ----

let lastTarget: Element | null = null;
let lastAt = 0;

/** Remembers what the user last pressed, so the open callback (which only gets a Series) can find its poster. */
export function installInteractionTracker(onPress?: () => void): () => void {
  const remember = (e: Event) => {
    onPress?.();
    lastTarget = e.target instanceof Element ? e.target : null;
    lastAt = performance.now();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") remember(e);
  };
  window.addEventListener("pointerdown", remember, true);
  window.addEventListener("keydown", onKey, true);
  return () => {
    window.removeEventListener("pointerdown", remember, true);
    window.removeEventListener("keydown", onKey, true);
  };
}

/** Poster element of the card the user just activated, or null. */
export function posterOfLastInteraction(): HTMLElement | null {
  if (!lastTarget || !isFreshInteraction(performance.now(), lastAt)) return null;
  const hit = findPoster(lastTarget as unknown as PosterNode) as unknown as HTMLElement | null;
  return hit && hit.isConnected ? hit : null;
}

// ---- the transition ----

type VT = {
  finished: Promise<void>;
  ready: Promise<void>;
  updateCallbackDone: Promise<void>;
  skipTransition(): void;
};
type VTDocument = Document & { startViewTransition?: (cb: () => void) => VT };

let active: VT | null = null;
let viaMorph = false;

/** True when the detail now being mounted arrived through a morph (so its cover skips its own entrance). */
export function arrivedViaMorph(): boolean {
  return viaMorph;
}

function usable(el: Element | null | undefined): el is HTMLElement {
  if (!el || !el.isConnected) return false;
  return rectVisible(el.getBoundingClientRect(), window.innerWidth, window.innerHeight);
}

// Which transition currently owns an element's name, so a superseded
// transition's late cleanup never strips the name a newer one just set
// (e.g. open then quick close share the same poster).
const owners = new WeakMap<Element, object>();

function setName(el: Element | null | undefined, on: boolean, owner: object) {
  if (!el) return;
  if (on) {
    owners.set(el, owner);
    (el as HTMLElement).style.viewTransitionName = MORPH_NAME;
  } else if (owners.get(el) === owner) {
    owners.delete(el);
    (el as HTMLElement).style.viewTransitionName = "";
  }
}

/**
 * Runs `update` (a React state change) inside a view transition.
 * - `from`: element that morphs into the incoming `.detail-cover` (opening).
 * - `findTo`: called after the update commits; element the outgoing cover
 *   morphs into (closing). Null/off-screen -> plain crossfade.
 * - `opens`: this transition mounts a detail.
 */
export function transitionTo(
  update: () => void,
  opts: { from?: Element | null; findTo?: () => Element | null; opens?: boolean; after?: () => void } = {}
): void {
  const doc = document as VTDocument;
  const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  if (!canMorph({ supported: typeof doc.startViewTransition === "function", reducedMotion: reduced })) {
    viaMorph = false;
    update();
    opts.after?.();
    return;
  }
  active?.skipTransition();
  const from = usable(opts.from) ? opts.from : null;
  viaMorph = Boolean(opts.opens && from);
  const root = document.documentElement;
  const owner = {};
  root.dataset.vt = "1";
  setName(from, true, owner);
  let to: Element | null = null;
  const t = doc.startViewTransition!(() => {
    // The old snapshot is taken; drop the name so the new tree has one owner.
    setName(from, false, owner);
    flushSync(update);
    const cand = opts.findTo?.() ?? null;
    if (usable(cand)) {
      to = cand;
      setName(to, true, owner);
    }
    opts.after?.();
  });
  active = t;
  // Skipping a transition rejects these with AbortError; that is expected.
  t.ready.catch(() => {});
  t.updateCallbackDone.catch(() => {});
  const done = () => {
    setName(from, false, owner);
    setName(to, false, owner);
    if (active === t) {
      active = null;
      delete root.dataset.vt;
    }
  };
  t.finished.then(done, done);
}
