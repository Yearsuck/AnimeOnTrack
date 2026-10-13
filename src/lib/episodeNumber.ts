// Strict numeric parse, matching Rust's `str::parse::<f64>()` rather than
// JS's `parseFloat` (which accepts trailing garbage — parseFloat("5a") is 5,
// while Rust's parse errors). The SxE branch below needs it: the backend only
// takes that branch when *both* halves parse cleanly as numbers.
const STRICT_NUMBER = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/;
function strictFloat(s: string): number | null {
  const t = s.trim();
  if (!STRICT_NUMBER.test(t)) return null;
  const n = parseFloat(t);
  return Number.isFinite(n) ? n : null;
}

// Mirrors the backend's parse_ep_number (src-tauri/src/db/episodes.rs) — must
// stay in sync with it or the optimistic cascade below will mark a different
// set of episodes than what actually got persisted. Same two branches, in the
// backend's order:
//  1. Season-prefixed "SxE" numbering, which DooPlay sites render for
//     multi-cour series ("1x13"): split on the FIRST 'x'/'X'; when both halves
//     parse as numbers, pack them as `season * 100_000 + episode`, so
//     "1x05" -> 100005 and "2x03" -> 200003. The old leading-digits-only regex
//     collapsed every episode of a season to the bare season digit ("1x05" and
//     "1x12" both -> 1), so marking one episode seen cascaded over whole
//     seasons at a time while the backend only went up to the real target.
//  2. Otherwise leading digits (+ one optional decimal part): "12" -> 12,
//     "12.5" -> 12.5, "0 | Recap" -> 0. Equivalent to the backend's char-walk:
//     a trailing dot is dropped ("12." -> 12) and a second dot ends the number
//     ("1.2.3" -> 1.2).
// Numbers with no leading digit (e.g. "OVA") return null so the optimistic
// update falls back to an exact-string match instead of guessing an order.
export function epNum(number: string): number | null {
  const trimmed = number.trim();
  const xi = trimmed.search(/[xX]/);
  if (xi !== -1) {
    const season = strictFloat(trimmed.slice(0, xi));
    const ep = strictFloat(trimmed.slice(xi + 1));
    if (season !== null && ep !== null) return season * 100_000 + ep;
  }
  const m = trimmed.match(/^\d+(\.\d+)?/);
  return m ? parseFloat(m[0]) : null;
}
