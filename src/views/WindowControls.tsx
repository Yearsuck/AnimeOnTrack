import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Custom minimize / maximize-restore / close controls for the frameless
// window (the native title bar is disabled via `decorations: false` in
// tauri.conf.json, so these replace it). Dragging the window is handled
// separately by the `data-tauri-drag-region` attribute on the topbar.
//
// The maximize glyph flips to a "restore" (overlapping squares) icon while
// the window is maximized; state is kept in sync via the window's own resize
// event so it stays correct even when the OS maximizes the window some other
// way (double-click drag region, Win+Up, snapping).
export function WindowControls() {
  const win = getCurrentWindow();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    win.isMaximized().then(setMaximized).catch(() => {});
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => {});
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, [win]);

  return (
    <div className="win-controls">
      <button
        type="button"
        className="win-btn"
        aria-label="Minimizar"
        onClick={() => win.minimize()}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <rect x="0" y="4.5" width="10" height="1" fill="currentColor" />
        </svg>
      </button>
      <button
        type="button"
        className="win-btn"
        aria-label={maximized ? "Restaurar" : "Maximizar"}
        onClick={() => win.toggleMaximize()}
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <rect x="0.5" y="2.5" width="6" height="6" fill="none" stroke="currentColor" />
            <path d="M2.5 2.5V0.5H9.5V7.5H7.5" fill="none" stroke="currentColor" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" />
          </svg>
        )}
      </button>
      <button
        type="button"
        className="win-btn win-btn-close"
        aria-label="Cerrar"
        onClick={() => win.close()}
      >
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M0.5 0.5L9.5 9.5M9.5 0.5L0.5 9.5" stroke="currentColor" />
        </svg>
      </button>
    </div>
  );
}
