import { useT } from "../../i18n";
import type { SwipeHistoryItem } from "../../types";
import { DECISION_BADGE } from "./constants";
import { resolveCoverUrl } from "../../lib/posterFallback";
import { Menu, type MenuItem } from "../../components/Menu";

// One card of the "recently decided" strip: poster, title, current decision,
// and a menu to re-classify it or send it back to the deck.
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
  const items: MenuItem[] = [
    ...(["want", "seen", "discard"] as const)
      .filter((a) => a !== item.decision)
      .map((a) => ({ label: t(`discover.${a}`), onSelect: () => onReclassify(item, a) })),
    { label: t("discover.returnToDeck"), onSelect: () => onReturn(item) },
  ];
  return (
    <div className={`disc-recent-item decision-${item.decision}`}>
      {posterSrc ? <img src={posterSrc} alt="" /> : <span className="disc-recent-ph" aria-hidden="true" />}
      <div className="disc-recent-text">
        <span className="disc-recent-title" title={item.title}>
          {item.title}
        </span>
        <span className="disc-recent-decision">{t(DECISION_BADGE[item.decision])}</span>
      </div>
      <Menu label={t("discover.changeDecision", { title: item.title })} items={items} />
    </div>
  );
}
