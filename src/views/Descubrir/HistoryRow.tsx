import { useT } from "../../i18n";
import type { SwipeHistoryItem } from "../../types";
import { DECISION_BADGE } from "./constants";
import { resolveCoverUrl } from "../../lib/posterFallback";

// One row of the swipe-history strip: poster, title, current-decision badge,
// quick re-classify buttons, and a "return to deck" undo for this one card.
export function HistoryRow({
  item,
  onReclassify,
  onReturn,
}: {
  item: SwipeHistoryItem;
  onReclassify: (item: SwipeHistoryItem, action: "discard" | "want" | "seen") => void;
  onReturn: (item: SwipeHistoryItem) => void;
}) {
  const t = useT();
  const posterSrc = resolveCoverUrl(item.poster_url);
  return (
    <div className="swipe-history-row">
      {posterSrc && <img src={posterSrc} alt="" />}

      <div className="swipe-history-main">
        <div className="swipe-history-title" title={item.title}>
          {item.title}
        </div>
        <div className="muted text-xs">
          {t(DECISION_BADGE[item.decision])}
        </div>
      </div>
      <div className="swipe-history-actions">
        <button
          className="btn btn-ghost"
          title={t("discover.discard")}
          onClick={() => onReclassify(item, "discard")}
          disabled={item.decision === "discard"}
        >
          ✕
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.want")}
          onClick={() => onReclassify(item, "want")}
          disabled={item.decision === "want"}
        >
          ★
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.seen")}
          onClick={() => onReclassify(item, "seen")}
          disabled={item.decision === "seen"}
        >
          ✓
        </button>
        <button
          className="btn btn-ghost"
          title={t("discover.returnToDeck")}
          onClick={() => onReturn(item)}
        >
          ↺
        </button>
      </div>
    </div>
  );
}
