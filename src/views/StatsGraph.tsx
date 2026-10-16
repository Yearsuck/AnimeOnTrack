import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ForceGraph3D, { type ForceGraphMethods, type NodeObject } from "react-force-graph-3d";
import * as THREE from "three";
import { useT } from "../i18n";
import { useTheme } from "../theme";
import { categoryColor, NO_GENRE_KEY } from "../lib/categoryColor";
import { resolveCoverUrl } from "../lib/posterFallback";
import { useOutsideClick } from "../lib/useOutsideClick";
import { listLibrary } from "../api";
import type { SeriesGraphNode } from "../types";
import "../styles/statsgraph.css";

// --- WEIGHT & VISUAL SCALING CONSTANTS ---
// Connection line colors per theme
const LINK_COLOR_DARK = "rgba(255, 255, 255, 0.25)";
const LINK_COLOR_LIGHT = "rgba(23, 34, 46, 0.35)";
// Dimmed link colors when a node is selected (neighbors highlighted)
const LINK_DIM_DARK = "rgba(255, 255, 255, 0.04)";
const LINK_DIM_LIGHT = "rgba(23, 34, 46, 0.05)";
// Highlighted link colors for connections to selected node
const LINK_HIGHLIGHT_DARK = "rgba(255, 255, 255, 0.75)";
const LINK_HIGHLIGHT_LIGHT = "rgba(23, 34, 46, 0.85)";

// Genre hub node sphere radius bounds (min/max ratio >= 4x: 36 / 8 = 4.5x)
const HUB_MIN_R = 8;
const HUB_MAX_R = 36;

// Series node sprite scale bounds (min/max ratio >= 4x: 52 / 12 = 4.33x)
const SERIES_MIN_SIZE = 12;
const SERIES_MAX_SIZE = 52;

// Exaggerated curve exponent: power law radius = base + k * Math.pow(norm, 0.6)
const WEIGHT_CURVE_EXPONENT = 0.6;

// Link distance: heavier series within a genre are closer to their genre hub
const LINK_MIN_DIST = 75; // closest distance for highest-weight series
const LINK_MAX_DIST = 260; // furthest distance for lowest-weight series
const HUB_BACKBONE_LINK_DIST = 320; // loose distance between top genre hubs

// Link width: heavier series within a genre have thicker connection lines
const LINK_MIN_WIDTH = 0.8;
const LINK_MAX_WIDTH = 4.2;
const HUB_BACKBONE_LINK_WIDTH = 0.6;

// Force layout constants
const CHARGE_STRENGTH = -750;
const COOLDOWN_TICKS = 200;
const MIN_HEIGHT = 500;
const BOTTOM_MARGIN = 24;

interface Vec3 {
  x: number;
  y: number;
  z: number;
}

function disposeNodeObject(obj: THREE.Object3D): void {
  const anyObj = obj as unknown as { material?: unknown; geometry?: { dispose?: () => void } };
  const materials = Array.isArray(anyObj.material) ? anyObj.material : [anyObj.material];
  for (const m of materials) {
    const mat = m as { map?: { dispose?: () => void }; dispose?: () => void } | undefined;
    mat?.map?.dispose?.();
    mat?.dispose?.();
  }
  anyObj.geometry?.dispose?.();
}

type GNodeKind = "genreHub" | "kindHub" | "series";

interface GNode {
  id: string;
  kind: GNodeKind;
  label: string;
  count?: number;
  episodes?: number;
  genres?: string[];
  color?: string;
  coverUrl?: string | null;
  seriesId?: number;
  size?: number;
  radius?: number;
}

interface GLink {
  source: string | NodeObject<GNode>;
  target: string | NodeObject<GNode>;
  width?: number;
  distance?: number;
  isHubLink?: boolean;
}

