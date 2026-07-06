import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

// Dev log overlay: shows `scan-log` events emitted by the Rust scraper.
export function LogPanel() {
  const [logs, setLogs] = useState<string[]>([]);
  const [collapsed, setCollapsed] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const un = listen<string>("scan-log", (e) => {
      setLogs((l) => [...l.slice(-300), e.payload]);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!collapsed) boxRef.current?.scrollTo(0, boxRef.current.scrollHeight);
  }, [logs, collapsed]);

  if (logs.length === 0) return null;

  return (
    <div className="logpanel" style={{ maxHeight: collapsed ? 32 : "30vh" }}>
      <div className="log-head">
        <span>registro del scraper ({logs.length})</span>
        <span style={{ display: "flex", gap: 10 }}>
          <span style={{ cursor: "pointer" }} onClick={() => setLogs([])}>
            limpiar
          </span>
          <span style={{ cursor: "pointer" }} onClick={() => setCollapsed((v) => !v)}>
            {collapsed ? "▲ mostrar" : "▼ ocultar"}
          </span>
        </span>
      </div>
      {!collapsed && (
        <div ref={boxRef} style={{ overflowY: "auto", maxHeight: "26vh" }}>
          {logs.map((l, i) => (
            <div className="log-line" key={i}>
              {l}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
