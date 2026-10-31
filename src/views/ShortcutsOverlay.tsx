import { useT, type MessageKey } from "../i18n";
import { useBackLayer } from "../lib/backStack";
import { SHORTCUT_ROWS } from "../lib/shortcuts";
import { isMobile } from "../lib/platform";

interface ShortcutsOverlayProps {
  open: boolean;
  onClose: () => void;
}

export function ShortcutsOverlay({ open, onClose }: ShortcutsOverlayProps) {
  const t = useT();
  useBackLayer(open, onClose);

  if (isMobile || !open) return null;

  return (
    <div
      className="shortcuts-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        className="shortcuts-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("shortcuts.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="shortcuts-header">
          <h2 className="shortcuts-title">{t("shortcuts.title")}</h2>
          <button
            type="button"
            className="shortcuts-close"
            onClick={onClose}
            aria-label={t("shortcuts.close")}
            autoFocus
          >
            ✕
          </button>
        </div>
        <div className="shortcuts-list">
          {SHORTCUT_ROWS.map((row) => (
            <div key={row.labelKey} className="shortcuts-row">
              <span className="shortcuts-label">{t(row.labelKey as MessageKey)}</span>
              <kbd className="shortcuts-key">{row.keys}</kbd>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
