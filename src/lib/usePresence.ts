import { useEffect, useRef, useState } from "react";
import { EXIT_MS, type Presence } from "./progress";

/**
 * Keeps an element mounted while it plays its exit: `show` going false yields
 * "leaving" for `exitMs`, then "hidden". Showing again during the leave returns
 * to "shown" on the same element immediately in the same render, so the CSS
 * transition reverses from where it is without a 1-frame flash.
 * Entrance is CSS (@starting-style), so it needs no state here.
 */
export function usePresence(show: boolean, exitMs = EXIT_MS): Presence {
  const [presence, setPresence] = useState<Presence>(show ? "shown" : "hidden");
  const prevShowRef = useRef(show);

  let currentPresence = presence;
  if (show && presence !== "shown") {
    currentPresence = "shown";
    setPresence("shown");
  } else if (!show && prevShowRef.current && presence === "shown") {
    currentPresence = "leaving";
    setPresence("leaving");
  }
  prevShowRef.current = show;

  useEffect(() => {
    if (presence !== "leaving") return;
    const id = setTimeout(() => {
      setPresence((s) => (s === "leaving" ? "hidden" : s));
    }, exitMs);
    return () => clearTimeout(id);
  }, [presence, exitMs]);

  return show ? "shown" : currentPresence;
}
