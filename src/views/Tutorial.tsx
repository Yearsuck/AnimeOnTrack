import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useBackLayer } from "../lib/backStack";
import { useT } from "../i18n";
import { TUTORIAL_STEPS, clampStep, type TutorialIllustration } from "../lib/tutorial";
import "../styles/tutorial.css";

interface TutorialProps {
  onClose: () => void;
}

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

const PAD = 6;

function Illustration({ kind }: { kind: TutorialIllustration }) {
  // Decorative, drawn with design tokens only (classes map to CSS variables).
  return (
    <svg className="tutorial-art" viewBox="0 0 200 96" aria-hidden="true" focusable="false">
      <rect className="ta-panel" x="4" y="4" width="192" height="88" rx="10" />
      {kind === "welcome" && (
        <>
          <circle className="ta-accent" cx="100" cy="42" r="22" />
          <path className="ta-onaccent" d="M92 32 L112 42 L92 52 Z" />
          <rect className="ta-line" x="62" y="74" width="76" height="6" rx="3" />
        </>
      )}
      {kind === "airing" && (
        <>
          {[0, 1, 2, 3].map((i) => (
            <rect key={i} className="ta-card" x={18 + i * 44} y="16" width="36" height="52" rx="6" />
          ))}
          <path className="ta-heart" d="M80 40 c-3-5-10-2-8 4 c1 4 8 8 8 8 s7-4 8-8 c2-6-5-9-8-4 z" />
          <rect className="ta-line" x="18" y="76" width="164" height="6" rx="3" />
        </>
      )}
      {kind === "pending" && (
        <>
          {[0, 1, 2].map((i) => (
            <g key={i}>
              <rect className="ta-card" x="18" y={14 + i * 26} width="164" height="20" rx="6" />
              <rect className="ta-line" x="26" y={21 + i * 26} width="70" height="6" rx="3" />
              <rect className="ta-accent" x="124" y={18 + i * 26} width="30" height="12" rx="6" />
              <circle className="ta-ok" cx="166" cy={24 + i * 26} r="6" />
            </g>
          ))}
        </>
      )}
      {kind === "library" && (
        <>
          <rect className="ta-chip ta-chip-on" x="18" y="14" width="44" height="14" rx="7" />
          <rect className="ta-chip" x="68" y="14" width="44" height="14" rx="7" />
          <rect className="ta-chip" x="118" y="14" width="44" height="14" rx="7" />
          {[0, 1, 2, 3, 4].map((i) => (
            <rect key={i} className="ta-card" x={18 + i * 33} y="38" width="27" height="42" rx="5" />
          ))}
        </>
      )}
      {kind === "catalog" && (
        <>
          <rect className="ta-card" x="72" y="14" width="56" height="72" rx="8" />
          <path className="ta-arrow" d="M62 50 H40 M46 44 L38 50 L46 56" />
          <path className="ta-arrow" d="M138 50 H160 M154 44 L162 50 L154 56" />
          <path className="ta-arrow" d="M100 12 V6 M95 11 L100 5 L105 11" />
        </>
      )}
      {kind === "settings" && (
        <>
          {[0, 1, 2].map((i) => (
            <g key={i}>
              <rect className="ta-line" x="22" y={20 + i * 22} width="90" height="6" rx="3" />
              <rect className="ta-chip" x="136" y={15 + i * 22} width="40" height="16" rx="8" />
              <circle className={i === 1 ? "ta-accent" : "ta-muted"} cx={i === 1 ? 168 : 144} cy={23 + i * 22} r="6" />
            </g>
          ))}
        </>
      )}
    </svg>
  );
}