function buildGraphData(
  seriesList: SeriesGraphNode[],
  noGenreLabel: string,
  watchedMap?: Map<number, number> | null
): { nodes: GNode[]; links: GLink[] } {
  const nodes: GNode[] = [];
  const links: GLink[] = [];

  const genreCounts = new Map<string, number>();
  const genreEpisodes = new Map<string, number>();
  const genreSeriesMaxEpisodes = new Map<string, number>();
  const kindCounts = new Map<string, number>();
  let sinGeneroCount = 0;
  let sinGeneroEpisodes = 0;
  let sinGeneroMaxEpisodes = 0;

  const getEpisodes = (s: SeriesGraphNode): number => {
    return watchedMap?.get(s.id) ?? 0;
  };

  const maxSeriesEpisodes = Math.max(1, ...seriesList.map((s) => getEpisodes(s)));

  for (const s of seriesList) {
    const eps = getEpisodes(s);
    if (s.genres.length === 0) {
      sinGeneroCount++;
      sinGeneroEpisodes += eps;
      if (eps > sinGeneroMaxEpisodes) sinGeneroMaxEpisodes = eps;
    } else {
      for (const g of s.genres) {
        genreCounts.set(g, (genreCounts.get(g) ?? 0) + 1);
        genreEpisodes.set(g, (genreEpisodes.get(g) ?? 0) + eps);
        const prevMax = genreSeriesMaxEpisodes.get(g) ?? 0;
        if (eps > prevMax) genreSeriesMaxEpisodes.set(g, eps);
      }
    }
    if (s.kind) {
      kindCounts.set(s.kind, (kindCounts.get(s.kind) ?? 0) + 1);
    }
  }

  const allGenreEpisodes = Array.from(genreEpisodes.values());
  const maxGenreEpisodes = Math.max(1, ...allGenreEpisodes, sinGeneroEpisodes);
  const maxGenreCount = Math.max(1, ...Array.from(genreCounts.values()), sinGeneroCount);
  const hasAnyWatched = allGenreEpisodes.some((e) => e > 0) || sinGeneroEpisodes > 0;

  // Build top-level genre hubs
  for (const [g, count] of genreCounts) {
    const eps = genreEpisodes.get(g) ?? 0;
    const norm = hasAnyWatched ? eps / maxGenreEpisodes : count / maxGenreCount;
    const radius =
      HUB_MIN_R + (HUB_MAX_R - HUB_MIN_R) * Math.pow(Math.min(1, Math.max(0, norm)), WEIGHT_CURVE_EXPONENT);
    nodes.push({
      id: `genre:${g}`,
      kind: "genreHub",
      label: g,
      count,
      episodes: eps,
      color: categoryColor(g),
      radius,
    });
  }

  if (sinGeneroCount > 0) {
    const norm = hasAnyWatched ? sinGeneroEpisodes / maxGenreEpisodes : sinGeneroCount / maxGenreCount;
    const radius =
      HUB_MIN_R + (HUB_MAX_R - HUB_MIN_R) * Math.pow(Math.min(1, Math.max(0, norm)), WEIGHT_CURVE_EXPONENT);
    nodes.push({
      id: `genre:${NO_GENRE_KEY}`,
      kind: "genreHub",
      label: noGenreLabel,
      count: sinGeneroCount,
      episodes: sinGeneroEpisodes,
      color: categoryColor(NO_GENRE_KEY),
      radius,
    });
  }

  for (const [k, count] of kindCounts) {
    nodes.push({
      id: `kind:${k}`,
      kind: "kindHub",
      label: k,
      count,
      color: categoryColor(k),
      radius: HUB_MIN_R,
    });
  }

  // Loosely connect the largest genre hubs to prevent the galaxy layout from flying apart
  const genreHubNodes = nodes
    .filter((n) => n.kind === "genreHub")
    .sort((a, b) => (b.episodes ?? b.count ?? 0) - (a.episodes ?? a.count ?? 0));
  const topHubCount = Math.min(genreHubNodes.length, 6);
  if (topHubCount >= 2) {
    for (let i = 0; i < topHubCount; i++) {
      const nextIndex = (i + 1) % topHubCount;
      links.push({
        source: genreHubNodes[i].id,
        target: genreHubNodes[nextIndex].id,
        width: HUB_BACKBONE_LINK_WIDTH,
        distance: HUB_BACKBONE_LINK_DIST,
        isHubLink: true,
      });
    }
  }

  // Build series nodes with exaggerated sprite scaling and links to genre hubs
  for (const s of seriesList) {
    const id = `series:${s.id}`;
    const eps = getEpisodes(s);
    const norm = maxSeriesEpisodes > 0 ? eps / maxSeriesEpisodes : 0;
    const size =
      SERIES_MIN_SIZE +
      (SERIES_MAX_SIZE - SERIES_MIN_SIZE) * Math.pow(Math.min(1, Math.max(0, norm)), WEIGHT_CURVE_EXPONENT);
    const fallbackColor = s.genres[0] ? categoryColor(s.genres[0]) : categoryColor(NO_GENRE_KEY);

    nodes.push({
      id,
      kind: "series",
      label: s.title,
      color: fallbackColor,
      coverUrl: s.cover_url,
      episodes: eps,
      genres: s.genres,
      seriesId: s.id,
      size,
    });

    const targetGenres = s.genres.length === 0 ? [NO_GENRE_KEY] : s.genres;
    for (const g of targetGenres) {
      const maxInGenre = g === NO_GENRE_KEY ? sinGeneroMaxEpisodes : genreSeriesMaxEpisodes.get(g) ?? 0;
      const weightInGenre = maxInGenre > 0 ? eps / maxInGenre : 0.5;
      const curvedWeight = Math.pow(Math.min(1, Math.max(0, weightInGenre)), WEIGHT_CURVE_EXPONENT);
      const linkDist = LINK_MAX_DIST - (LINK_MAX_DIST - LINK_MIN_DIST) * curvedWeight;
      const linkW = LINK_MIN_WIDTH + (LINK_MAX_WIDTH - LINK_MIN_WIDTH) * curvedWeight;
      links.push({
        source: `genre:${g}`,
        target: id,
        width: linkW,
        distance: linkDist,
      });
    }

    if (s.kind) {
      links.push({
        source: `kind:${s.kind}`,
        target: id,
        width: LINK_MIN_WIDTH,
        distance: LINK_MAX_DIST,
      });
    }
  }

  return { nodes, links };
}

