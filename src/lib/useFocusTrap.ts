import { useEffect, useRef } from "react";

export const FOCUSABLE_SELECTOR =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Index to focus when Tab / Shift+Tab is pressed, or null to let the browser
 * move focus natively. `current` is the index of the focused item inside the
 * container, or -1 when focus is outside it.
 */
export function nextFocusIndex(current: number, count: number, shift: boolean): number | null {
  if (count <= 0) return null;
  if (current < 0) return shift ? count - 1 : 0;
  if (shift && current === 0) return count - 1;
  if (!shift && current === count - 1) return 0;
  return null;
}

interface FocusTrapOptions {
  /** Called on Escape. */
  onEscape?: () => void;
  /** Element to focus on open; defaults to the first focusable in the container. */
  initialFocus?: () => HTMLElement | null | undefined;
}

/**
 * While `active`, keeps Tab inside `ref`, focuses the first element on open,
 * and returns focus to the previously focused element on close.
 */
export function useFocusTrap(
  active: boolean,
  ref: React.RefObject<HTMLElement | null>,
  options: FocusTrapOptions = {}
) {
  const optsRef = useRef(options);
  optsRef.current = options;

  useEffect(() => {
    if (!active) return;
    const opener = document.activeElement as HTMLElement | null;
    const items = () =>
      [...(ref.current?.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR) ?? [])];
    const target = optsRef.current.initialFocus?.() ?? items()[0];
    target?.focus();

    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && optsRef.current.onEscape) {
        e.preventDefault();
        optsRef.current.onEscape();
        return;
      }
      if (e.key !== "Tab") return;
      const list = items();
      const idx = list.indexOf(document.activeElement as HTMLElement);
      const next = nextFocusIndex(idx, list.length, e.shiftKey);
      if (next === null) return;
      e.preventDefault();
      list[next].focus();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      opener?.focus?.();
    };
  }, [active, ref]);
}
