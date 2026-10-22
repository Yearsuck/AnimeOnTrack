// Pure model behind the 3D genre map in StatsGraph.tsx: which genres become
// hubs, which links exist, where hubs sit, what colour each gets, what a
// selection highlights and what a search finds. No three.js and no DOM here,
// so all of it is unit-tested (graphModel.test.ts).
//
// The rules that make the map readable:
// - A show links ONLY to its own genres (at most `maxGenresPerShow`, in the
//   order the backend gives them). There are no hub-to-hub links and no
//   type (TV/Movie) hubs: those were artificial edges that read as "odd
//   connections". Type is a filter instead.
// - At most `maxHubs` genres get a hub (by number of shows); the long tail
//   folds into one "other genres" hub, so 50+ AniList tags cannot turn the
//   map into a hairball.
// - Hubs are pinned on a sphere and placed so genres that share many shows
//   sit next to each other, which keeps most links short.

import { categoryColor, NO_GENRE_KEY, PALETTE } from "../../lib/categoryColor";
import type { SeriesGraphNode } from "../../types";

export const OTHER_GENRES_KEY = "__other_genres__";
const OTHER_GENRES_COLOR = "#a0a9b4";

export type GraphKind = "TV" | "MOVIE" | "OVA" | "ONA" | "SPECIAL" | "OTHER";
export const GRAPH_KINDS: GraphKind[] = ["TV", "MOVIE", "OVA", "ONA", "SPECIAL", "OTHER"];

/** series.kind is scraped free text ("Pelicula", "4K", null...): fold it into a small fixed set. */
export function normalizeKind(raw: string | null | undefined): GraphKind {
  if (!raw) return "OTHER";
  const upper = raw.trim().toUpperCase();
  if (upper === "PELICULA" || upper === "PELÍCULA") return "MOVIE";
  if (upper === "TV_SHORT") return "TV";
  if (upper === "TV" || upper === "MOVIE" || upper === "OVA" || upper === "ONA" || upper === "SPECIAL") {
    return upper;
  }
  return "OTHER";
}

export interface Hub {
  /** Genre name, or OTHER_GENRES_KEY / NO_GENRE_KEY for the two synthetic buckets. */
  key: string;
  /** Number of shows linked to this hub. */
  count: number;
  /** Episodes watched across those shows. */
  episodes: number;
  color: string;
  /** Genre names folded into this hub (only for OTHER_GENRES_KEY). */
  folded: string[];
}

export interface Show {
  id: number;
  title: string;
  coverUrl: string | null;
  kind: GraphKind;
  /** The show's own genres, as given (for the info card). */
  genres: string[];
  /** Hub keys this show links to, primary first (deduplicated, capped). */
  hubs: string[];
  episodes: number;
  /** Colour of the primary hub. */
  color: string;
}

export interface GenreLink {
  hub: string;
  showId: number;
  /** 0..1, how central the show is within the hub (by episodes watched). */
  weight: number;
}

export interface GenreAtlas {
  hubs: Hub[];
  shows: Show[];
  links: GenreLink[];
}

export interface AtlasOptions {
  maxHubs?: number;
  maxGenresPerShow?: number;
}

export const DEFAULT_MAX_HUBS = 14;
export const DEFAULT_MAX_GENRES_PER_SHOW = 3;

/**
 * Give every hub a distinct colour. The first choice is the genre's usual
 * `categoryColor` (so it matches the Genres tab); a collision with a bigger
 * genre moves it to the next free palette slot.
 */
export function assignHubColors(rankedKeys: string[]): Map<string, string> {
  const out = new Map<string, string>();
  const taken = new Set<string>();
  for (const key of rankedKeys) {
    if (key === NO_GENRE_KEY || key === OTHER_GENRES_KEY) {
      out.set(key, key === NO_GENRE_KEY ? categoryColor(NO_GENRE_KEY) : OTHER_GENRES_COLOR);
      continue;
    }
    const preferred = categoryColor(key);
    let color = preferred;
    if (taken.has(preferred) && taken.size < PALETTE.length) {
      const start = Math.max(0, PALETTE.indexOf(preferred));
      for (let i = 1; i <= PALETTE.length; i++) {
        const candidate = PALETTE[(start + i) % PALETTE.length];
        if (!taken.has(candidate)) {
          color = candidate;
          break;
        }
      }
    }
    taken.add(color);
    out.set(key, color);
  }
  return out;
}

