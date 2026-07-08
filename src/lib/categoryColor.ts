// Deterministic name -> color mapping for genre/kind categories. Genre names
// are scraped and open-ended (not known ahead of time), so this can't be a
// static enum lookup. Hash the name to a starting point, then walk the hue
// wheel in golden-angle (137.5°) steps — neighboring hashes land far apart in
// hue, so categories stay visually distinct instead of clustering on a few
// repeated swatches. Fixed saturation/lightness keeps every color equally
// legible against the dark theme.
const HUE_STEP = 137.5;
const SATURATION = 62;
const LIGHTNESS = 58;

export const SIN_GENERO_LABEL = "Sin género";
const SIN_GENERO_COLOR = "#868e96";

function hashString(name: string): number {
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  }
  return hash;
}

export function categoryColor(name: string): string {
  if (name === SIN_GENERO_LABEL) return SIN_GENERO_COLOR;
  const hue = (hashString(name) * HUE_STEP) % 360;
  return `hsl(${hue.toFixed(1)}, ${SATURATION}%, ${LIGHTNESS}%)`;
}
