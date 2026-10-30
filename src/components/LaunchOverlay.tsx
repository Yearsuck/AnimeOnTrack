import { useEffect, useLayoutEffect, useRef, useSyncExternalStore } from "react";
import { useBackLayer } from "../lib/backStack";
import { createPortal } from "react-dom";
import { useT } from "../i18n";
import { dispatchLaunch, getLaunchState, subscribeLaunch } from "../lib/launchEpisode";
import { flipDelta, flipTransform } from "../lib/flip";
import "../styles/launch.css";

const ENTER_MS = 320;
const EXIT_MS = 260;
const EASE_OUT = "cubic-bezier(0.23, 1, 0.32, 1)";

const reduced = () =>
  typeof window !== "undefined" && !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/** Mount once (App.tsx). Renders the curtain while an episode window opens. */
export function LaunchHost() {
  const st = useSyncExternalStore(subscribeLaunch, getLaunchState);
  const t = useT();
  const curtain = useRef<HTMLDivElement>(null);
  const card = useRef<HTMLDivElement>(null);
  const seqShown = useRef(0);
  const active = st.phase !== "idle" && st.req != null;
  // System back dismisses the curtain like Escape (not while it is already leaving).
  useBackLayer(active && st.phase !== "closing", () => dispatchLaunch({ type: "dismiss" }));

  // Enter: grow out of the clicked element. Curtain via clip-path (exact rect),
  // card via the shared FLIP transform. Fade only under reduced motion.
  useLayoutEffect(() => {
    if (!active || !st.req || seqShown.current === st.seq) return;
    seqShown.current = st.seq;
    const c = curtain.current;
    const k = card.current;
    if (!c || !k) return;
    // A launch during the closing phase must not inherit the forwards-filled exit.
    c.getAnimations().forEach((a) => a.cancel());
    const opts = { duration: reduced() ? 160 : ENTER_MS, easing: EASE_OUT, fill: "backwards" as const };
    const o = st.req.origin;
    if (reduced() || !o) {
      c.animate([{ opacity: 0 }, { opacity: 1 }], opts);
      return;
    }
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const r = Math.min(o.width, o.height) / 2;
    c.animate(
      [
        {
          clipPath: `inset(${o.top}px ${vw - o.left - o.width}px ${vh - o.top - o.height}px ${o.left}px round ${r}px)`,
          opacity: 0.6,
        },
        { clipPath: "inset(0px 0px 0px 0px round 0px)", opacity: 1 },
      ],
      opts
    );
    const d = flipDelta(o, k.getBoundingClientRect());
    if (d) {
      k.animate([{ transform: flipTransform(d), opacity: 0 }, { transform: "none", opacity: 1 }], opts);
    }
  }, [active, st.seq, st.req]);

  // Exit: zoom + fade as if handing off to the new window.
  useEffect(() => {
    if (st.phase !== "closing") return;
    const c = curtain.current;
    if (!c) {
      dispatchLaunch({ type: "exited", seq: st.seq });
      return;
    }
    const anim = c.animate(
      reduced()
        ? [{ opacity: 1 }, { opacity: 0 }]
        : [
            { opacity: 1, transform: "scale(1)" },
            { opacity: 0, transform: "scale(1.04)" },
          ],
      { duration: reduced() ? 140 : EXIT_MS, easing: EASE_OUT, fill: "forwards" }
    );
    const done = () => dispatchLaunch({ type: "exited", seq: st.seq });
    anim.addEventListener("finish", done);
    // Safety net: never leave the curtain stuck if "finish" does not fire.
    const safety = setTimeout(done, EXIT_MS + 100);
    return () => {
      clearTimeout(safety);
      anim.removeEventListener("finish", done);
      anim.cancel();
    };
  }, [st.phase, st.seq]);

  useEffect(() => {
    if (!active) return;
    // Capture phase: runs before App.tsx shortcuts and the SeriesDetail Escape
    // handler, and swallows every key so nothing acts underneath the curtain.
    const onKey = (e: KeyboardEvent) => {
      e.stopImmediatePropagation();
      e.preventDefault();
      if (e.key === "Escape") dispatchLaunch({ type: "dismiss" });
    };
    window.addEventListener("keydown", onKey, true);
    const prevFocus = document.activeElement as HTMLElement | null;
    const root = document.getElementById("root");
    root?.setAttribute("inert", "");
    curtain.current?.focus({ preventScroll: true });
    return () => {
      window.removeEventListener("keydown", onKey, true);
      root?.removeAttribute("inert");
      if (prevFocus && prevFocus.isConnected) prevFocus.focus({ preventScroll: true });
    };
  }, [active]);

  if (!active || !st.req) return null;
  const { req } = st;
  const isError = st.phase === "error";
  return createPortal(
    <div
      ref={curtain}
      className="launch"
      tabIndex={-1}
      role="status"
      aria-live="polite"
      onClick={() => dispatchLaunch({ type: "dismiss" })}
    >
      {req.cover && <img className="launch-bg" src={req.cover} alt="" aria-hidden="true" />}
      <div className="launch-shade" aria-hidden="true" />
      <div ref={card} className="launch-card">
        <div className="launch-title">{req.title}</div>
        {req.episode && <div className="launch-ep">{t("common.episodeNumber", { number: req.episode })}</div>}
        {isError ? (
          <div className="launch-status launch-error">
            {t("launch.error")}
            {st.error ? ` ${st.error}` : ""}
          </div>
        ) : (
          <>
            <div className="launch-status">
              {req.site ? t("launch.openingSite", { site: req.site }) : t("launch.opening")}
            </div>
            <div className="launch-bar" aria-hidden="true">
              <span />
            </div>
          </>
        )}
      </div>
    </div>,
    document.body
  );
}
