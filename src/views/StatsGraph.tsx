import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useBackLayer } from "../lib/backStack";
import ForceGraph3D, { type ForceGraphMethods, type NodeObject } from "react-force-graph-3d";
import * as THREE from "three";
import { useT, type MessageKey } from "../i18n";
import { useTheme } from "../theme";
import { NO_GENRE_KEY } from "../lib/categoryColor";
import { resolveCoverUrl, useCoverFailure } from "../lib/posterFallback";
import { listLibrary } from "../api";
import type { SeriesGraphNode } from "../types";
import {
  OTHER_GENRES_KEY,
  buildGenreAtlas,
  declutterLabels,
  escapeHtml,
  fitDistance,
  hubCounts,
  labelStyle,
  hubNodeId,
  linkKey,
  neighbourhood,
  placeHubs,
  presentKinds,
  scaleSqrt,
  screenSpriteScale,
  searchShows,
  showMatchesKind,
  showNodeId,
  topShowsOf,
  withAlpha,
  type GraphKind,
  type Hub,
  type KindFilter,
  type LabelStyle,
  type ScreenLabel,
  type Selection,
  type Show,
  type Vec3,
} from "./stats/graphModel";
import "../styles/statsgraph.css";

// Genre map: genres are labelled hubs pinned on a sphere, each series is a
// cover disc that the force layout pulls towards (only) its own genres. All
// the "which links exist / where hubs go / which colour" logic lives in
// ./stats/graphModel.ts; this file is the three.js + overlay shell.

const HUB_MIN_R = 9;
const HUB_MAX_R = 30;
const SHOW_MIN_SIZE = 10;
const SHOW_MAX_SIZE = 30;
// A show sits closer to a genre it is central to (many episodes watched).
const LINK_MIN_DIST = 55;
const LINK_MAX_DIST = 125;
const SHOW_CHARGE = -22;
const CHARGE_DISTANCE_MAX = 300;
const COOLDOWN_TICKS = 180;
const LABEL_GAP = 4;
// Extra room around the cloud for the hub labels when fitting the camera.
const FIT_LABEL_MARGIN = 18;
const LABEL_DECLUTTER_GAP = 3;
const LABEL_EASE = 0.3;
const NARROW_QUERY = "(max-width: 760px)";
const DIM_SHOW = 0.15;
const DIM_HUB = 0.28;
const DIM_LABEL = 0.45;
const MIN_HEIGHT = 500;
const BOTTOM_MARGIN = 24;
const AUTO_ROTATE_SPEED = 0.35;
const DISC_PX = 96;

type GNode = {
  id: string;
  type: "hub" | "show";
  key?: string;
  showId?: number;
  color: string;
  size: number;
};

type GLink = {
  source: string | NodeObject<GNode>;
  target: string | NodeObject<GNode>;
  key: string;
  hub: string;
  showId: number;
  color: string;
  distance: number;
};

interface Tokens {
  text: string;
  muted: string;
  surface: string;
  border: string;
  font: string;
}

function readTokens(): Tokens {
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
  return {
    text: v("--text", "#e9eff5"),
    muted: v("--muted", "#90a4b7"),
    surface: v("--surface", "#14212f"),
    border: v("--border", "#24384a"),
    font: v("--font", "sans-serif"),
  };
}

const KIND_LABEL: Record<GraphKind, MessageKey> = {
  TV: "stats.graphKindTV",
  MOVIE: "stats.graphKindMovie",
  OVA: "stats.graphKindOVA",
  ONA: "stats.graphKindONA",
  SPECIAL: "stats.graphKindSpecial",
  OTHER: "stats.graphKindOther",
};

function prefersReducedMotion(): boolean {
  return typeof window !== "undefined" && window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
}

function disposeObject(obj: THREE.Object3D, keepGeometry: THREE.BufferGeometry | null): void {
  obj.traverse((child) => {
    const any = child as unknown as { material?: THREE.Material | THREE.Material[]; geometry?: THREE.BufferGeometry };
    const mats = Array.isArray(any.material) ? any.material : any.material ? [any.material] : [];
    for (const m of mats) {
      (m as THREE.SpriteMaterial).map?.dispose();
      m.dispose();
    }
    // Sprites share one three.js geometry; disposing it only forces a re-upload.
    if (any.geometry && any.geometry !== keepGeometry && !(child instanceof THREE.Sprite)) any.geometry.dispose();
  });
}

function setObjectOpacity(obj: THREE.Object3D, opacity: number): void {
  obj.traverse((child) => {
    // The label keeps its own (higher) floor so a dimmed genre stays readable.
    if (child.userData.isLabel) return;
    const mat = (child as unknown as { material?: THREE.Material }).material;
    if (mat) mat.opacity = opacity;
  });
}