function fallbackCircleCanvas(color: string, strokeColor: string): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 64;
  const ctx = canvas.getContext("2d")!;
  ctx.beginPath();
  ctx.arc(32, 32, 29, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  ctx.lineWidth = 2;
  ctx.strokeStyle = strokeColor;
  ctx.stroke();
  return canvas;
}

function shadeHex(hex: string, amt: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return hex;
  const n = parseInt(m[1], 16);
  const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  const r = clamp(((n >> 16) & 0xff) + 255 * amt);
  const g = clamp(((n >> 8) & 0xff) + 255 * amt);
  const b = clamp((n & 0xff) + 255 * amt);
  return `rgb(${r}, ${g}, ${b})`;
}

function mixHex(a: string, b: string, t: number): string {
  const ma = /^#?([0-9a-f]{6})$/i.exec(a.trim());
  const mb = /^#?([0-9a-f]{6})$/i.exec(b.trim());
  if (!ma || !mb) return a;
  const na = parseInt(ma[1], 16);
  const nb = parseInt(mb[1], 16);
  const lerp = (x: number, y: number) => Math.max(0, Math.min(255, Math.round(x + (y - x) * t)));
  const r = lerp((na >> 16) & 0xff, (nb >> 16) & 0xff);
  const g = lerp((na >> 8) & 0xff, (nb >> 8) & 0xff);
  const bch = lerp(na & 0xff, nb & 0xff);
  return `rgb(${r}, ${g}, ${bch})`;
}

const ICE_LIGHT_TINT = "#eaf9ff";
const ICE_DARK_TINT = "#4f7690";

function hash2(x: number, y: number): number {
  const s = Math.sin(x * 127.1 + y * 311.7) * 43758.5453123;
  return s - Math.floor(s);
}

function valueNoise(x: number, y: number): number {
  const xi = Math.floor(x);
  const yi = Math.floor(y);
  const xf = x - xi;
  const yf = y - yi;
  const a = hash2(xi, yi);
  const b = hash2(xi + 1, yi);
  const c = hash2(xi, yi + 1);
  const d = hash2(xi + 1, yi + 1);
  const u = xf * xf * (3 - 2 * xf);
  const v = yf * yf * (3 - 2 * yf);
  return a * (1 - u) * (1 - v) + b * u * (1 - v) + c * (1 - u) * v + d * u * v;
}

function fbm(x: number, y: number, octaves: number): number {
  let total = 0;
  let amp = 0.5;
  let freq = 1;
  let maxAmp = 0;
  for (let i = 0; i < octaves; i++) {
    total += valueNoise(x * freq, y * freq) * amp;
    maxAmp += amp;
    amp *= 0.5;
    freq *= 2;
  }
  return total / maxAmp;
}

