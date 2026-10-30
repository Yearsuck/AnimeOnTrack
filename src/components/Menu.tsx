import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { computeMenuPosition, nextMenuIndex } from "../lib/menuPlacement";
import { useBackLayer } from "../lib/backStack";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
  icon?: React.ReactNode;
}

/**
 * Overflow menu. The list is portalled to document.body with position:fixed,
 * so no ancestor's overflow/containment/transform can clip it.
 */
export function Menu({
  label,
  items,
  className = "",
}: {
  label: string;
  items: MenuItem[];
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  // `mounted` outlives `open` by the exit animation, so the list can scale
  // out toward its button instead of vanishing. While it is leaving it is
  // inert (pointer-events:none in CSS) and `open` is already false.
  const [mounted, setMounted] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number; origin: string } | null>(null);
  const btnRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const close = useCallback((refocus: boolean) => {
    setOpen(false);
    if (refocus) btnRef.current?.focus();
  }, []);

  // Measure the (hidden) list once rendered, then place it.
  useLayoutEffect(() => {
    if (!open) return;
    setMounted(true);
    if (!btnRef.current || !listRef.current) return;
    const r = btnRef.current.getBoundingClientRect();
    // Layout size, not getBoundingClientRect: the list is mid scale-in here.
    const m = { width: listRef.current.offsetWidth, height: listRef.current.offsetHeight };
    const p = computeMenuPosition(
      r,
      { width: m.width, height: m.height },
      { width: window.innerWidth, height: window.innerHeight }
    );
    // Grow from the trigger: origin = the button's centre in the list's own
    // coordinates (so a menu flipped above/left of the button still scales
    // out of it).
    const ox = Math.round(r.left + r.width / 2 - p.left);
    const oy = Math.round(r.top + r.height / 2 - p.top);
    setPos({ top: p.top, left: p.left, origin: `${ox}px ${oy}px` });
  }, [open, mounted]);

  const placed = pos !== null;
  useEffect(() => {
    if (open && placed && mounted) {
      listRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    }
  }, [open, placed, mounted]);

  // Safety net: if the exit animation never reports (e.g. animations
  // disabled), unmount anyway.
  useEffect(() => {
    if (open || !mounted) return;
    const id = window.setTimeout(() => {
      setMounted(false);
      setPos(null);
    }, 250);
    return () => window.clearTimeout(id);
  }, [open, mounted]);

  useEffect(() => {
    if (!open) return;
    function onDown(e: PointerEvent) {
      const n = e.target as Node;
      if (listRef.current?.contains(n) || btnRef.current?.contains(n)) return;
      close(false);
    }
    const onAway = () => close(false);
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onAway, true);
    window.addEventListener("resize", onAway);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onAway, true);
      window.removeEventListener("resize", onAway);
    };
  }, [open, close]);
  useBackLayer(open, () => close(false));

  if (items.length === 0) return null;

  function onListKeyDown(e: React.KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
      return;
    }
    if (e.key === "Tab") {
      close(false);
      return;
    }
    const els = Array.from(
      listRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? []
    );
    const cur = els.indexOf(document.activeElement as HTMLElement);
    const next = nextMenuIndex(e.key, cur, els.length);
    if (next !== null) {
      e.preventDefault();
      els[next]?.focus();
    }
  }

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`card-menu-btn ${className}`.trim()}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          if (open) close(false);
          else setOpen(true);
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "ArrowDown" && !open) {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        <DotsIcon />
      </button>
      {mounted &&
        createPortal(
          <div
            ref={listRef}
            className="menu-list"
            role="menu"
            aria-label={label}
            data-state={open ? "open" : "closed"}
            style={{
              top: pos?.top ?? 0,
              left: pos?.left ?? 0,
              visibility: pos ? "visible" : "hidden",
              transformOrigin: pos?.origin,
            }}
            onAnimationEnd={(e) => {
              if (!open && e.animationName.startsWith("menu-")) {
                setMounted(false);
                setPos(null);
              }
            }}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={onListKeyDown}
          >
            {items.map((it, i) => (
              <button
                key={i}
                type="button"
                role="menuitem"
                tabIndex={-1}
                className={`menu-item${it.danger ? " danger" : ""}`}
                onClick={(e) => {
                  e.stopPropagation();
                  close(true);
                  it.onSelect();
                }}
              >
                {it.icon && <span className="menu-item-icon">{it.icon}</span>}
                {it.label}
              </button>
            ))}
          </div>,
          document.body
        )}
    </>
  );
}

export function DotsIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
      <circle cx="3" cy="8" r="1.4" />
      <circle cx="8" cy="8" r="1.4" />
      <circle cx="13" cy="8" r="1.4" />
    </svg>
  );
}
