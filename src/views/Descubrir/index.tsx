import { useEffect, useState } from "react";
import { useT } from "../../i18n";
import type { Series } from "../../types";
import type { SubView } from "./types";
import { SwipeView } from "./SwipeView";
import { ListasView } from "./ListasView";

// onOpenSeries is the same App.tsx-owned navigation callback Pending/Library/
// AiringGrid already use to open SeriesDetail. Wiring it into Listas' "Quiero
// ver" rows is what makes trigger (c) from the design spec ("SeriesDetail
// opened on an unlinked catalog row" — see SeriesDetail.tsx's link-on-open
// effect) actually reachable: without a click-through here, an unfollowed
// catalog row had no UI path into SeriesDetail at all.
export function Descubrir({
  onOpenSeries,
  initialSubView = "swipe",
  onSubViewChange,
}: {
  onOpenSeries: (s: Series) => void;
  initialSubView?: SubView;
  onSubViewChange?: (v: SubView) => void;
}) {
  const t = useT();
  const [subView, setSubView] = useState<SubView>(initialSubView);

  useEffect(() => {
    setSubView(initialSubView);
  }, [initialSubView]);

  function handleSetSubView(v: SubView) {
    setSubView(v);
    onSubViewChange?.(v);
  }

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.discover")}</h2>
      </div>
      <div className="tabs discover-tabs">
        <button
          className={`tab ${subView === "swipe" ? "active" : ""}`}
          onClick={() => handleSetSubView("swipe")}
        >
          {t("discover.tabSwipe")}
        </button>
        <button
          className={`tab ${subView === "listas" ? "active" : ""}`}
          onClick={() => handleSetSubView("listas")}
        >
          {t("discover.tabLists")}
        </button>
      </div>

      {subView === "swipe" && <SwipeView />}
      {subView === "listas" && (
        <ListasView
          onOpenSeries={onOpenSeries}
          onSwitchToSwipe={() => handleSetSubView("swipe")}
        />
      )}
    </div>
  );
}

export default Descubrir;