function planetTexture(color: string): THREE.CanvasTexture {
  const size = 256;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const cx = size / 2;
  const cy = size / 2;
  const r = size / 2 - 2;

  let seed = 0;
  for (let i = 0; i < color.length; i++) seed += color.charCodeAt(i);
  const nx = seed * 0.13;
  const ny = seed * 0.071;
  const variant = Math.floor(seed) % 3; // 0 rocky, 1 gas giant, 2 ice
  const isGasGiant = variant === 1;
  const isIce = variant === 2;

  const lightC = isIce ? mixHex(shadeHex(color, 0.4), ICE_LIGHT_TINT, 0.55) : shadeHex(color, 0.4);
  const darkC = isIce ? mixHex(shadeHex(color, -0.35), ICE_DARK_TINT, 0.45) : shadeHex(color, -0.35);
  const limbC = isIce ? mixHex(shadeHex(color, -0.65), ICE_DARK_TINT, 0.35) : shadeHex(color, -0.65);
  const baseMid = isIce ? mixHex(color, ICE_LIGHT_TINT, 0.35) : color;

  ctx.save();
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.clip();

  const base = ctx.createRadialGradient(cx - r * 0.35, cy - r * 0.38, r * 0.05, cx, cy, r * 1.05);
  base.addColorStop(0, lightC);
  base.addColorStop(0.32, baseMid);
  base.addColorStop(1, darkC);
  ctx.fillStyle = base;
  ctx.fillRect(0, 0, size, size);

  const bandCount = isGasGiant ? 5 : 7;
  const bandWobbleScale = isGasGiant ? 0.025 : 0.06;
  const bandToneBase = isGasGiant ? 0.12 : 0.05;
  const bandToneNoise = isGasGiant ? 0.06 : 0.05;
  const bandAlpha = isGasGiant ? 0.5 : 0.35;
  for (let i = 0; i < bandCount; i++) {
    const bandY = (i / bandCount) * size;
    const wobble = (fbm(i * 0.7 + nx, ny, 2) - 0.5) * size * bandWobbleScale;
    const bandHeight = size / bandCount;
    const toneAmt = (i % 2 === 0 ? 1 : -1) * (bandToneBase + bandToneNoise * fbm(i * 1.3 + nx, ny + 5, 2));
    ctx.globalAlpha = bandAlpha;
    ctx.fillStyle = isIce ? mixHex(shadeHex(color, toneAmt), ICE_LIGHT_TINT, 0.3) : shadeHex(color, toneAmt);
    ctx.fillRect(0, bandY + wobble, size, bandHeight);
  }
  ctx.globalAlpha = 1;

  const blotchCell = isGasGiant ? 8 : 4;
  const blotchThreshold = isGasGiant ? 0.78 : isIce ? 0.64 : 0.56;
  const blotchAlphaScale = isGasGiant ? 0.3 : 0.9;
  const blotchMaxAlpha = isGasGiant ? 0.25 : 0.45;
  for (let y = 0; y < size; y += blotchCell) {
    for (let x = 0; x < size; x += blotchCell) {
      const n = fbm(x * 0.045 + nx, y * 0.045 + ny, 3);
      if (n > blotchThreshold) {
        const amt = (n - blotchThreshold) * blotchAlphaScale;
        ctx.globalAlpha = Math.min(blotchMaxAlpha, amt);
        ctx.fillStyle = isIce ? mixHex(lightC, "#ffffff", 0.5) : n > blotchThreshold + 0.12 ? lightC : darkC;
        ctx.fillRect(x, y, blotchCell, blotchCell);
      }
    }
  }
  ctx.globalAlpha = 1;

  if (variant === 0) {
    const craterCount = 4 + Math.floor(hash2(nx, ny) * 5);
    for (let i = 0; i < craterCount; i++) {
      const ang = hash2(nx + i * 2.31, ny - i * 1.17) * Math.PI * 2;
      const dist = Math.sqrt(hash2(nx - i * 3.73, ny + i * 4.19)) * r * 0.82;
      const px = cx + Math.cos(ang) * dist;
      const py = cy + Math.sin(ang) * dist;
      const craterR = r * (0.04 + hash2(nx + i * 5.07, ny + i * 2.63) * 0.05);

      ctx.beginPath();
      ctx.arc(px, py, craterR, 0, Math.PI * 2);
      ctx.fillStyle = shadeHex(color, -0.22);
      ctx.fill();

      ctx.lineWidth = Math.max(1, craterR * 0.22);
      ctx.strokeStyle = shadeHex(color, -0.42);
      ctx.stroke();
    }
  }

  const vignette = ctx.createRadialGradient(cx, cy, r * 0.55, cx, cy, r);
  vignette.addColorStop(0, "rgba(0, 0, 0, 0)");
  const limbRgb = /^rgb\(([^)]+)\)$/.exec(limbC)?.[1] ?? "0, 0, 0";
  vignette.addColorStop(1, `rgba(${limbRgb}, 0.55)`);
  ctx.fillStyle = vignette;
  ctx.fillRect(0, 0, size, size);

  ctx.restore();

  const glowBase = isIce ? mixHex(color, ICE_LIGHT_TINT, 0.4) : shadeHex(color, 0.15);
  const glowRgb = /^rgb\(([^)]+)\)$/.exec(glowBase)?.[1] ?? "255, 255, 255";
  const glow = ctx.createRadialGradient(cx, cy, r * 0.97, cx, cy, r * 1.18);
  glow.addColorStop(0, `rgba(${glowRgb}, 0.35)`);
  glow.addColorStop(1, "rgba(0, 0, 0, 0)");
  ctx.fillStyle = glow;
  ctx.beginPath();
  ctx.arc(cx, cy, r * 1.18, 0, Math.PI * 2);
  ctx.fill();

  return new THREE.CanvasTexture(canvas);
}

