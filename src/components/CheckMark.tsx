/** Seen-toggle tick: the stroke draws in via CSS (`.check.on .check-mark path`). */
export function CheckMark() {
  return (
    <svg className="check-mark" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" focusable="false">
      <path d="M3.5 8.5 6.6 11.5 12.5 4.8" pathLength="1" />
    </svg>
  );
}