export function buildGenreAtlas(
  series: SeriesGraphNode[],
  episodesOf: (id: number) => number,
  options: AtlasOptions = {}
): GenreAtlas {
  const maxHubs = Math.max(1, options.maxHubs ?? DEFAULT_MAX_HUBS);
  const maxPerShow = Math.max(1, options.maxGenresPerShow ?? DEFAULT_MAX_GENRES_PER_SHOW);

  // Rank genres by how many shows carry them (name breaks ties so the result is stable).
  const genreCounts = new Map<string, number>();
  for (const s of series) {
    for (const g of new Set(s.genres)) genreCounts.set(g, (genreCounts.get(g) ?? 0) + 1);
  }
  const ranked = [...genreCounts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([g]) => g);
  // Folding a single genre into "other" would only rename it, so fold only when it saves a hub.
  const kept = ranked.length > maxHubs + 1 ? ranked.slice(0, maxHubs) : ranked;
  const keptSet = new Set(kept);
  const folded = ranked.filter((g) => !keptSet.has(g));

  const hubStats = new Map<string, { count: number; episodes: number }>();
  const showsDraft: Omit<Show, "color">[] = [];
  for (const s of series) {
    const eps = Math.max(0, episodesOf(s.id) || 0);
    const hubs: string[] = [];
    for (const g of s.genres) {
      const key = keptSet.has(g) ? g : OTHER_GENRES_KEY;
      if (!hubs.includes(key)) hubs.push(key);
      if (hubs.length >= maxPerShow) break;
    }
    if (hubs.length === 0) hubs.push(NO_GENRE_KEY);
    for (const h of hubs) {
      const st = hubStats.get(h) ?? { count: 0, episodes: 0 };
      st.count += 1;
      st.episodes += eps;
      hubStats.set(h, st);
    }
    showsDraft.push({
      id: s.id,
      title: s.title,
      coverUrl: s.cover_url,
      kind: normalizeKind(s.kind),
      genres: s.genres,
      hubs,
      episodes: eps,
    });
  }

  const hubKeys = [...hubStats.keys()].sort((a, b) => {
    const synthetic = (k: string) => (k === OTHER_GENRES_KEY || k === NO_GENRE_KEY ? 1 : 0);
    return (
      synthetic(a) - synthetic(b) ||
      hubStats.get(b)!.count - hubStats.get(a)!.count ||
      a.localeCompare(b)
    );
  });
  const colors = assignHubColors(hubKeys);
  const hubs: Hub[] = hubKeys.map((key) => ({
    key,
    count: hubStats.get(key)!.count,
    episodes: hubStats.get(key)!.episodes,
    color: colors.get(key)!,
    folded: key === OTHER_GENRES_KEY ? folded.filter((g) => showsDraft.some((s) => s.genres.includes(g))) : [],
  }));

  const shows: Show[] = showsDraft.map((s) => ({ ...s, color: colors.get(s.hubs[0])! }));

  const maxEpsInHub = new Map<string, number>();
  for (const s of shows) {
    for (const h of s.hubs) maxEpsInHub.set(h, Math.max(maxEpsInHub.get(h) ?? 0, s.episodes));
  }
  const links: GenreLink[] = [];
  for (const s of shows) {
    for (const h of s.hubs) {
      const max = maxEpsInHub.get(h) ?? 0;
      links.push({ hub: h, showId: s.id, weight: max > 0 ? s.episodes / max : 0 });
    }
  }
  return { hubs, shows, links };
}

export interface Vec3 {
  x: number;
  y: number;
  z: number;
}

/** `n` evenly spread points on a sphere (Fibonacci lattice), first point at the top. */
export function fibonacciSphere(n: number, radius: number): Vec3[] {
  if (n <= 0) return [];
  if (n === 1) return [{ x: 0, y: 0, z: 0 }];
  const golden = Math.PI * (3 - Math.sqrt(5));
  const out: Vec3[] = [];
  for (let i = 0; i < n; i++) {
    const y = 1 - (i / (n - 1)) * 2;
    const r = Math.sqrt(Math.max(0, 1 - y * y));
    const theta = golden * i;
    out.push({ x: Math.cos(theta) * r * radius, y: y * radius, z: Math.sin(theta) * r * radius });
  }
  return out;
}

/** Sphere radius that leaves roughly `spacing` world units between neighbouring hubs. */
export function hubSphereRadius(hubCount: number, spacing = 320, min = 220): number {
  return Math.max(min, spacing * Math.sqrt(hubCount / (4 * Math.PI)));
}

