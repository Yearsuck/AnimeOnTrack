import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ForceGraph3D, { type ForceGraphMethods, type NodeObject } from "react-force-graph-3d";
import * as THREE from "three";
import { useT } from "../i18n";
import { useTheme } from "../theme";
import { categoryColor, SIN_GENERO_LABEL } from "../lib/categoryColor";
import type { SeriesGraphNode } from "../types";

// The root node's near-white dark-theme color (#e9ecef) is invisible on the
// light theme's pale backdrop — pick per theme instead of a single constant.
const ROOT_COLOR_DARK = "#e9ecef";
const ROOT_COLOR_LIGHT = "#33414f";
const HUB_MIN_R = 6;
const HUB_MAX_R = 22;
const SERIES_SPRITE_SIZE = 18;
const MIN_HEIGHT = 500;
const BOTTOM_MARGIN = 24;
// Spread nodes far enough apart that the edges between hubs and series read
// clearly (the earlier -260 / 90 packed them into an unreadable ball). The
// wider layout is re-framed once via zoomToFit on the first settle.
const CHARGE_STRENGTH = -520;
const LINK_DISTANCE = 150;
// Bound the initial layout run instead of letting it simulate indefinitely
// (the library's own cooldownTime default is 15s of wall time). Once the
// engine stops (or this many ticks pass), onEngineStop below snapshots
// positions so a later rebuild can reuse them instead of re-scrambling.
const COOLDOWN_TICKS = 200;

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

type GNodeKind = "root" | "genreHub" | "kindHub" | "series";

interface GNode {
  id: string;
  kind: GNodeKind;
  label: string;
  count?: number;
  color?: string;
  coverUrl?: string | null;
}

interface GLink {
  source: string;
  target: string;
}

function buildGraphData(
  seriesList: SeriesGraphNode[],
  rootLabel: string
): { nodes: GNode[]; links: GLink[] } {
  const nodes: GNode[] = [{ id: "root", kind: "root", label: rootLabel }];
  const links: GLink[] = [];

  const genreCounts = new Map<string, number>();
  const kindCounts = new Map<string, number>();
  let sinGeneroCount = 0;

  for (const s of seriesList) {
    if (s.genres.length === 0) {
      sinGeneroCount++;
    } else {
      for (const g of s.genres) genreCounts.set(g, (genreCounts.get(g) ?? 0) + 1);
    }
    if (s.kind) kindCounts.set(s.kind, (kindCounts.get(s.kind) ?? 0) + 1);
  }

  for (const [g, count] of genreCounts) {
    nodes.push({ id: `genre:${g}`, kind: "genreHub", label: g, count, color: categoryColor(g) });
    links.push({ source: "root", target: `genre:${g}` });
  }
  if (sinGeneroCount > 0) {
    nodes.push({
      id: `genre:${SIN_GENERO_LABEL}`,
      kind: "genreHub",
      label: SIN_GENERO_LABEL,
      count: sinGeneroCount,
      color: categoryColor(SIN_GENERO_LABEL),
    });
    links.push({ source: "root", target: `genre:${SIN_GENERO_LABEL}` });
  }
  for (const [k, count] of kindCounts) {
    nodes.push({ id: `kind:${k}`, kind: "kindHub", label: k, count, color: categoryColor(k) });
    links.push({ source: "root", target: `kind:${k}` });
  }

  for (const s of seriesList) {
    const id = `series:${s.id}`;
    const fallbackColor = s.genres[0] ? categoryColor(s.genres[0]) : categoryColor(SIN_GENERO_LABEL);
    nodes.push({ id, kind: "series", label: s.title, color: fallbackColor, coverUrl: s.cover_url });
    if (s.genres.length === 0) {
      links.push({ source: `genre:${SIN_GENERO_LABEL}`, target: id });
    } else {
      for (const g of s.genres) links.push({ source: `genre:${g}`, target: id });
    }
    if (s.kind) links.push({ source: `kind:${s.kind}`, target: id });
  }

  return { nodes, links };
}

function hubRadius(count: number, maxCount: number): number {
  if (maxCount <= 0) return HUB_MIN_R;
  return HUB_MIN_R + (HUB_MAX_R - HUB_MIN_R) * (count / maxCount);
}

// `strokeColor` outlines the disc so a series with no cover art still reads
// as a distinct node against the light theme's pale backdrop (dark fills
// already had enough contrast on the dark backdrop; on light the same flat
// fill can blend into the sky).
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

// Lighten (amt > 0) / darken (amt < 0) a #rrggbb hex, clamped per channel.
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

// Deterministic pseudo-random hash + bilinear-smoothed value noise, used to
// bake latitude-band wobble and blotch/storm texture below. Deterministic
// (not Math.random) so the same hub color always bakes the same surface —
// a rebuild (theme flip, follow/unfollow elsewhere) doesn't re-roll the look.
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

