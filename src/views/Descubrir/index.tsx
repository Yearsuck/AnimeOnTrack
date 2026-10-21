import type { ReactNode } from "react";
import { useT } from "../../i18n";
import type { Series } from "../../types";
import type { SubView } from "./types";
import { SwipeView } from "./SwipeView";
import { ListasView } from "./ListasView";

// Descubrir (swipe deck) and Listas, hosted as sub-sections of Catálogo. The
// active sub-view is owned by App (controlled) and the section switch arrives
// as `headExtra`, rendered in the page head next to the title.
//
// onOpenSeries is the same App.tsx-owned navigation callback Pending/Library/
// AiringGrid already use to open SeriesDetail. Wiring it into Listas' "Quiero
// ver" rows is what makes trigger (c) from the design spec ("SeriesDetail
// opened on an unlinked catalog row" — see SeriesDetail.tsx's link-on-open
// effect) actually reachable: without a click-through here, an unfollowed
// catalog row had no UI path into SeriesDetail at all.
export function Descubrir({
  onOpenSeries,
  subView,
  onSwitchToSwipe,
  headExtra,
}: {
  onOpenSeries: (s: Series) => void;
  subView: SubView;
  onSwitchToSwipe: () => void;
  headExtra?: ReactNode;
}) {
  const t = useT();
  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.catalog")}</h2>
        {headExtra}
      </div>

      {subView === "swipe" && <SwipeView />}
      {subView === "listas" && (
        <ListasView onOpenSeries={onOpenSeries} onSwitchToSwipe={onSwitchToSwipe} />
      )}
    </div>
  );
}

export default Descubrir;
