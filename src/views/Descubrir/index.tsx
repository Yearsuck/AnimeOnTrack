import { useState } from "react";
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
export function Descubrir({ onOpenSeries }: { onOpenSeries: (s: Series) => void }) {
  const t = useT();
  const [subView, setSubView] = useState<SubView>("swipe");

  return (
    <div className="page">
      <div className="page-head">
        <h2 className="page-title">{t("nav.discover")}</h2>
      </div>
      <div className="tabs" style={{ marginBottom: 20 }}>
        <button
          className={`tab ${subView === "swipe" ? "active" : ""}`}
          onClick={() => setSubView("swipe")}
        >
          {t("discover.tabSwipe")}
        </button>
        <button
          className={`tab ${subView === "listas" ? "active" : ""}`}
          onClick={() => setSubView("listas")}
        >
          {t("discover.tabLists")}
        </button>
      </div>

      {subView === "swipe" && <SwipeView />}
      {subView === "listas" && <ListasView onOpenSeries={onOpenSeries} />}
    </div>
  );
}

export default Descubrir;
