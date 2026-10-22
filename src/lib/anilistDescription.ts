// AniList descriptions arrive as light markup even with `asHtml: false`:
// `<br>`, `<i>`/`<b>`, HTML entities, `__bold__` and `~!spoiler!~` blocks.
// They are turned into plain-text paragraphs here and rendered as text nodes —
// never through dangerouslySetInnerHTML — so nothing in them can execute or
// inject markup. Spoiler text is kept but flagged so the view can hide it.

export interface DescSegment {
  text: string;
  spoiler: boolean;
}
export type DescParagraph = DescSegment[];

const NAMED_ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  mdash: "—",
  ndash: "–",
  hellip: "…",
  rsquo: "’",
  lsquo: "‘",
  rdquo: "”",
  ldquo: "“",
};

function decodeEntities(s: string): string {
  return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, body: string) => {
    if (body[0] === "#") {
      const code = body[1] === "x" || body[1] === "X" ? parseInt(body.slice(2), 16) : parseInt(body.slice(1), 10);
      return Number.isFinite(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    }
    return NAMED_ENTITIES[body.toLowerCase()] ?? whole;
  });
}

/** Plain-text paragraphs of an AniList description (or any synopsis). */
export function parseDescription(raw: string | null | undefined): DescParagraph[] {
  if (!raw) return [];
  const text = decodeEntities(
    raw
      .replace(/\r\n?/g, "\n")
      .replace(/<br\s*\/?>/gi, "\n")
      .replace(/<\/p\s*>/gi, "\n")
      .replace(/<[^>]*>/g, "")
  )
    .replace(/__|\*\*|~~~/g, "");

  const paragraphs: DescParagraph[] = [];
  let current: DescParagraph = [];
  const flush = () => {
    const cleaned = current
      .map((s) => ({ ...s, text: s.text.replace(/[ \t ]+/g, " ") }))
      .filter((s) => s.text.trim() !== "");
    if (cleaned.length > 0) {
      cleaned[0] = { ...cleaned[0], text: cleaned[0].text.trimStart() };
      const last = cleaned.length - 1;
      cleaned[last] = { ...cleaned[last], text: cleaned[last].text.trimEnd() };
      paragraphs.push(cleaned);
    }
    current = [];
  };

  let spoiler = false;
  for (const part of text.split(/(~!|!~)/)) {
    if (part === "~!") {
      spoiler = true;
      continue;
    }
    if (part === "!~") {
      spoiler = false;
      continue;
    }
    const lines = part.split("\n");
    lines.forEach((line, i) => {
      if (i > 0) flush();
      if (line !== "") current.push({ text: line, spoiler });
    });
  }
  flush();
  return paragraphs;
}

export function hasSpoilers(paragraphs: DescParagraph[]): boolean {
  return paragraphs.some((p) => p.some((s) => s.spoiler));
}

export type SynopsisSource = "site" | "anilist";

/** Which text to show first: the site's synopsis is Spanish, AniList's is
 *  English, so Spanish/Catalan readers get the site one first and English
 *  readers AniList's, each falling back to whichever exists. */
export function initialSynopsisSource(
  lang: string,
  hasSite: boolean,
  hasAnilist: boolean
): SynopsisSource | null {
  const preferSite = lang !== "en";
  if (preferSite) return hasSite ? "site" : hasAnilist ? "anilist" : null;
  return hasAnilist ? "anilist" : hasSite ? "site" : null;
}