export function StatsGraph({
  seriesList,
  active,
  onOpenSeries,
  watchedEpisodes,
}: {
  seriesList: SeriesGraphNode[];
  active: boolean;
  onOpenSeries?: (seriesId: number) => void;
  watchedEpisodes?: Map<number, number>;
}) {
  const t = useT();
  const { theme } = useTheme();
  const linkColor = theme === "light" ? LINK_COLOR_LIGHT : LINK_COLOR_DARK;
  const graphRef = useRef<ForceGraphMethods<NodeObject<GNode>, GLink> | undefined>(undefined);
  const containerRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  const [height, setHeight] = useState(MIN_HEIGHT);
  const [width, setWidth] = useState(0);

  // Selected node and context menu state
  const [selectedNode, setSelectedNode] = useState<GNode | null>(null);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    node: GNode;
  } | null>(null);

  // Fallback map of watched episodes if not passed from parent
  const [internalWatchedMap, setInternalWatchedMap] = useState<Map<number, number> | null>(null);

  useEffect(() => {
    if (watchedEpisodes) return;
    let mounted = true;
    listLibrary()
      .then((items) => {
        if (!mounted) return;
        const map = new Map<number, number>();
        for (const item of items) {
          const eps =
            item.seen_episodes > 0
              ? item.seen_episodes
              : item.watched_externally
              ? Math.max(item.total_episodes, 1)
              : 0;
          map.set(item.series.id, eps);
        }
        setInternalWatchedMap(map);
      })
      .catch(() => {});
    return () => {
      mounted = false;
    };
  }, [watchedEpisodes]);

  const effectiveWatchedMap = watchedEpisodes ?? internalWatchedMap;

  const positionsRef = useRef<Map<string, Vec3>>(new Map());
  const layoutKeyRef = useRef<string | null>(null);
  const nodeObjectsRef = useRef<Map<string, THREE.Object3D>>(new Map());
  const textureCacheRef = useRef<Map<string, THREE.CanvasTexture>>(new Map());
  const didFitRef = useRef(false);
  const prevIdsRef = useRef<Set<string> | null>(null);

  // Close context menu on outside click
  useOutsideClick(!!contextMenu, menuRef, () => setContextMenu(null));

  // Escape key closes context menu and clears selection
  useEffect(() => {
    if (!active) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setContextMenu(null);
        setSelectedNode(null);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [active]);

  // Cleanup Three.js resources on unmount
  useEffect(() => {
    return () => {
      for (const [, obj] of nodeObjectsRef.current) {
        disposeNodeObject(obj);
      }
      nodeObjectsRef.current.clear();
      for (const [, tex] of textureCacheRef.current) {
        tex.dispose();
      }
      textureCacheRef.current.clear();
    };
  }, []);

  const { nodes, links } = useMemo(() => {
    const built = buildGraphData(seriesList, t("stats.noGenre"), effectiveWatchedMap);
    for (const n of built.nodes) {
      const saved = positionsRef.current.get(n.id);
      if (saved) {
        Object.assign(n as NodeObject<GNode>, saved);
      } else {
        // Seed new nodes near their genre hub's settled position instead of world origin
        const hubId = n.genres?.[0] ? `genre:${n.genres[0]}` : null;
        const hubSaved = hubId ? positionsRef.current.get(hubId) : null;
        const anchor = hubSaved ?? { x: 0, y: 0, z: 0 };
        const jitter = () => (Math.random() - 0.5) * 40;
        Object.assign(n as NodeObject<GNode>, {
          x: anchor.x + jitter(),
          y: anchor.y + jitter(),
          z: anchor.z + jitter(),
        });
      }
    }
    return built;
  }, [seriesList, t, effectiveWatchedMap]);

  const structuralKey = useMemo(
    () => `${nodes.map((n) => n.id).sort().join(",")}|${links.length}`,
    [nodes, links]
  );

  // Highlighting: active node and immediate neighbors
  const highlightNodesRef = useRef<Set<string> | null>(null);
  const { highlightNodes, highlightLinks } = useMemo(() => {
    if (!selectedNode) {
      return { highlightNodes: null, highlightLinks: null };
    }
    const hNodes = new Set<string>([selectedNode.id]);
    const hLinks = new Set<GLink>();

    for (const l of links) {
      const sId = typeof l.source === "object" ? (l.source as NodeObject<GNode>).id : l.source;
      const tId = typeof l.target === "object" ? (l.target as NodeObject<GNode>).id : l.target;
      if (sId === selectedNode.id) {
        if (tId) hNodes.add(tId as string);
        hLinks.add(l);
      } else if (tId === selectedNode.id) {
        if (sId) hNodes.add(sId as string);
        hLinks.add(l);
      }
    }

    return { highlightNodes: hNodes, highlightLinks: hLinks };
  }, [selectedNode, links]);

  // Dynamically update opacity of existing Three.js node objects on selection change without re-creating them
  useEffect(() => {
    highlightNodesRef.current = highlightNodes;
    for (const [id, obj] of nodeObjectsRef.current) {
      const isDimmed = highlightNodes !== null && !highlightNodes.has(id);
      const opacity = isDimmed ? 0.18 : 1.0;
      if (obj instanceof THREE.Sprite) {
        obj.material.opacity = opacity;
      } else if (obj instanceof THREE.Mesh) {
        (obj.material as THREE.MeshBasicMaterial).opacity = opacity;
      }
    }
  }, [highlightNodes]);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const recompute = () => {
      if (el.clientWidth === 0) return;
      const top = el.getBoundingClientRect().top;
      setHeight(Math.max(MIN_HEIGHT, window.innerHeight - top - BOTTOM_MARGIN));
      setWidth(el.clientWidth);
    };
    recompute();
    const ro = new ResizeObserver(recompute);
    ro.observe(el);
    window.addEventListener("resize", recompute);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", recompute);
    };
  }, []);

  useEffect(() => {
    if (!active) return;
    const el = containerRef.current;
    if (!el || el.clientWidth === 0) return;
    const top = el.getBoundingClientRect().top;
    setHeight(Math.max(MIN_HEIGHT, window.innerHeight - top - BOTTOM_MARGIN));
    setWidth(el.clientWidth);
  }, [active]);

  // Stats stays mounted while hidden; without this the force graph keeps its
  // requestAnimationFrame loop running. `hasGraph` re-runs the effect when the
  // ForceGraph3D instance first appears (or is remounted) while hidden.
  const hasGraph = seriesList.length > 0;
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    if (active) {
      fg.resumeAnimation();
    } else {
      fg.pauseAnimation();
    }
  }, [active, hasGraph]);

  useEffect(() => {
    const fg = graphRef.current as unknown as
      | { width?: (v: number) => void; height?: (v: number) => void }
      | undefined;
    if (!fg || !width) return;
    fg.width?.(width);
    fg.height?.(height);
  }, [width, height]);

  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    (fg.d3Force("charge") as { strength?: (v: number) => void } | undefined)?.strength?.(CHARGE_STRENGTH);
    (fg.d3Force("link") as { distance?: (fn: (link: GLink) => number) => void } | undefined)?.distance?.(
      (link: GLink) => link.distance ?? LINK_MAX_DIST
    );
    if (layoutKeyRef.current !== null && layoutKeyRef.current !== structuralKey) {
      fg.d3ReheatSimulation();
    }
  }, [structuralKey]);

  useEffect(() => {
    const currentIds = new Set(nodes.map((n) => n.id));
    for (const [id, obj] of nodeObjectsRef.current) {
      if (!currentIds.has(id)) {
        disposeNodeObject(obj);
        nodeObjectsRef.current.delete(id);
      }
    }
  }, [nodes]);

  const handleEngineStop = useCallback(() => {
    const snapshot = positionsRef.current;
    for (const n of nodes as NodeObject<GNode>[]) {
      if (n.x !== undefined && n.y !== undefined && n.z !== undefined) {
        snapshot.set(n.id as string, { x: n.x, y: n.y, z: n.z });
      }
    }
    layoutKeyRef.current = structuralKey;

    const currentIds = new Set(nodes.map((n) => n.id as string));
    const grew =
      prevIdsRef.current !== null &&
      Array.from(currentIds).some((id) => !prevIdsRef.current!.has(id));
    prevIdsRef.current = currentIds;

    if (!didFitRef.current) {
      didFitRef.current = true;
      graphRef.current?.zoomToFit(400, 60);
    } else if (grew) {
      graphRef.current?.zoomToFit(600, 60);
    }
  }, [nodes, structuralKey]);

  const handleNodeThreeObject = useCallback(
    (node: NodeObject<GNode>) => {
      const existing = nodeObjectsRef.current.get(node.id as string);
      if (existing) disposeNodeObject(existing);
      let obj: THREE.Object3D;
      try {
        // Read through a ref: depending on the selection here rebuilt (and
        // re-loaded the cover of) every node on each click.
        const current = highlightNodesRef.current;
        const isDimmed = current !== null && !current.has(node.id as string);
        const opacity = isDimmed ? 0.18 : 1.0;

        if (node.kind === "series") {
          const coverSrc =
            node.coverUrl && /^(data|file):/.test(node.coverUrl) ? resolveCoverUrl(node.coverUrl) : null;
          const hasCover = !!coverSrc;
          const strokeColor = theme === "light" ? "rgba(23, 34, 46, 0.55)" : "rgba(255, 255, 255, 0.35)";
          const fallbackTexture = () =>
            new THREE.CanvasTexture(
              fallbackCircleCanvas(node.color ?? categoryColor(NO_GENRE_KEY), strokeColor)
            );
          const material = new THREE.SpriteMaterial({ transparent: true, opacity });
          material.map = hasCover
            ? new THREE.TextureLoader().setCrossOrigin("anonymous").load(
                coverSrc as string,
                undefined,
                undefined,
                () => {
                  material.map?.dispose();
                  material.map = fallbackTexture();
                  material.needsUpdate = true;
                }
              )
            : fallbackTexture();
          const sprite = new THREE.Sprite(material);
          const size = node.size ?? SERIES_MIN_SIZE;
          sprite.scale.set(size, size, 1);
          obj = sprite;
        } else {
          const color = node.color ?? "#4aa8ff";
          const radius = node.radius ?? HUB_MIN_R;
          let tex = textureCacheRef.current.get(color);
          if (!tex) {
            tex = planetTexture(color);
            textureCacheRef.current.set(color, tex);
          }
          const material = new THREE.MeshBasicMaterial({ map: tex, transparent: true, opacity });
          obj = new THREE.Mesh(new THREE.SphereGeometry(radius, 24, 24), material);
        }
      } catch (e) {
        console.error("nodeThreeObject failed for", node.id, e);
        obj = new THREE.Mesh(new THREE.SphereGeometry(5, 8, 8), new THREE.MeshBasicMaterial({ color: "red" }));
      }
      nodeObjectsRef.current.set(node.id as string, obj);
      return obj;
    },
    [theme]
  );

  const flyCameraToNode = useCallback((node: NodeObject<GNode>) => {
    const fg = graphRef.current;
    if (!fg || node.x === undefined || node.y === undefined || node.z === undefined) return;
    // Far enough to keep the node's neighbours in frame (links run up to
    // LINK_MAX_DIST); hubs are bigger and have far more neighbours.
    const distance = node.kind === "series" ? 200 : 340;
    const distRatio = 1 + distance / Math.hypot(node.x, node.y, node.z || 1);
    fg.cameraPosition(
      { x: node.x * distRatio, y: node.y * distRatio, z: node.z * distRatio },
      node as unknown as { x: number; y: number; z: number },
      1200
    );
  }, []);

  const handleNodeClick = useCallback(
    (node: NodeObject<GNode>) => {
      setSelectedNode(node);
      setContextMenu(null);
      flyCameraToNode(node);
    },
    [flyCameraToNode]
  );

  const handleNodeRightClick = useCallback((node: NodeObject<GNode>, event: MouseEvent) => {
    event.preventDefault();
    const rect = containerRef.current?.getBoundingClientRect();
    const x = rect ? event.clientX - rect.left : event.clientX;
    const y = rect ? event.clientY - rect.top : event.clientY;
    setContextMenu({ x, y, node });
  }, []);

  const handleBackgroundClick = useCallback(() => {
    setSelectedNode(null);
    setContextMenu(null);
  }, []);

  return (
    <div
      ref={containerRef}
      className="stats-graph-galaxy graph-clip"
      role="img"
      aria-label={t("stats.graphAria", {
        series: seriesList.length,
        genres: new Set(seriesList.flatMap((s) => s.genres)).size,
      })}
      onContextMenu={(e) => e.preventDefault()}
    >
      {seriesList.length === 0 ? (
        <div className="empty">{t("stats.graphEmpty")}</div>
      ) : (
        <ForceGraph3D<GNode, GLink>
          ref={graphRef}
          width={width || undefined}
          height={height}
          graphData={{ nodes, links }}
          backgroundColor="#00000000"
          controlType="orbit"
          enableNavigationControls={true}
          enablePointerInteraction={true}
          enableNodeDrag={false}
          linkColor={(link: GLink) => {
            if (!highlightLinks) return linkColor;
            return highlightLinks.has(link)
              ? theme === "light"
                ? LINK_HIGHLIGHT_LIGHT
                : LINK_HIGHLIGHT_DARK
              : theme === "light"
              ? LINK_DIM_LIGHT
              : LINK_DIM_DARK;
          }}
          linkWidth={(link: GLink) => {
            const base = link.width ?? 1;
            if (!highlightLinks) return base;
            return highlightLinks.has(link) ? Math.max(base * 1.5, 2.5) : base * 0.4;
          }}
          cooldownTicks={COOLDOWN_TICKS}
          onEngineStop={handleEngineStop}
          nodeLabel={(node) =>
            node.kind === "series" ? node.label : `${node.label} (${node.count ?? 0})`
          }
          nodeThreeObject={handleNodeThreeObject}
          onNodeClick={handleNodeClick}
          onNodeRightClick={handleNodeRightClick}
          onBackgroundClick={handleBackgroundClick}
          onBackgroundRightClick={handleBackgroundClick}
        />
      )}

      {/* Floating UI Overlays */}
      <div className="graph-overlay-container">
        {selectedNode && (
          <div className="graph-info-panel" role="dialog" aria-label={selectedNode.label}>
            <div className="graph-info-header">
              <h3 className="graph-info-title">{selectedNode.label}</h3>
              <button
                type="button"
                className="graph-info-close"
                onClick={() => setSelectedNode(null)}
                aria-label={t("stats.graphClose")}
              >
                ✕
              </button>
            </div>
            <div className="graph-info-body">
              {selectedNode.kind === "series" ? (
                <>
                  {selectedNode.genres && selectedNode.genres.length > 0 && (
                    <div className="graph-info-row">
                      <span className="graph-info-label">{t("stats.graphGenres")}</span>
                      <div className="graph-info-genres">
                        {selectedNode.genres.map((g) => (
                          <span key={g} className="graph-info-genre-tag">
                            {g}
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                  <div className="graph-info-row">
                    <span className="graph-info-label">{t("stats.graphEpisodesWatched")}</span>
                    <span className="graph-info-value">
                      {t("stats.graphEpisodesCount", { count: selectedNode.episodes ?? 0 })}
                    </span>
                  </div>
                </>
              ) : (
                <>
                  <div className="graph-info-row">
                    <span className="graph-info-label">{t("stats.byGenre")}</span>
                    <span className="graph-info-value">
                      {t("stats.graphSeriesCount", { count: selectedNode.count ?? 0 })}
                    </span>
                  </div>
                  <div className="graph-info-row">
                    <span className="graph-info-label">{t("stats.graphEpisodesWatched")}</span>
                    <span className="graph-info-value">
                      {t("stats.graphEpisodesCount", { count: selectedNode.episodes ?? 0 })}
                    </span>
                  </div>
                </>
              )}
            </div>
          </div>
        )}

        {contextMenu && (
          <div
            ref={menuRef}
            className="graph-context-menu"
            style={{
              left: Math.min(contextMenu.x, (width || 400) - 170),
              top: Math.min(contextMenu.y, (height || 400) - 90),
            }}
          >
            {contextMenu.node.kind === "series" && (
              <button
                type="button"
                className="graph-context-item"
                onClick={() => {
                  if (onOpenSeries && contextMenu.node.seriesId !== undefined) {
                    onOpenSeries(contextMenu.node.seriesId);
                  } else {
                    // Fallback when no navigation callback exists on parent
                    setSelectedNode(contextMenu.node);
                  }
                  setContextMenu(null);
                }}
              >
                {t("stats.graphOpenSeries")}
              </button>
            )}
            <button
              type="button"
              className="graph-context-item"
              onClick={() => {
                flyCameraToNode(contextMenu.node as NodeObject<GNode>);
                setContextMenu(null);
              }}
            >
              {t("stats.graphCenterCamera")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
