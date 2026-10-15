import { useEffect, useRef, useState } from "react";
import { SPLASH_FADE_MS, SPLASH_VISIBLE_MS, shouldShowSplash } from "../lib/splash";

interface SplashProps {
  onDone: () => void;
}

export function Splash({ onDone }: SplashProps) {
  const [fading, setFading] = useState(false);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(() => {
    if (fading) {
      const fadeTimer = setTimeout(() => {
        onDoneRef.current();
      }, SPLASH_FADE_MS);
      return () => {
        clearTimeout(fadeTimer);
      };
    }

    const visibleTimer = setTimeout(() => {
      setFading(true);
    }, SPLASH_VISIBLE_MS);

    const handleDismiss = () => {
      setFading(true);
    };

    window.addEventListener("pointerdown", handleDismiss);
    window.addEventListener("keydown", handleDismiss);

    return () => {
      clearTimeout(visibleTimer);
      window.removeEventListener("pointerdown", handleDismiss);
      window.removeEventListener("keydown", handleDismiss);
    };
  }, [fading]);

  return (
    <div
      className={`splash ${fading ? "splash--fading" : ""}`.trim()}
      role="presentation"
      aria-hidden="true"
    >
      <img
        className="splash-logo"
        src="/app-icon.png"
        alt=""
        width={96}
        height={96}
      />
      <div className="splash-bar" />
      <div className="splash-name">AnimeOnTrack</div>
    </div>
  );
}

// Module-level so StrictMode's double mount and any remount never replay it:
// the splash is once per app launch.
let splashShown = false;

/**
 * Mounts the splash next to `<App />` (see main.tsx) instead of inside it, so
 * it covers every one of App's early returns (loading, onboarding, the shell)
 * without threading state through them.
 */
export function SplashHost() {
  const [show, setShow] = useState(() => {
    const reduced =
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const visible = shouldShowSplash(reduced, splashShown);
    splashShown = true;
    return visible;
  });
  return show ? <Splash onDone={() => setShow(false)} /> : null;
}