/** How many shows each pair of hubs shares (key "a|b" with a < b). */
export function hubCooccurrence(shows: Show[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const s of shows) {
    for (let i = 0; i < s.hubs.length; i++) {
      for (let j = i + 1; j < s.hubs.length; j++) {
        const [a, b] = [s.hubs[i], s.hubs[j]].sort();
        const k = `${a}|${b}`;
        out.set(k, (out.get(k) ?? 0) + 1);
      }
    }
  }
  return out;
}

/**
 * Pin hubs on a sphere. Hubs are placed biggest first; each takes the free
 * lattice point closest (weighted by shared shows) to the hubs it shares shows
 * with, so related genres end up neighbours and their links stay short.
 */
export function placeHubs(hubs: Hub[], shows: Show[], radius = hubSphereRadius(hubs.length)): Map<string, Vec3> {
  const points = fibonacciSphere(hubs.length, radius);
  const cooc = hubCooccurrence(shows);
  const shared = (a: string, b: string) => cooc.get(a < b ? `${a}|${b}` : `${b}|${a}`) ?? 0;
  const free = points.map((_, i) => i);
  const placed = new Map<string, Vec3>();
  const order = [...hubs].sort((a, b) => b.count - a.count || a.key.localeCompare(b.key));
  for (const hub of order) {
    let bestIdx = 0;
    let bestCost = Infinity;
    for (let fi = 0; fi < free.length; fi++) {
      const p = points[free[fi]];
      let cost = 0;
      for (const [other, q] of placed) {
        const w = shared(hub.key, other);
        if (w > 0) cost += w * Math.hypot(p.x - q.x, p.y - q.y, p.z - q.z);
      }
      // Strictly lower only: ties keep the earliest lattice point, so the result is deterministic.
      if (cost < bestCost) {
        bestCost = cost;
        bestIdx = fi;
      }
    }
    placed.set(hub.key, points[free[bestIdx]]);
    free.splice(bestIdx, 1);
  }
  return placed;
}

/** Map `value / max` onto [min, max] with a square-root curve (area-like perception). */
export function scaleSqrt(value: number, maxValue: number, min: number, max: number): number {
  if (maxValue <= 0) return min;
  const t = Math.sqrt(Math.min(1, Math.max(0, value / maxValue)));
  return min + (max - min) * t;
}

export const hubNodeId = (key: string) => `genre:${key}`;
export const showNodeId = (id: number) => `series:${id}`;
export const linkKey = (hub: string, showId: number) => `${hub}|${showId}`;

export type Selection = { type: "genre"; key: string } | { type: "series"; id: number } | null;

/** Node ids and link keys a selection lights up (null = nothing selected, nothing dimmed). */
export function neighbourhood(
  atlas: GenreAtlas,
  selection: Selection
): { nodes: Set<string>; links: Set<string> } | null {
  if (!selection) return null;
  const nodes = new Set<string>();
  const links = new Set<string>();
  if (selection.type === "genre") {
    nodes.add(hubNodeId(selection.key));
    for (const l of atlas.links) {
      if (l.hub === selection.key) {
        nodes.add(showNodeId(l.showId));
        links.add(linkKey(l.hub, l.showId));
      }
    }
  } else {
    const show = atlas.shows.find((s) => s.id === selection.id);
    if (!show) return null;
    nodes.add(showNodeId(show.id));
    for (const h of show.hubs) {
      nodes.add(hubNodeId(h));
      links.add(linkKey(h, show.id));
    }
  }
  return { nodes, links };
}

export type KindFilter = GraphKind | "all";

export function showMatchesKind(show: Show, filter: KindFilter): boolean {
  return filter === "all" || show.kind === filter;
}

/** Shows per hub under a type filter (for the legend). */
export function hubCounts(atlas: GenreAtlas, filter: KindFilter): Map<string, number> {
  const out = new Map<string, number>(atlas.hubs.map((h) => [h.key, 0]));
  for (const s of atlas.shows) {
    if (!showMatchesKind(s, filter)) continue;
    for (const h of s.hubs) out.set(h, (out.get(h) ?? 0) + 1);
  }
  return out;
}

/** Types actually present, in display order. */
export function presentKinds(atlas: GenreAtlas): GraphKind[] {
  const present = new Set(atlas.shows.map((s) => s.kind));
  return GRAPH_KINDS.filter((k) => present.has(k));
}

function fold(text: string): string {
  return text.normalize("NFD").replace(/[̀-ͯ]/g, "").toLowerCase().trim();
}

