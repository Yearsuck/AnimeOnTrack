import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

// Dev log overlay: shows `scan-log` events emitted by the Rust scraper.
export function LogPanel() {
  const [logs, setLogs] = useState<string[]>([]);
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
    boxRef.current?.scrollTo(0, boxRef.current.scrollHeight);
  }, [logs]);

  if (logs.length === 0) return null;

  return (
    <div
      ref={boxRef}
      style={{
        position: "fixed",
        bottom: 0,
        left: 0,
        right: 0,
        maxHeight: "32vh",
        overflowY: "auto",
        background: "#0b0b0b",
        color: "#5f5",
        fontFamily: "monospace",
        fontSize: 11,
        lineHeight: 1.4,
        padding: "6px 10px",
        borderTop: "2px solid #333",
        zIndex: 9999,
        whiteSpace: "pre-wrap",
      }}
    >
      <div style={{ color: "#888", marginBottom: 4 }}>scraper log ({logs.length})</div>
      {logs.map((l, i) => (
        <div key={i}>{l}</div>
      ))}
    </div>
  );
}
