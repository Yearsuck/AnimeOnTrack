import { useCallback, useEffect, useRef, useState } from "react";
import { Onboarding } from "./views/Onboarding";
import { AiringGrid } from "./views/AiringGrid";
import { Pending } from "./views/Pending";
import { SeriesDetail } from "./views/SeriesDetail";
import { Settings } from "./views/Settings";
import { Library } from "./views/Library";
import { ProgressBar } from "./views/ProgressBar";
import { listAiring, refresh, pendingCount } from "./api";
import type { Series } from "./types";

type View = "loading" | "onboarding" | "pending" | "airing" | "library" | "settings" | "detail";

export default function App() {
  const [view, setView] = useState<View>("loading");
  const [selected, setSelected] = useState<Series | null>(null);
  const [cameFrom, setCameFrom] = useState<View>("airing");
  const [pending, setPending] = useState(0);
  const [refreshing, setRefreshing] = useState(false);

  const refreshBadge = useCallback(async () => {
    try {
      setPending(await pendingCount());
    } catch {
      /* no source yet */
    }
  }, []);

  // Decide first screen: onboarding if no source yet, else pending (+ refresh-on-open).
  // Guarded against React StrictMode's dev-only double-invoke so we don't hit
  // the scraped site twice on startup.
  const startedRef = useRef(false);
  useEffect(() => {
    if (startedRef.current) return;
    startedRef.current = true;
    (async () => {
      try {
        await listAiring(); // throws if no source configured
        setView("pending");
        setRefreshing(true);
        await refresh().catch(() => 0);
        setRefreshing(false);
        await refreshBadge();
      } catch {
        setView("onboarding");
      }
    })();
  }, [refreshBadge]);

  async function doRefresh() {
    setRefreshing(true);
    try {
      await refresh();
    } finally {
      setRefreshing(false);
      await refreshBadge();
      setView("pending");
    }
  }

  function openSeries(s: Series) {
    setCameFrom(view === "detail" ? cameFrom : view);
    setSelected(s);
    setView("detail");
  }

  if (view === "loading") return <div className="empty">Cargando…</div>;

  if (view === "onboarding")
    return (
      <>
        <Onboarding
          onDone={async () => {
            await refreshBadge();
            setView("airing");
          }}
        />
        <ProgressBar />
      </>
    );

  const Tab = ({ id, label }: { id: View; label: string }) => (
    <button
      className={`tab ${view === id ? "active" : ""}`}
      onClick={() => setView(id)}
    >
      {label}
      {id === "pending" && pending > 0 && <span className="badge">{pending}</span>}
    </button>
  );

  return (
    <>
      <div className="topbar">
        <div className="brand">
          <span className="dot" />
          AnimeOnTrack
        </div>
        <div className="tabs">
          <Tab id="pending" label="Pendientes" />
          <Tab id="airing" label="En emisión" />
          <Tab id="library" label="Biblioteca" />
          <Tab id="settings" label="Ajustes" />
        </div>
        <div className="spacer" />
        <button className="btn btn-primary" onClick={doRefresh} disabled={refreshing}>
          {refreshing ? "Actualizando…" : "↻ Actualizar"}
        </button>
      </div>
      <ProgressBar />

      {view === "pending" && <Pending onOpenSeries={openSeries} onChanged={refreshBadge} />}
      {view === "airing" && <AiringGrid onOpenSeries={openSeries} />}
      {view === "library" && <Library onOpenSeries={openSeries} />}
      {view === "settings" && <Settings />}
      {view === "detail" && selected && (
        <SeriesDetail
          series={selected}
          onBack={() => setView(cameFrom)}
          onChanged={refreshBadge}
        />
      )}
    </>
  );
}