// A baked planet disc: sphere-shaded gradient (light side / dark limb —
// the terminator) plus latitude bands, low-frequency blotch noise, limb
// darkening, and a faint atmospheric glow. Painted within a circular clip on
// a square canvas — mapped onto the unlit SphereGeometry's default (front-
// facing) UV patch, the same "baked lit disc" trick as before, just with a
// real surface instead of a flat gradient. Single CanvasTexture on a
// MeshBasicMaterial — no scene light (react-force-graph-3d ships none).
function planetTexture(color: string): THREE.CanvasTexture {
  const size = 256;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const cx = size / 2;
  const cy = size / 2;
  const r = size / 2 - 2;

  // Per-color seed so different hubs don't all bake the exact same band/
  // blotch layout.
  let seed = 0;
  for (let i = 0; i < color.length; i++) seed += color.charCodeAt(i);
  const nx = seed * 0.13;
  const ny = seed * 0.071;

  const lightC = shadeHex(color, 0.4);
  const darkC = shadeHex(color, -0.35);
  const limbC = shadeHex(color, -0.65);

  ctx.save();
  ctx.beginPath();
  ctx.arc(cx, cy, r, 0, Math.PI * 2);
  ctx.clip();

  // 1) Base sphere-shaded gradient: the terminator (light side upper-left,
  // dark limb lower-right) — this alone was the previous texture.
  const base = ctx.createRadialGradient(cx - r * 0.35, cy - r * 0.38, r * 0.05, cx, cy, r * 1.05);
  base.addColorStop(0, lightC);
  base.addColorStop(0.45, color);
  base.addColorStop(1, darkC);
  ctx.fillStyle = base;
  ctx.fillRect(0, 0, size, size);

  // 2) Latitude bands (gas-giant-style stripes), wobbled by low-freq noise so
  // they read as organic, not ruled lines.
  const bandCount = 7;
  for (let i = 0; i < bandCount; i++) {
    const bandY = (i / bandCount) * size;
    const wobble = (fbm(i * 0.7 + nx, ny, 2) - 0.5) * size * 0.06;
    const bandHeight = size / bandCount;
    const toneAmt = (i % 2 === 0 ? 1 : -1) * (0.05 + 0.05 * fbm(i * 1.3 + nx, ny + 5, 2));
    ctx.globalAlpha = 0.35;
    ctx.fillStyle = shadeHex(color, toneAmt);
    ctx.fillRect(0, bandY + wobble, size, bandHeight);
  }
  ctx.globalAlpha = 1;

  // 3) Low-frequency blotch noise (continents/storms), painted at a coarse
  // cell size so it stays cheap while reading as organic.
  const cell = 4;
  for (let y = 0; y < size; y += cell) {
    for (let x = 0; x < size; x += cell) {
      const n = fbm(x * 0.045 + nx, y * 0.045 + ny, 3);
      if (n > 0.56) {
        const amt = (n - 0.56) * 0.9;
        ctx.globalAlpha = Math.min(0.45, amt);
        ctx.fillStyle = n > 0.68 ? lightC : darkC;
        ctx.fillRect(x, y, cell, cell);
      }
    }
  }
  ctx.globalAlpha = 1;

  // 4) Limb darkening: a radial vignette darkening toward the disc's rim
  // regardless of the terminator gradient above — the "sphere, not flat
  // circle" cue.
  const vignette = ctx.createRadialGradient(cx, cy, r * 0.55, cx, cy, r);
  vignette.addColorStop(0, "rgba(0, 0, 0, 0)");
  const limbRgb = /^rgb\(([^)]+)\)$/.exec(limbC)?.[1] ?? "0, 0, 0";
  vignette.addColorStop(1, `rgba(${limbRgb}, 0.55)`);
  ctx.fillStyle = vignette;
  ctx.fillRect(0, 0, size, size);

  ctx.restore();

  // 5) Faint atmospheric glow just outside the disc edge (after the clip is
  // released) — suggests a thin atmosphere without a scene light.
  const glowRgb = /^rgb\(([^)]+)\)$/.exec(shadeHex(color, 0.15))?.[1] ?? "255, 255, 255";
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
}: {
  seriesList: SeriesGraphNode[];
  active: boolean;
}) {
  const t = useT();
  const { theme } = useTheme();
  const rootColor = theme === "light" ? ROOT_COLOR_LIGHT : ROOT_COLOR_DARK;
  const graphRef = useRef<ForceGraphMethods<NodeObject<GNode>, GLink> | undefined>(undefined);
  const containerRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(MIN_HEIGHT);
  const [width, setWidth] = useState(0);

  // Last settled layout, snapshotted on onEngineStop and keyed by node id —
  // survives graphData rebuilds (a fresh seriesList prop produces brand new
  // node/link objects every time) so re-following/backfilling doesn't
  // re-scramble nodes that were already laid out.
  const positionsRef = useRef<Map<string, Vec3>>(new Map());
  // Structural key of the layout currently reflected in positionsRef —
  // sorted node ids + link count, a cheap value comparison. useMemo returns
  // a new nodes/links array on every rebuild even when the data is
  // identical, so object identity can't be used to detect "did anything
  // actually change".
  const layoutKeyRef = useRef<string | null>(null);
  // Registry of the three.js object currently representing each node, so a
  // replaced or removed node's material/texture/geometry can be disposed
  // explicitly. The graph now stays mounted indefinitely, so nothing else
  // ever garbage-collects these.
  const nodeObjectsRef = useRef<Map<string, THREE.Object3D>>(new Map());
  // Frame the (now wider) layout exactly once, on the first settle — later
  // rebuilds reuse the snapshot and must NOT re-fit (that would fight the
  // frozen layout / user's own camera). A later re-fit still happens, but
  // only when handleEngineStop below detects the node set grew.
  const didFitRef = useRef(false);
  // Node ids present as of the last settle, so handleEngineStop can tell
  // "structure changed because a node was added" apart from "changed because
  // one was removed" (only the former should trigger a re-frame). `null`
  // means "no settle yet" — the very first settle uses didFitRef instead.
  const prevIdsRef = useRef<Set<string> | null>(null);

  const { nodes, links } = useMemo(() => {
    const built = buildGraphData(seriesList, t("stats.graphRootLabel"));
    const rootSaved = positionsRef.current.get("root");
    for (const n of built.nodes) {
      const saved = positionsRef.current.get(n.id);
      if (saved) {
        Object.assign(n as NodeObject<GNode>, saved);
      } else if (rootSaved) {
        // Brand-new node (e.g. just followed a series while Stats is open):
        // seed it near the root's last settled position instead of the
        // world origin, so it doesn't pile up at the center behind existing
        // nodes before/if the reheat separates it out.
        const jitter = () => (Math.random() - 0.5) * 40;
        Object.assign(n as NodeObject<GNode>, {
          x: rootSaved.x + jitter(),
          y: rootSaved.y + jitter(),
          z: rootSaved.z + jitter(),
        });
      }
    }
    return built;
  }, [seriesList, t]);
  const structuralKey = useMemo(
    () => `${nodes.map((n) => n.id).sort().join(",")}|${links.length}`,
    [nodes, links]
  );
  const maxHubCount = useMemo(
    () =>
      Math.max(
        1,
        ...nodes.filter((n) => n.kind === "genreHub" || n.kind === "kindHub").map((n) => n.count ?? 0)
      ),
    [nodes]
  );

  // Fill the container instead of a fixed px box — 3d-force-graph defaults
  // its canvas to `window.innerWidth`/height captured once at module load,
  // which is not the same as this container's actual size and doesn't
  // update on its own. Measure the real container box and push it in as
  // width/height props (and via the imperative accessors as a fallback,
  // since a bare prop change doesn't always reach the renderer post-mount).
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const recompute = () => {
      // display:none (tab hidden via App.tsx's `hidden` attribute) collapses
      // the box to clientWidth 0 — ResizeObserver and getBoundingClientRect
      // still fire/report through that collapse, and acting on it would
      // push a zero size into the graph, leaving the canvas blank on
      // re-show. Ignore zero-width reads and keep the last known-good size;
      // the `active` effect below forces one fresh recompute on becoming
      // visible again (covers the case where the window was resized while
      // hidden).
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

  useEffect(() => {
    const fg = graphRef.current as unknown as
      | { width?: (v: number) => void; height?: (v: number) => void }
      | undefined;
    if (!fg || !width) return;
    fg.width?.(width);
    fg.height?.(height);
  }, [width, height]);

  // Spread nodes out further apart — default d3-force charge/link settings
  // packed hubs and series so tightly the connecting edges were unreadable.
  // Reheating restarts the physics (alpha back to 1), which is what makes
  // the layout visibly move/re-settle — only do that when the node/link set
  // actually changed structurally, not on every rebuild of the same data
  // (the previous unconditional call here was why switching tabs — which
  // used to force a remount and thus a fresh nodes/links object — always
  // re-scrambled the graph).
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    (fg.d3Force("charge") as { strength?: (v: number) => void } | undefined)?.strength?.(CHARGE_STRENGTH);
    (fg.d3Force("link") as { distance?: (v: number) => void } | undefined)?.distance?.(LINK_DISTANCE);
    if (layoutKeyRef.current !== null && layoutKeyRef.current !== structuralKey) {
      fg.d3ReheatSimulation();
    }
  }, [structuralKey]);

  // Drop three.js objects for nodes that no longer exist (unfollowed
  // series, a genre hub that emptied out) — nodeThreeObject only fires
  // again for ids still present, so removals need an explicit sweep.
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

    // One-shot framing after the initial layout settles, so the wider spread
    // stays fully visible without touching the reheat/snapshot invariant.
    if (!didFitRef.current) {
      didFitRef.current = true;
      graphRef.current?.zoomToFit(400, 60);
    } else if (grew) {
      // A brand-new node appeared since the last settle (following a series
      // while Stats is open, or opening Stats right after following one) —
      // re-frame once so it isn't left outside the current view. Do NOT
      // re-fit on pure removals or identical structure — this callback only
      // fires again at all when a reheat ran, i.e. structuralKey changed, so
      // "not grew" here means "shrank" and the camera is left alone.
      graphRef.current?.zoomToFit(600, 60);
    }
  }, [nodes, structuralKey]);

  const handleNodeThreeObject = useCallback(
    (node: NodeObject<GNode>) => {
      const existing = nodeObjectsRef.current.get(node.id as string);
      if (existing) disposeNodeObject(existing);
      let obj: THREE.Object3D;
      try {
        if (node.kind === "series") {
          const hasCover = !!node.coverUrl && node.coverUrl.startsWith("data:");
          const strokeColor = theme === "light" ? "rgba(23, 34, 46, 0.55)" : "rgba(255, 255, 255, 0.35)";
          const texture = hasCover
            ? new THREE.TextureLoader().load(node.coverUrl as string)
            : new THREE.CanvasTexture(
                fallbackCircleCanvas(node.color ?? categoryColor(SIN_GENERO_LABEL), strokeColor)
              );
          const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: texture, transparent: true }));
          sprite.scale.set(SERIES_SPRITE_SIZE, SERIES_SPRITE_SIZE, 1);
          obj = sprite;
        } else {
          const color = node.kind === "root" ? rootColor : node.color ?? "#4aa8ff";
          const radius = node.kind === "root" ? HUB_MAX_R * 1.1 : hubRadius(node.count ?? 0, maxHubCount);
          // MeshBasicMaterial (unlit) rather than a lit material: the scene
          // has no light (react-force-graph-3d ships none), so a lit material
          // would render black. The planet look comes from a baked radial
          // gradient texture (light side / dark limb), not real shading.
          obj = new THREE.Mesh(
            new THREE.SphereGeometry(radius, 24, 24),
            new THREE.MeshBasicMaterial({ map: planetTexture(color) })
          );
        }
      } catch (e) {
        // A malformed cover_url or texture-decode failure shouldn't take
        // down the whole graph — fall back to a visible marker for just
        // this node instead of an uncaught exception mid-render.
        console.error("nodeThreeObject failed for", node.id, e);
        obj = new THREE.Mesh(new THREE.SphereGeometry(5, 8, 8), new THREE.MeshBasicMaterial({ color: "red" }));
      }
      nodeObjectsRef.current.set(node.id as string, obj);
      return obj;
    },
    // `theme`/`rootColor` deps make this callback's identity change on a
    // theme flip, which the underlying three-forcegraph library detects as
    // "nodeThreeObject accessor changed" and uses to rebuild every node's
    // object (see three-forcegraph's nodeDataMapper.clear() on that prop
    // changing) — that's what repaints existing nodes with the new theme's
    // colors without a reload.
    [maxHubCount, theme, rootColor]
  );

  // The ref-bearing div below must always mount, even while seriesList is
  // still empty (the normal state on first render — data loads async, so
  // this is briefly true on every real app launch, not just the true-empty
  // case). The one-shot mount effect above measures containerRef.current
  // exactly once; if the empty case returned a differently-shaped tree
  // instead of this same div, the ref would be null on that first pass and
  // never get a second chance to attach, permanently starving the graph.
  return (
    <div
      ref={containerRef}
      className="stats-graph-galaxy"
      style={{ borderRadius: "var(--radius-sm)", overflow: "hidden" }}
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
        cooldownTicks={COOLDOWN_TICKS}
        onEngineStop={handleEngineStop}
        nodeLabel={(node) =>
          node.kind === "series" ? node.label : `${node.label} (${node.count ?? 0})`
        }
        nodeThreeObject={handleNodeThreeObject}
        onNodeClick={(node) => {
          const fg = graphRef.current;
          if (!fg || node.x === undefined || node.y === undefined || node.z === undefined) return;
          const distance = 80;
          const distRatio = 1 + distance / Math.hypot(node.x, node.y, node.z || 1);
          fg.cameraPosition(
            { x: node.x * distRatio, y: node.y * distRatio, z: node.z * distRatio },
            node as unknown as { x: number; y: number; z: number },
            1200
          );
        }}
      />
      )}
    </div>
  );
}