/** Round disc in the genre colour, with the cover drawn inside once it loads. */
function makeDisc(color: string, coverSrc: string | null, isAlive: () => boolean): THREE.SpriteMaterial {
  const canvas = document.createElement("canvas");
  canvas.width = DISC_PX;
  canvas.height = DISC_PX;
  const ctx = canvas.getContext("2d")!;
  const c = DISC_PX / 2;
  ctx.beginPath();
  ctx.arc(c, c, c - 1, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  const material = new THREE.SpriteMaterial({ map: texture, transparent: true, depthWrite: false });
  if (coverSrc) {
    const img = new Image();
    // Without CORS approval the load fails outright (instead of tainting the canvas).
    img.crossOrigin = "anonymous";
    img.onload = () => {
      if (!isAlive() || !img.naturalWidth) return;
      const inner = c - 6;
      ctx.save();
      ctx.beginPath();
      ctx.arc(c, c, inner, 0, Math.PI * 2);
      ctx.clip();
      const scale = Math.max((inner * 2) / img.naturalWidth, (inner * 2) / img.naturalHeight);
      const w = img.naturalWidth * scale;
      const h = img.naturalHeight * scale;
      ctx.drawImage(img, c - w / 2, c - h / 2, w, h);
      ctx.restore();
      texture.needsUpdate = true;
    };
    img.src = coverSrc;
  }
  return material;
}

/** Billboarded genre label: a pill in surface colour with the genre swatch, name and count. */
function drawLabel(text: string, count: string, color: string, tokens: Tokens, style: LabelStyle): HTMLCanvasElement {
  const dpr = 2;
  const h = style.px;
  const padX = Math.round(h * 0.38);
  const dot = Math.round(h * 0.3);
  const gap = Math.round(h * 0.23);
  const nameFont = `600 ${style.fontPx}px ${tokens.font}`;
  const countFont = `500 ${style.fontPx - 1}px ${tokens.font}`;
  const measure = document.createElement("canvas").getContext("2d")!;
  measure.font = nameFont;
  const nameW = measure.measureText(text).width;
  measure.font = countFont;
  const countW = measure.measureText(count).width;
  const w = Math.ceil(padX + dot + gap + nameW + (count ? gap + countW : 0) + padX);
  const canvas = document.createElement("canvas");
  canvas.width = w * dpr;
  canvas.height = h * dpr;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(dpr, dpr);
  const r = h / 2;
  ctx.beginPath();
  ctx.moveTo(r, 0.5);
  ctx.lineTo(w - r, 0.5);
  ctx.arc(w - r, r, r - 0.5, -Math.PI / 2, Math.PI / 2);
  ctx.lineTo(r, h - 0.5);
  ctx.arc(r, r, r - 0.5, Math.PI / 2, (3 * Math.PI) / 2);
  ctx.closePath();
  ctx.fillStyle = withAlpha(tokens.surface, 0.94);
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = color;
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(padX + dot / 2, r, dot / 2, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  ctx.textBaseline = "middle";
  ctx.font = nameFont;
  ctx.fillStyle = tokens.text;
  ctx.fillText(text, padX + dot + gap, r + 0.5);
  if (count) {
    ctx.font = countFont;
    ctx.fillStyle = tokens.muted;
    ctx.fillText(count, padX + dot + gap + nameW + gap, r + 0.5);
  }
  return canvas;
}

interface LabelEntry {
  sprite: THREE.Sprite;
  hub: Hub;
  maxCount: number;
  /** Pill size in screen px, for the declutter pass. */
  w: number;
  h: number;
  /** Current upward lift in px (eased towards the declutter target). */
  lift: number;
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
  const graphRef = useRef<ForceGraphMethods<NodeObject<GNode>, GLink> | undefined>(undefined);
  const containerRef = useRef<HTMLDivElement>(null);

  const [height, setHeight] = useState(MIN_HEIGHT);
  const [width, setWidth] = useState(0);
  const [selection, setSelection] = useState<Selection>(null);
  const [kindFilter, setKindFilter] = useState<KindFilter>("all");
  const [query, setQuery] = useState("");
  const [searchIndex, setSearchIndex] = useState(0);
  const [tokens, setTokens] = useState<Tokens>(() => readTokens());
  const [reducedMotion, setReducedMotion] = useState(prefersReducedMotion);

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
              ? Math.max(item.total_episodes, 0)
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

  // The theme provider writes data-theme in its own effect, after ours, so
  // read the tokens on the next frame.
  useEffect(() => {
    const id = requestAnimationFrame(() => setTokens(readTokens()));
    return () => cancelAnimationFrame(id);
  }, [theme]);

  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-reduced-motion: reduce)");
    if (!mq) return;
    const onChange = () => setReducedMotion(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  const atlas = useMemo(
    () => buildGenreAtlas(seriesList, (id) => effectiveWatchedMap?.get(id) ?? 0),
    [seriesList, effectiveWatchedMap]
  );
  const hubByKey = useMemo(() => new Map(atlas.hubs.map((h) => [h.key, h])), [atlas]);
  const showById = useMemo(() => new Map(atlas.shows.map((s) => [s.id, s])), [atlas]);
  const hubPositions = useMemo(() => placeHubs(atlas.hubs, atlas.shows), [atlas]);
  const counts = useMemo(() => hubCounts(atlas, kindFilter), [atlas, kindFilter]);
  const kinds = useMemo(() => presentKinds(atlas), [atlas]);

  const hubLabel = useCallback(
    (key: string) =>
      key === OTHER_GENRES_KEY ? t("stats.graphOtherGenres") : key === NO_GENRE_KEY ? t("stats.noGenre") : key,
    [t]
  );

  const positionsRef = useRef<Map<string, Vec3>>(new Map());
  const nodeObjectsRef = useRef<Map<string, THREE.Object3D>>(new Map());
  const labelsRef = useRef<Map<string, LabelEntry>>(new Map());
  const labelLayerRef = useRef<THREE.Group | null>(null);
  const legendRef = useRef<HTMLElement>(null);
  const sphereGeoRef = useRef<THREE.SphereGeometry | null>(null);
  const tokensRef = useRef(tokens);
  tokensRef.current = tokens;
  const countsRef = useRef(counts);
  countsRef.current = counts;
  const hubLabelRef = useRef(hubLabel);
  hubLabelRef.current = hubLabel;
  const heightRef = useRef(height);
  heightRef.current = height;
  const highlightRef = useRef<ReturnType<typeof neighbourhood>>(null);
  const layoutKeyRef = useRef<string | null>(null);
  const didFitRef = useRef(false);
  const userMovedRef = useRef(false);

  const graphData = useMemo(() => {
    const maxCount = Math.max(1, ...atlas.hubs.map((h) => h.count));
    const maxEps = Math.max(1, ...atlas.shows.map((s) => s.episodes));
    const nodes: NodeObject<GNode>[] = [];
    for (const h of atlas.hubs) {
      const p = hubPositions.get(h.key) ?? { x: 0, y: 0, z: 0 };
      nodes.push({
        id: hubNodeId(h.key),
        type: "hub",
        key: h.key,
        color: h.color,
        size: scaleSqrt(h.count, maxCount, HUB_MIN_R, HUB_MAX_R),
        x: p.x,
        y: p.y,
        z: p.z,
        fx: p.x,
        fy: p.y,
        fz: p.z,
      });
    }
    for (const s of atlas.shows) {
      const id = showNodeId(s.id);
      const saved = positionsRef.current.get(id);
      const anchor = hubPositions.get(s.hubs[0]) ?? { x: 0, y: 0, z: 0 };
      const jitter = () => (Math.random() - 0.5) * 60;
      nodes.push({
        id,
        type: "show",
        showId: s.id,
        color: s.color,
        size: scaleSqrt(s.episodes, maxEps, SHOW_MIN_SIZE, SHOW_MAX_SIZE),
        ...(saved ?? { x: anchor.x + jitter(), y: anchor.y + jitter(), z: anchor.z + jitter() }),
      });
    }
    const links: GLink[] = atlas.links.map((l) => ({
      source: hubNodeId(l.hub),
      target: showNodeId(l.showId),
      key: linkKey(l.hub, l.showId),
      hub: l.hub,
      showId: l.showId,
      color: hubByKey.get(l.hub)?.color ?? "#888888",
      distance: LINK_MAX_DIST - (LINK_MAX_DIST - LINK_MIN_DIST) * Math.sqrt(l.weight),
    }));
    return { nodes, links };
  }, [atlas, hubPositions, hubByKey]);

  const structuralKey = useMemo(
    () => graphData.links.map((l) => l.key).sort().join(",") + "|" + atlas.hubs.map((h) => h.key).join(","),
    [graphData, atlas]
  );

  const highlight = useMemo(() => neighbourhood(atlas, selection), [atlas, selection]);
  highlightRef.current = highlight;

  // ---- sizing ----
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
  // requestAnimationFrame loop running.
  const hasGraph = seriesList.length > 0;
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    if (active) fg.resumeAnimation();
    else fg.pauseAnimation();
  }, [active, hasGraph]);

  // ---- labels: constant on-screen size, redrawn on theme / filter / language ----
  const fov = useCallback(() => {
    const cam = graphRef.current?.camera() as THREE.PerspectiveCamera | undefined;
    return cam?.fov ?? 50;
  }, []);

  const paintLabel = useCallback(
    (entry: LabelEntry) => {
      const count = countsRef.current.get(entry.hub.key) ?? entry.hub.count;
      const style = labelStyle(entry.hub.count, entry.maxCount);
      const canvas = drawLabel(
        hubLabelRef.current(entry.hub.key),
        style.showCount ? String(count) : "",
        entry.hub.color,
        tokensRef.current,
        style
      );
      const mat = entry.sprite.material;
      mat.map?.dispose();
      const tex = new THREE.CanvasTexture(canvas);
      tex.colorSpace = THREE.SRGBColorSpace;
      mat.map = tex;
      mat.needsUpdate = true;
      const sy = screenSpriteScale(style.px, fov(), heightRef.current);
      entry.sprite.scale.set(sy * (canvas.width / canvas.height), sy, 1);
      entry.h = style.px;
      entry.w = style.px * (canvas.width / canvas.height);
    },
    [fov]
  );

  useEffect(() => {
    for (const entry of labelsRef.current.values()) paintLabel(entry);
  }, [tokens, counts, hubLabel, height, paintLabel]);

  // ---- node objects ----
  const nodeThreeObject = useCallback(
    (node: NodeObject<GNode>) => {
      const id = node.id as string;
      const existing = nodeObjectsRef.current.get(id);
      if (existing) disposeObject(existing, sphereGeoRef.current);
      let obj: THREE.Object3D;
      if (node.type === "hub") {
        if (!sphereGeoRef.current) sphereGeoRef.current = new THREE.SphereGeometry(1, 28, 20);
        const group = new THREE.Group();
        const sphere = new THREE.Mesh(
          sphereGeoRef.current,
          new THREE.MeshLambertMaterial({ color: node.color, transparent: true, emissive: node.color, emissiveIntensity: 0.25 })
        );
        sphere.scale.setScalar(node.size);
        group.add(sphere);
        obj = group;
      } else {
        const show = showById.get(node.showId!);
        const cover = show?.coverUrl && /^(data|file):/.test(show.coverUrl) ? resolveCoverUrl(show.coverUrl) : null;
        const sprite: THREE.Sprite = new THREE.Sprite(
          makeDisc(node.color, cover, (): boolean => nodeObjectsRef.current.get(id) === sprite)
        );
        sprite.scale.set(node.size, node.size, 1);
        obj = sprite;
      }
      const hl = highlightRef.current;
      if (hl && !hl.nodes.has(id)) setObjectOpacity(obj, node.type === "hub" ? DIM_HUB : DIM_SHOW);
      nodeObjectsRef.current.set(id, obj);
      return obj;
    },
    [showById]
  );

  // Drop objects of nodes that left the graph.
  useEffect(() => {
    const ids = new Set(graphData.nodes.map((n) => n.id as string));
    for (const [id, obj] of nodeObjectsRef.current) {
      if (!ids.has(id)) {
        disposeObject(obj, sphereGeoRef.current);
        nodeObjectsRef.current.delete(id);
      }
    }
  }, [graphData]);

  // Hub labels live in their own scene-level group, not inside the node
  // objects: three.js sorts by the parent Group's renderOrder first, so only a
  // separate high-order group reliably draws every label over every disc and
  // link. Hubs are pinned, so each label is placed once, just above its sphere.
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    const scene = fg.scene();
    let layer = labelLayerRef.current;
    if (!layer) {
      layer = new THREE.Group();
      layer.renderOrder = 1000;
      labelLayerRef.current = layer;
    }
    if (layer.parent !== scene) scene.add(layer);
    for (const entry of labelsRef.current.values()) {
      layer.remove(entry.sprite);
      disposeObject(entry.sprite, null);
    }
    labelsRef.current.clear();
    const sizes = new Map(graphData.nodes.filter((n) => n.type === "hub").map((n) => [n.key!, n.size]));
    const maxCount = Math.max(1, ...atlas.hubs.map((h) => h.count));
    for (const hub of atlas.hubs) {
      const p = hubPositions.get(hub.key) ?? { x: 0, y: 0, z: 0 };
      const sprite = new THREE.Sprite(
        new THREE.SpriteMaterial({ transparent: true, depthTest: false, depthWrite: false, sizeAttenuation: false })
      );
      sprite.center.set(0.5, 0);
      sprite.position.set(p.x, p.y + (sizes.get(hub.key) ?? HUB_MIN_R) + LABEL_GAP, p.z);
      // Bigger genres draw last, so if pills still touch the biggest one stays on top.
      sprite.renderOrder = 1000 + hub.count;
      layer.add(sprite);
      const entry: LabelEntry = { sprite, hub, maxCount, w: 0, h: 0, lift: 0 };
      labelsRef.current.set(hub.key, entry);
      paintLabel(entry);
    }
  }, [atlas, hubPositions, graphData, hasGraph, paintLabel]);

  useEffect(() => {
    const objects = nodeObjectsRef.current;
    const labels = labelsRef.current;
    return () => {
      for (const obj of objects.values()) disposeObject(obj, null);
      objects.clear();
      for (const entry of labels.values()) disposeObject(entry.sprite, null);
      labels.clear();
      labelLayerRef.current?.removeFromParent();
      labelLayerRef.current = null;
      sphereGeoRef.current?.dispose();
      sphereGeoRef.current = null;
    };
  }, []);

  // Dim everything outside the selection; dim genres the type filter empties.
  useEffect(() => {
    for (const [id, obj] of nodeObjectsRef.current) {
      const isHub = id.startsWith("genre:");
      const off = highlight !== null && !highlight.nodes.has(id);
      let opacity = 1;
      if (isHub) {
        const empty = (counts.get(id.slice("genre:".length)) ?? 0) === 0;
        opacity = off || empty ? DIM_HUB : 1;
      } else if (off) {
        opacity = DIM_SHOW;
      }
      setObjectOpacity(obj, opacity);
    }
    for (const [key, entry] of labelsRef.current) {
      const off = highlight !== null && !highlight.nodes.has(hubNodeId(key));
      const empty = (counts.get(key) ?? 0) === 0;
      entry.sprite.material.opacity = off || empty ? DIM_LABEL : 1;
    }
  }, [highlight, counts, graphData]);

  // Per-frame label declutter while the graph is visible: project each label
  // anchor to screen px, let declutterLabels lift the smaller genre's pill
  // above any pill it would overlap, and ease towards that so rotating does
  // not make labels jump. The lift is applied through sprite.center (in
  // units of the pill's own height, since the label keeps a fixed px size).
  useEffect(() => {
    if (!active || !hasGraph) return;
    let raf = 0;
    const v = new THREE.Vector3();
    const step = () => {
      raf = requestAnimationFrame(step);
      const cam = graphRef.current?.camera();
      if (!cam || !width || !height) return;
      const items: ScreenLabel[] = [];
      for (const [key, e] of labelsRef.current) {
        if (!e.h) continue;
        v.copy(e.sprite.position).project(cam);
        // Behind the camera: nothing to declutter.
        if (v.z > 1) continue;
        items.push({
          key,
          x: ((v.x + 1) / 2) * width,
          y: ((1 - v.y) / 2) * height,
          w: e.w,
          h: e.h,
          priority: e.hub.count,
        });
      }
      const lifts = declutterLabels(items, LABEL_DECLUTTER_GAP);
      for (const [key, e] of labelsRef.current) {
        const target = lifts.get(key) ?? 0;
        e.lift = reducedMotion ? target : e.lift + (target - e.lift) * LABEL_EASE;
        if (Math.abs(e.lift - target) < 0.2) e.lift = target;
        if (e.h) e.sprite.center.y = -e.lift / e.h;
      }
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [active, hasGraph, width, height, reducedMotion]);

  // ---- forces ----
  useEffect(() => {
    const fg = graphRef.current;
    if (!fg) return;
    // Hubs are pinned, so the centring force would only drag the shows away from them.
    fg.d3Force("center", null);
    const charge = fg.d3Force("charge") as
      | { strength?: (v: number) => void; distanceMax?: (v: number) => void }
      | undefined;
    charge?.strength?.(SHOW_CHARGE);
    charge?.distanceMax?.(CHARGE_DISTANCE_MAX);
    (fg.d3Force("link") as { distance?: (fn: (l: GLink) => number) => void } | undefined)?.distance?.(
      (l: GLink) => l.distance
    );
    // Never on first mount: the engine has no layout until kapsule's debounced
    // first update, and a reheat before that throws in layoutTick ("reading
    // 'tick'") and freezes the render loop.
    if (layoutKeyRef.current !== null && layoutKeyRef.current !== structuralKey) fg.d3ReheatSimulation();
  }, [structuralKey, hasGraph]);

  // Width hidden behind the legend panel (0 when it is a bottom strip on narrow windows).
  const legendOffset = useCallback(() => {
    const el = legendRef.current;
    const box = containerRef.current;
    if (!el || !box || window.matchMedia?.(NARROW_QUERY)?.matches) return 0;
    return Math.max(0, el.getBoundingClientRect().right - box.getBoundingClientRect().left);
  }, []);

  // Shift the projection centre right by half the legend width, so the
  // camera's look-at point (and every fly-to) lands in the middle of the free
  // area instead of under the legend. Raycasting goes through the same
  // projection matrix, so picking stays correct.
  useEffect(() => {
    const cam = graphRef.current?.camera() as THREE.PerspectiveCamera | undefined;
    if (!cam || !width || !height) return;
    const off = legendOffset();
    if (off > 0) cam.setViewOffset(width, height, -off / 2, 0, width, height);
    else cam.clearViewOffset();
  }, [width, height, hasGraph, legendOffset]);

  // Own fit instead of zoomToFit: that one sizes on the vertical field of view
  // only, with a fixed padding, and ignores the legend, which left the cloud
  // at about a third of a wide canvas.
  const fitVisible = useCallback(
    (ms: number) => {
      const fg = graphRef.current;
      if (!fg || !width || !height) return;
      const visible = graphData.nodes.filter((n) => {
        if (n.x === undefined || n.y === undefined || n.z === undefined) return false;
        if (n.type === "hub") return true;
        const s = showById.get(n.showId!);
        return !!s && showMatchesKind(s, kindFilter);
      });
      if (visible.length === 0) return;
      const min = { x: Infinity, y: Infinity, z: Infinity };
      const max = { x: -Infinity, y: -Infinity, z: -Infinity };
      for (const n of visible) {
        min.x = Math.min(min.x, n.x!);
        min.y = Math.min(min.y, n.y!);
        min.z = Math.min(min.z, n.z!);
        max.x = Math.max(max.x, n.x!);
        max.y = Math.max(max.y, n.y!);
        max.z = Math.max(max.z, n.z!);
      }
      const c = { x: (min.x + max.x) / 2, y: (min.y + max.y) / 2, z: (min.z + max.z) / 2 };
      let radius = 0;
      for (const n of visible) {
        radius = Math.max(radius, Math.hypot(n.x! - c.x, n.y! - c.y, n.z! - c.z) + n.size);
      }
      radius += FIT_LABEL_MARGIN;
      const cam = fg.camera() as THREE.PerspectiveCamera;
      const target = (fg.controls() as { target?: THREE.Vector3 }).target ?? new THREE.Vector3();
      const dir = cam.position.clone().sub(target);
      if (dir.lengthSq() < 1e-6) dir.set(0, 0, 1);
      dir.normalize();
      const dist = fitDistance(radius, cam.fov, width - legendOffset(), height);
      fg.cameraPosition(
        { x: c.x + dir.x * dist, y: c.y + dir.y * dist, z: c.z + dir.z * dist },
        c,
        reducedMotion ? 0 : ms
      );
    },
    [graphData, width, height, reducedMotion, showById, kindFilter, legendOffset]
  );

  const handleEngineStop = useCallback(() => {
    for (const n of graphData.nodes) {
      if (n.type === "show" && n.x !== undefined && n.y !== undefined && n.z !== undefined) {
        positionsRef.current.set(n.id as string, { x: n.x, y: n.y, z: n.z });
      }
    }
    const changed = layoutKeyRef.current !== structuralKey;
    layoutKeyRef.current = structuralKey;
    if (!didFitRef.current || (changed && !userMovedRef.current)) {
      didFitRef.current = true;
      fitVisible(600);
    }
  }, [graphData, structuralKey, fitVisible]);

  // ---- gentle idle orbit (never with reduced motion, stops for good once the user moves the camera) ----
  useEffect(() => {
    const controls = graphRef.current?.controls() as
      | {
          autoRotate?: boolean;
          autoRotateSpeed?: number;
          addEventListener?: (e: string, fn: () => void) => void;
          removeEventListener?: (e: string, fn: () => void) => void;
        }
      | undefined;
    if (!controls) return;
    const onStart = () => {
      userMovedRef.current = true;
      controls.autoRotate = false;
    };
    controls.addEventListener?.("start", onStart);
    controls.autoRotateSpeed = AUTO_ROTATE_SPEED;
    controls.autoRotate = active && !reducedMotion && !userMovedRef.current && selection === null;
    return () => controls.removeEventListener?.("start", onStart);
  }, [active, reducedMotion, selection, hasGraph]);

  // ---- camera ----
  const flyTo = useCallback(
    (nodeId: string) => {
      const fg = graphRef.current;
      const node = graphData.nodes.find((n) => n.id === nodeId);
      if (!fg || !node || node.x === undefined || node.y === undefined || node.z === undefined) return;
      const target = { x: node.x, y: node.y, z: node.z };
      const len = Math.hypot(target.x, target.y, target.z);
      // Look from outside the hub sphere, far enough to keep the cluster in frame.
      const distance = node.type === "hub" ? 380 : 170;
      const pos =
        len < 1
          ? { x: target.x, y: target.y, z: target.z + distance }
          : {
              x: target.x * (1 + distance / len),
              y: target.y * (1 + distance / len),
              z: target.z * (1 + distance / len),
            };
      fg.cameraPosition(pos, target, reducedMotion ? 0 : 1000);
    },
    [graphData, reducedMotion]
  );

  const selectGenre = useCallback(
    (key: string) => {
      setSelection((cur) => (cur?.type === "genre" && cur.key === key ? null : { type: "genre", key }));
      flyTo(hubNodeId(key));
    },
    [flyTo]
  );

  const selectShow = useCallback(
    (id: number) => {
      const show = showById.get(id);
      if (show && !showMatchesKind(show, kindFilter)) setKindFilter("all");
      setSelection({ type: "series", id });
      flyTo(showNodeId(id));
    },
    [flyTo, showById, kindFilter]
  );

  const resetView = useCallback(() => {
    setSelection(null);
    fitVisible(800);
  }, [fitVisible]);

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSelection(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active]);
  // System back clears the selection (closes the node card) like Escape.
  useBackLayer(active && selection !== null, () => setSelection(null));

  // ---- search ----
  const results = useMemo(() => searchShows(atlas.shows, query), [atlas, query]);
  const pickResult = (s: Show) => {
    setQuery("");
    setSearchIndex(0);
    selectShow(s.id);
  };
  const onSearchKey = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSearchIndex((i) => Math.min(results.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSearchIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter" && results[searchIndex]) {
      e.preventDefault();
      pickResult(results[searchIndex]);
    } else if (e.key === "Escape") {
      e.stopPropagation();
      setQuery("");
    }
  };

  const nodeLabel = useCallback(
    (n: NodeObject<GNode>) => {
      if (n.type === "hub") {
        const count = countsRef.current.get(n.key!) ?? 0;
        return `<span class="graph-tip"><strong>${escapeHtml(hubLabel(n.key!))}</strong> ${escapeHtml(
          t("stats.graphSeriesCount", { count })
        )}</span>`;
      }
      const s = showById.get(n.showId!);
      if (!s) return "";
      return `<span class="graph-tip"><strong>${escapeHtml(s.title)}</strong> ${escapeHtml(
        t("stats.graphEpisodesCount", { count: s.episodes })
      )}</span>`;
    },
    [hubLabel, showById, t]
  );

  const showVisible = useCallback(
    (id: number) => {
      const s = showById.get(id);
      return !!s && showMatchesKind(s, kindFilter);
    },
    [showById, kindFilter]
  );

  const lineColor = withAlpha(tokens.muted, theme === "light" ? 0.32 : 0.24);
  const lineDim = withAlpha(tokens.muted, 0.05);

  // Stable accessors: a new function identity makes three-forcegraph re-run its
  // whole node/link update, which would happen on every search keystroke.
  const nodeVisibility = useCallback((n: GNode) => n.type === "hub" || showVisible(n.showId!), [showVisible]);
  const linkVisibility = useCallback((l: GLink) => showVisible(l.showId), [showVisible]);
  const linkColor = useCallback(
    (l: GLink) => {
      if (!highlight) return lineColor;
      return highlight.links.has(l.key) ? withAlpha(l.color, 0.9) : lineDim;
    },
    [highlight, lineColor, lineDim]
  );
  const linkWidth = useCallback((l: GLink) => (highlight?.links.has(l.key) ? 1.4 : 0), [highlight]);

  const selectedShow = selection?.type === "series" ? showById.get(selection.id) ?? null : null;
  const selectedHub = selection?.type === "genre" ? hubByKey.get(selection.key) ?? null : null;
  const listboxId = "graph-search-results";

  return (
    <div ref={containerRef} className="graph-atlas graph-clip" onContextMenu={(e) => e.preventDefault()}>
      {seriesList.length === 0 ? (
        <div className="empty">{t("stats.graphEmpty")}</div>
      ) : (
        <>
          <div
            className="graph-atlas-canvas"
            role="img"
            aria-label={t("stats.graphAria", { series: atlas.shows.length, genres: atlas.hubs.length })}
          >
            <ForceGraph3D<GNode, GLink>
              ref={graphRef}
              width={width || undefined}
              height={height}
              graphData={graphData}
              backgroundColor="rgba(0,0,0,0)"
              showNavInfo={false}
              controlType="orbit"
              enableNodeDrag={false}
              nodeThreeObject={nodeThreeObject}
              nodeLabel={nodeLabel}
              nodeVisibility={nodeVisibility}
              linkVisibility={linkVisibility}
              linkOpacity={1}
              linkColor={linkColor}
              linkWidth={linkWidth}
              cooldownTicks={COOLDOWN_TICKS}
              onEngineStop={handleEngineStop}
              onNodeClick={(n) => {
                if (n.type === "hub") selectGenre(n.key!);
                else selectShow(n.showId!);
              }}
              onBackgroundClick={() => setSelection(null)}
            />
          </div>

          <div className="graph-atlas-toolbar">
            <div className="graph-search">
              <input
                type="search"
                className="graph-search-input"
                value={query}
                placeholder={t("stats.graphSearchPlaceholder")}
                aria-label={t("stats.graphSearchAria")}
                role="combobox"
                aria-expanded={query.trim() !== ""}
                aria-controls={listboxId}
                aria-autocomplete="list"
                aria-activedescendant={results[searchIndex] ? `${listboxId}-${results[searchIndex].id}` : undefined}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setSearchIndex(0);
                }}
                onKeyDown={onSearchKey}
              />
              {query.trim() !== "" && (
                <ul className="graph-search-results" id={listboxId} role="listbox">
                  {results.length === 0 ? (
                    <li className="graph-search-empty">{t("stats.graphSearchEmpty")}</li>
                  ) : (
                    results.map((s, i) => (
                      <li
                        key={s.id}
                        id={`${listboxId}-${s.id}`}
                        role="option"
                        aria-selected={i === searchIndex}
                        className={i === searchIndex ? "graph-search-option is-active" : "graph-search-option"}
                        onMouseDown={(e) => {
                          e.preventDefault();
                          pickResult(s);
                        }}
                      >
                        <span className="graph-swatch" style={{ background: s.color }} />
                        <span className="graph-search-title">{s.title}</span>
                      </li>
                    ))
                  )}
                </ul>
              )}
            </div>

            {kinds.length > 1 && (
              <div className="graph-kinds" role="group" aria-label={t("stats.graphKindFilterAria")}>
                {(["all", ...kinds] as KindFilter[]).map((k) => (
                  <button
                    key={k}
                    type="button"
                    className="graph-chip"
                    aria-pressed={kindFilter === k}
                    onClick={() => setKindFilter(k)}
                  >
                    {k === "all" ? t("stats.graphKindAll") : t(KIND_LABEL[k])}
                  </button>
                ))}
              </div>
            )}

            <button type="button" className="graph-btn" onClick={resetView}>
              {t("stats.graphResetView")}
            </button>
          </div>

          <nav ref={legendRef} className="graph-legend" aria-label={t("stats.graphLegendAria")}>
            <h3 className="graph-legend-title">{t("stats.graphLegendTitle")}</h3>
            <p className="graph-legend-help">{t("stats.graphLegendHelp")}</p>
            <p className="graph-legend-help graph-legend-scope">{t("stats.graphLegendScope")}</p>
            <ul className="graph-legend-list">
              {atlas.hubs.map((h) => {
                const count = counts.get(h.key) ?? 0;
                return (
                  <li key={h.key}>
                    <button
                      type="button"
                      className="graph-legend-item"
                      aria-pressed={selection?.type === "genre" && selection.key === h.key}
                      disabled={count === 0}
                      onClick={() => selectGenre(h.key)}
                    >
                      <span className="graph-swatch" style={{ background: h.color }} />
                      <span className="graph-legend-name">{hubLabel(h.key)}</span>
                      <span className="graph-legend-count">{count}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </nav>

          {selectedShow && (
            <ShowCard
              show={selectedShow}
              hubByKey={hubByKey}
              hubLabel={hubLabel}
              onClose={() => setSelection(null)}
              onGenre={selectGenre}
              onCenter={() => flyTo(showNodeId(selectedShow.id))}
              onOpen={onOpenSeries ? () => onOpenSeries(selectedShow.id) : undefined}
            />
          )}

          {selectedHub && (
            <section className="graph-card" aria-label={hubLabel(selectedHub.key)}>
              <header className="graph-card-header">
                <h3 className="graph-card-title">
                  <span className="graph-swatch" style={{ background: selectedHub.color }} />
                  {hubLabel(selectedHub.key)}
                </h3>
                <button
                  type="button"
                  className="graph-card-close"
                  onClick={() => setSelection(null)}
                  aria-label={t("stats.graphClose")}
                >
                  ✕
                </button>
              </header>
              <p className="graph-card-meta">
                {t("stats.graphSeriesCount", { count: counts.get(selectedHub.key) ?? 0 })},{" "}
                {t("stats.graphEpisodesCount", { count: selectedHub.episodes })}
              </p>
              {selectedHub.folded.length > 0 && (
                <p className="graph-card-note">{selectedHub.folded.join(", ")}</p>
              )}
              <h4 className="graph-card-subtitle">{t("stats.graphTopShows")}</h4>
              <ul className="graph-card-list">
                {topShowsOf(atlas, selectedHub.key, kindFilter).map((s) => (
                  <li key={s.id}>
                    <button type="button" className="graph-card-link" onClick={() => selectShow(s.id)}>
                      <span className="graph-card-link-title">{s.title}</span>
                      <span className="graph-legend-count">{s.episodes}</span>
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}

          <p className="graph-hint">{t("stats.graphHint")}</p>
        </>
      )}
    </div>
  );
}

function ShowCard({
  show,
  hubByKey,
  hubLabel,
  onClose,
  onGenre,
  onCenter,
  onOpen,
}: {
  show: Show;
  hubByKey: Map<string, Hub>;
  hubLabel: (key: string) => string;
  onClose: () => void;
  onGenre: (key: string) => void;
  onCenter: () => void;
  onOpen?: () => void;
}) {
  const t = useT();
  const cover = resolveCoverUrl(show.coverUrl);
  const [failed, markFailed] = useCoverFailure(cover);
  const other = hubByKey.get(OTHER_GENRES_KEY);
  return (
    <section className="graph-card" aria-label={show.title}>
      <header className="graph-card-header">
        {cover && !failed && <img className="graph-card-cover" src={cover} alt="" onError={markFailed} />}
        <h3 className="graph-card-title">{show.title}</h3>
        <button type="button" className="graph-card-close" onClick={onClose} aria-label={t("stats.graphClose")}>
          ✕
        </button>
      </header>
      <p className="graph-card-meta">
        {t(KIND_LABEL[show.kind])}, {t("stats.graphEpisodesCount", { count: show.episodes })}
      </p>
      {show.hubs.length > 0 && (
        <div className="graph-card-genres" aria-label={t("stats.graphGenres")}>
          {show.hubs.map((key) => {
            const hub = hubByKey.get(key) ?? other;
            return (
              <button key={key} type="button" className="graph-chip graph-chip-genre" onClick={() => onGenre(key)}>
                <span className="graph-swatch" style={{ background: hub?.color }} />
                {hubLabel(key)}
              </button>
            );
          })}
        </div>
      )}
      <div className="graph-card-actions">
        {onOpen && (
          <button type="button" className="graph-btn graph-btn-primary" onClick={onOpen}>
            {t("stats.graphOpenSeries")}
          </button>
        )}
        <button type="button" className="graph-btn" onClick={onCenter}>
          {t("stats.graphCenterCamera")}
        </button>
      </div>
    </section>
  );
}
