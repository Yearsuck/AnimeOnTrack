// Sliding "active pill" behind the top-level tabs and every `.seg` control.
//
// One pseudo-element per container (`::before`, styled in styles.css) is moved
// with `transform: translateX(var(--ind-x))` and sized with `--ind-w`; this
// module only measures the active item and publishes those two variables, so
// no caller has to change: any `.seg` / `.tabs` that has a `.active` child
// gets the slide. Without JS (or before the first measure) the old per-item
// `.active` background is still what shows.

/** Items an indicator can sit behind. */
export const INDICATOR_CONTAINERS = ".seg, .tabs";
const ACTIVE_ITEM = ".seg-btn.active, .tab.active";

export interface IndicatorBox {
  x: number;
  w: number;
}

/**
 * Position of the pill inside its container, from the active item's
 * offsetLeft/offsetWidth (layout values, so a press-scale transform on the
 * item cannot skew them). Null when there is nothing sensible to show: the
 * container or the item is not laid out (display:none, zero width).
 */
export function measureIndicator(
  item: { offsetLeft: number; offsetWidth: number },
  containerWidth: number
): IndicatorBox | null {
  if (!(containerWidth > 0) || !(item.offsetWidth > 0)) return null;
  const x = Math.max(0, item.offsetLeft);
  const w = Math.min(item.offsetWidth, Math.max(0, containerWidth - x));
  return w > 0 ? { x, w } : null;
}

/** Direction of a tab switch, by position in the nav: "fwd" = to the right. */
export function tabDirection<T>(order: readonly T[], from: T, to: T): "fwd" | "back" {
  const a = order.indexOf(from);
  const b = order.indexOf(to);
  return a >= 0 && b >= 0 && b < a ? "back" : "fwd";
}

type Mode = "slide" | "snap";
type Read = { el: HTMLElement; box: IndicatorBox | null; mode: Mode };

/** Phase 1: layout reads only (no writes, so one layout for all containers). */
function read(el: HTMLElement, mode: Mode): Read {
  const item = el.querySelector<HTMLElement>(ACTIVE_ITEM);
  return { el, box: item ? measureIndicator(item, el.clientWidth) : null, mode };
}

function enableSlideLater(el: HTMLElement) {
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      if (el.dataset.ind === "snap") el.dataset.ind = "on";
    })
  );
}

/** Phase 2: style writes only. */
function write({ el, box, mode }: Read) {
  if (!box) {
    // Nothing active/visible: hide the pill, keep the last vars so it fades
    // out in place instead of sliding somewhere.
    if (el.dataset.ind) el.dataset.ind = "idle";
    return;
  }
  const state = el.dataset.ind;
  // Snap (no transition) for the first placement, coming back from idle, and
  // resize/relayout-driven updates; the slide is re-enabled two frames later
  // so the pill never glides in from x=0 or lags behind a window resize.
  const snap = !state || state === "idle" || (mode === "snap" && state === "on");
  if (snap) el.dataset.ind = "snap";
  el.style.setProperty("--ind-x", `${box.x}px`);
  el.style.setProperty("--ind-w", `${box.w}px`);
  if (snap || state === "snap") enableSlideLater(el);
}

/** Closest indicator container of a node (text nodes use their parent). */
function hostOf(n: Node | null): HTMLElement | null {
  const el = n instanceof HTMLElement ? n : (n?.parentElement ?? null);
  return (el?.closest(INDICATOR_CONTAINERS) as HTMLElement | null) ?? null;
}

/**
 * Watches the document for indicator containers (added, re-labelled, switched
 * active item, resized) and keeps their `--ind-*` variables current. Observes
 * the whole body (portalled content may contain a `.seg` too) but bails out
 * of every record that is not inside a container. Returns a cleanup.
 */
export function installSlidingIndicators(root: HTMLElement = document.body): () => void {
  const known = new Set<HTMLElement>();
  const pending = new Map<HTMLElement, Mode>();
  let scheduled = false;
  const resize = new ResizeObserver((entries) => {
    for (const e of entries) pending.set(e.target as HTMLElement, "snap");
    schedule();
  });

  function track(el: HTMLElement, mode: Mode = "slide") {
    if (!known.has(el)) {
      known.add(el);
      resize.observe(el);
    }
    if (pending.get(el) !== "snap") pending.set(el, mode);
  }
  function schedule() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      const reads: Read[] = [];
      for (const [el, mode] of pending) {
        if (el.isConnected) reads.push(read(el, mode));
        else {
          known.delete(el);
          resize.unobserve(el);
        }
      }
      pending.clear();
      reads.forEach(write);
    });
  }
  function collect(node: Node) {
    if (!(node instanceof HTMLElement)) return;
    if (node.matches(INDICATOR_CONTAINERS)) track(node);
    if (node.firstElementChild) node.querySelectorAll<HTMLElement>(INDICATOR_CONTAINERS).forEach((e) => track(e));
  }

  const mo = new MutationObserver((records) => {
    for (const r of records) {
      if (r.type === "childList") {
        r.addedNodes.forEach(collect);
        const host = hostOf(r.target);
        if (host) track(host);
      } else if (r.type === "attributes") {
        // Cheap pre-check: only the items themselves flip the pill.
        const t = r.target as HTMLElement;
        if (t.classList.contains("seg-btn") || t.classList.contains("tab")) {
          const host = hostOf(t);
          if (host) track(host);
        }
      } else {
        // characterData: a label's text changed (language switch).
        const host = hostOf(r.target);
        if (host) track(host);
      }
    }
    if (pending.size) schedule();
  });
  mo.observe(root, {
    subtree: true,
    childList: true,
    characterData: true,
    attributes: true,
    attributeFilter: ["class", "aria-selected", "aria-pressed"],
  });
  collect(root);
  schedule();

  return () => {
    mo.disconnect();
    resize.disconnect();
    known.clear();
    pending.clear();
  };
}
