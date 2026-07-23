// Deterministic name -> color mapping for genre/kind categories. Genre names
// are scraped and open-ended (not known ahead of time), so this can't be a
// static enum lookup — instead a stable hash picks a slot in a hand-picked
// palette. A previous version computed colors on the fly by walking the HSL
// hue wheel at a fixed saturation/lightness; the result read as washed-out
// and samey. This is a curated set of vivid, high-contrast swatches chosen
// to read well against the app's dark navy surfaces, cycled by hash instead
// of computed at runtime.
const PALETTE = [
  "#FF6B6B", // red
  "#4ECDC4", // teal
  "#FFA94D", // orange
  "#748FFC", // indigo
  "#69DB7C", // green
  "#F783AC", // pink
  "#FFD43B", // yellow
  "#9775FA", // purple
  "#3BC9DB", // cyan
  "#FF922B", // deep orange
  "#51CF66", // emerald
  "#E599F7", // orchid
  "#4DABF7", // blue
  "#FF8787", // salmon
  "#63E6BE", // mint
  "#FFC078", // gold
];

// Stable identity for the "this series has no genre" bucket, kept separate
// from the text shown for it. The label used to be a hardcoded Spanish
// string that doubled as the lookup key, so the English UI still read "Sin
// género"; the visible text now comes from `t("stats.noGenre")` while this
// key stays constant so the grey swatch below keeps matching it regardless of
// the active language.
export const NO_GENRE_KEY = "__no_genre__";
const NO_GENRE_COLOR = "#868e96";

function hashString(name: string): number {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  }
  return hash;
}

export function categoryColor(name: string): string {
  if (name === NO_GENRE_KEY) return NO_GENRE_COLOR;
  return PALETTE[hashString(name) % PALETTE.length];
}
