import { useEffect, useState } from "react";
import { Onboarding } from "./views/Onboarding";
import { AiringGrid } from "./views/AiringGrid";
import { Pending } from "./views/Pending";
import { Settings } from "./views/Settings";
import { LogPanel } from "./views/LogPanel";
import { listAiring, refresh } from "./api";

type View = "loading" | "onboarding" | "pending" | "airing" | "settings";

export default function App() {
  const [view, setView] = useState<View>("loading");

  // Decide first screen: onboarding if no source yet, else pending.
  useEffect(() => {
    (async () => {
      try {
        await listAiring(); // throws if no source configured
        await refresh().catch(() => 0); // refresh-on-open, best effort
        setView("pending");
      } catch {
        setView("onboarding");
      }
    })();
  }, []);

  return (
    <>
      {view === "loading" && <div style={{ padding: 16 }}>Loading…</div>}
      {view === "onboarding" && <Onboarding onDone={() => setView("airing")} />}
      {(view === "pending" || view === "airing" || view === "settings") && (
        <div>
          <nav style={{ display: "flex", gap: 8, padding: 8, borderBottom: "1px solid #ccc" }}>
            <button onClick={() => setView("pending")}>Pending</button>
            <button onClick={() => setView("airing")}>Airing</button>
            <button onClick={() => setView("settings")}>Settings</button>
            <button onClick={async () => { await refresh(); setView("pending"); }}>Refresh</button>
          </nav>
          {view === "pending" && <Pending />}
          {view === "airing" && <AiringGrid />}
          {view === "settings" && <Settings />}
        </div>
      )}
      <LogPanel />
    </>
  );
}