export function Tutorial({ onClose }: TutorialProps) {
  const t = useT();
  const [index, setIndex] = useState(0);
  const [box, setBox] = useState<Box | null>(null);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const primaryRef = useRef<HTMLButtonElement | null>(null);

  const step = TUTORIAL_STEPS[clampStep(index)];
  const last = index >= TUTORIAL_STEPS.length - 1;
  const total = TUTORIAL_STEPS.length;

  const go = useCallback((n: number) => setIndex(clampStep(n)), []);
  const next = useCallback(() => {
    if (last) onClose();
    else setIndex((i) => clampStep(i + 1));
  }, [last, onClose]);
  const back = useCallback(() => setIndex((i) => clampStep(i - 1)), []);
  // System back skips the tour, like Escape.
  useBackLayer(true, onClose);

  // Ring the tab for this step; fall back to a centered card when missing.
  useLayoutEffect(() => {
    function measure() {
      const el = step.target
        ? document.querySelector<HTMLElement>(`[data-tour="${step.target}"]`)
        : null;
      if (!el) return setBox(null);
      const r = el.getBoundingClientRect();
      if (r.width === 0 || r.height === 0) return setBox(null);
      setBox({ x: r.left - PAD, y: r.top - PAD, w: r.width + PAD * 2, h: r.height + PAD * 2 });
    }
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, [step.target]);

  // Focus management: take focus on open, restore on close.
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    return () => previous?.focus?.();
  }, []);

  // Keep focus on the primary button on open and after each step change.
  useEffect(() => {
    primaryRef.current?.focus();
  }, [index]);

  // Capture-phase keys: Escape skips, arrows navigate, Tab is trapped, and
  // single-character keys must not reach App's global shortcuts (1-6, ?, ...)
  // while the modal is open.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        onClose();
        return;
      }
      if (e.key === "ArrowRight") {
        e.preventDefault();
        e.stopImmediatePropagation();
        next();
        return;
      }
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        e.stopImmediatePropagation();
        back();
        return;
      }
      if (e.key === "Tab") {
        const nodes = dialogRef.current?.querySelectorAll<HTMLElement>("button:not([disabled])");
        if (!nodes || nodes.length === 0) return;
        const first = nodes[0];
        const lastNode = nodes[nodes.length - 1];
        const active = document.activeElement;
        if (!dialogRef.current?.contains(active)) {
          e.preventDefault();
          first.focus();
        } else if (e.shiftKey && active === first) {
          e.preventDefault();
          lastNode.focus();
        } else if (!e.shiftKey && active === lastNode) {
          e.preventDefault();
          first.focus();
        }
        return;
      }
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopImmediatePropagation();
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [next, back, onClose]);

  return (
    <div className={`tutorial-backdrop ${box ? "tutorial-backdrop-spot" : ""}`}>
      {box && (
        <div
          className="tutorial-ring"
          aria-hidden="true"
          style={{ transform: `translate(${box.x}px, ${box.y}px)`, width: box.w, height: box.h }}
        />
      )}
      <div
        ref={dialogRef}
        className="tutorial-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tutorial-title"
        aria-describedby="tutorial-body"
        data-step={step.id}
      >
        <div className="tutorial-step" key={step.id}>
          <Illustration kind={step.illustration} />
          <h2 id="tutorial-title" className="tutorial-title">
            {t(step.titleKey)}
          </h2>
          <p id="tutorial-body" className="tutorial-body">
            {t(step.bodyKey)}
          </p>
        </div>
        <div
          className="tutorial-dots"
          role="group"
          aria-label={t("tutorial.progressAria", { current: index + 1, total })}
        >
          {TUTORIAL_STEPS.map((s, i) => (
            <button
              key={s.id}
              type="button"
              className={`tutorial-dot ${i === index ? "active" : ""}`}
              aria-label={t("tutorial.dotAria", { n: i + 1 })}
              aria-current={i === index ? "step" : undefined}
              onClick={() => go(i)}
            />
          ))}
        </div>
        <div className="tutorial-actions">
          {!last ? (
            <button type="button" className="btn btn-ghost" onClick={onClose}>
              {t("tutorial.skip")}
            </button>
          ) : (
            <span />
          )}
          <div className="tutorial-nav">
            {index > 0 && (
              <button type="button" className="btn btn-ghost" onClick={back}>
                {t("tutorial.back")}
              </button>
            )}
            <button ref={primaryRef} type="button" className="btn btn-primary" onClick={next}>
              {last ? t("tutorial.start") : t("tutorial.next")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
