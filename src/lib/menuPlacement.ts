export interface Rect {
  top: number;
  left: number;
  right: number;
  bottom: number;
}
export interface Size {
  width: number;
  height: number;
}

export const MENU_MARGIN = 8;
const GAP = 4;

/**
 * Where to put a fixed-position menu relative to its trigger. Default: below
 * the trigger, right edges aligned. Flips upward when it does not fit below
 * (and there is more room above), falls back to left-aligned when
 * right-aligned would run off the left edge, then clamps inside the viewport
 * with a margin.
 */
export function computeMenuPosition(
  trigger: Rect,
  menu: Size,
  viewport: Size,
  margin: number = MENU_MARGIN
): { top: number; left: number; placement: "bottom" | "top" } {
  const spaceBelow = viewport.height - trigger.bottom - margin;
  const spaceAbove = trigger.top - margin;
  const needed = menu.height + GAP;
  const placement: "bottom" | "top" =
    spaceBelow >= needed || spaceBelow >= spaceAbove ? "bottom" : "top";
  let top = placement === "bottom" ? trigger.bottom + GAP : trigger.top - GAP - menu.height;

  let left = trigger.right - menu.width;
  if (left < margin) left = trigger.left;

  const maxLeft = viewport.width - margin - menu.width;
  const maxTop = viewport.height - margin - menu.height;
  left = Math.max(margin, Math.min(left, maxLeft));
  top = Math.max(margin, Math.min(top, maxTop));
  return { top, left, placement };
}

/** Next focused index for ArrowUp/Down/Home/End; null when the key is not handled. */
export function nextMenuIndex(key: string, current: number, count: number): number | null {
  if (count <= 0) return null;
  switch (key) {
    case "ArrowDown":
      return current < 0 ? 0 : (current + 1) % count;
    case "ArrowUp":
      return current < 0 ? count - 1 : (current - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}
