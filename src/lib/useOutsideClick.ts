import { useEffect, useRef } from "react";

// Was implemented independently, byte-for-byte identically, in both
// Library.tsx's card overflow menu and Descubrir/components.tsx's
// OverflowMenu — one copy here, used by both. `onOutside` is read from a
// ref rather than listed as an effect dependency, so passing a fresh inline
// closure each render (the common case at both call sites) doesn't tear
// down and re-add the document listener on every render.
export function useOutsideClick(
  active: boolean,
  ref: React.RefObject<HTMLElement | null>,
  onOutside: () => void
) {
  const onOutsideRef = useRef(onOutside);
  onOutsideRef.current = onOutside;

  useEffect(() => {
    if (!active) return;
    function onMouseDown(e: MouseEvent) {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onOutsideRef.current();
      }
    }
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, [active, ref]);
}
