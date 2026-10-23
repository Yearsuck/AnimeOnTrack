// Small imperative motion helpers (the CSS-only ones live in styles.css).

export function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/** A quick scale pop (WAAPI, transform only). Interruptible: a new pop cancels the running one. */
export function pop(el: HTMLElement | null, peak = 1.28): void {
  if (!el || prefersReducedMotion() || typeof el.animate !== "function") return;
  el.getAnimations().forEach((a) => a.cancel());
  el.animate(
    [{ transform: "scale(1)" }, { transform: `scale(${peak})`, offset: 0.4 }, { transform: "scale(1)" }],
    { duration: 260, easing: "cubic-bezier(0.16, 1, 0.3, 1)" }
  );
}