/** Title search, accent- and case-insensitive; prefix matches first, then by episodes watched. */
export function searchShows(shows: Show[], query: string, limit = 6): Show[] {
  const q = fold(query);
  if (!q) return [];
  return shows
    .map((s) => ({ s, at: fold(s.title).indexOf(q) }))
    .filter((r) => r.at >= 0)
    .sort((a, b) => (a.at === 0 ? 0 : 1) - (b.at === 0 ? 0 : 1) || b.s.episodes - a.s.episodes || a.s.title.localeCompare(b.s.title))
    .slice(0, limit)
    .map((r) => r.s);
}

/** Shows of a hub, most watched first. */
export function topShowsOf(atlas: GenreAtlas, hubKey: string, filter: KindFilter, limit = 5): Show[] {
  return atlas.shows
    .filter((s) => s.hubs.includes(hubKey) && showMatchesKind(s, filter))
    .sort((a, b) => b.episodes - a.episodes || a.title.localeCompare(b.title))
    .slice(0, limit);
}

/** The graph library renders node tooltips as HTML; scraped titles must not be able to inject markup. */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/**
 * Camera distance at which a sphere of `radius` just fits the free part of the
 * viewport (`freeWidth` x `height` px), for a vertical field of view `vfovDeg`.
 */
export function fitDistance(radius: number, vfovDeg: number, freeWidth: number, height: number, pad = 1.04): number {
  const halfV = (vfovDeg * Math.PI) / 360;
  if (radius <= 0 || height <= 0 || freeWidth <= 0) return radius * 3 || 300;
  const halfH = Math.atan(Math.tan(halfV) * (freeWidth / height));
  return (radius * pad) / Math.sin(Math.min(halfV, halfH));
}

export interface LabelStyle {
  /** Pill height in px. */
  px: number;
  fontPx: number;
  showCount: boolean;
}

/** Big genres get a full label; small ones a smaller pill, without the count below 3 shows, to cut overlap. */
export function labelStyle(count: number, maxCount: number): LabelStyle {
  const big = maxCount > 0 && count >= maxCount * 0.25;
  return big ? { px: 26, fontPx: 13, showCount: true } : { px: 21, fontPx: 11, showCount: count >= 3 };
}

export interface ScreenLabel {
  key: string;
  /** Anchor in screen px (bottom-centre of the pill), y grows downwards. */
  x: number;
  y: number;
  w: number;
  h: number;
  /** Higher keeps its place; lower ones move. */
  priority: number;
}

/**
 * Screen-space declutter: labels are placed by priority (biggest genre
 * first, never moved) and each later label that would overlap an already
 * placed one is lifted just above it. Returns the lift in px per key.
 */
export function declutterLabels(labels: ScreenLabel[], gap = 2, maxSteps = 12): Map<string, number> {
  const placed: { l: number; r: number; t: number; b: number }[] = [];
  const out = new Map<string, number>();
  const order = [...labels].sort((a, b) => b.priority - a.priority || a.key.localeCompare(b.key));
  for (const lab of order) {
    const l = lab.x - lab.w / 2;
    const r = lab.x + lab.w / 2;
    let dy = 0;
    for (let step = 0; step < maxSteps; step++) {
      const b = lab.y - dy;
      const t = b - lab.h;
      const hit = placed.find((p) => l < p.r + gap && r > p.l - gap && t < p.b + gap && b > p.t - gap);
      if (!hit) break;
      dy = lab.y - hit.t + gap;
    }
    placed.push({ l, r, t: lab.y - dy - lab.h, b: lab.y - dy });
    out.set(lab.key, dy);
  }
  return out;
}

/** `#rgb` / `#rrggbb` ->`rgba(r, g, b, a)`; anything else is returned unchanged. */
export function withAlpha(color: string, alpha: number): string {
  let hex = color.trim().replace(/^#/, "");
  if (/^[0-9a-f]{3}$/i.test(hex)) hex = hex.split("").map((c) => c + c).join("");
  if (!/^[0-9a-f]{6}$/i.test(hex)) return color;
  const n = parseInt(hex, 16);
  return `rgba(${(n >> 16) & 0xff}, ${(n >> 8) & 0xff}, ${n & 0xff}, ${alpha})`;
}

/**
 * Sprite scale that keeps a label `px` pixels tall on screen whatever the zoom
 * (three.js sprites with sizeAttenuation=false are sized in view units at
 * distance 1).
 */
export function screenSpriteScale(px: number, fovDeg: number, viewportHeight: number): number {
  if (viewportHeight <= 0) return 0;
  return (px * 2 * Math.tan((fovDeg * Math.PI) / 360)) / viewportHeight;
}
