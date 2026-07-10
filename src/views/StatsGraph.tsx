import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ForceGraph3D, { type ForceGraphMethods, type NodeObject } from "react-force-graph-3d";
import * as THREE from "three";
import { categoryColor, SIN_GENERO_LABEL } from "../lib/categoryColor";
import type { SeriesGraphNode } from "../types";

const ROOT_COLOR = "#e9ecef";
const HUB_MIN_R = 6;
const HUB_MAX_R = 22;
const SERIES_SPRITE_SIZE = 18;
const MIN_HEIGHT = 500;
const BOTTOM_MARGIN = 24;
const CHARGE_STRENGTH = -260;
const LINK_DISTANCE = 90;
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

function buildGraphData(seriesList: SeriesGraphNode[]): { nodes: GNode[]; links: GLink[] } {
  const nodes: GNode[] = [{ id: "root", kind: "root", label: "Seguidas" }];
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

function fallbackCircleCanvas(color: string): HTMLCanvasElement {
  const canvas = document.createElement("canvas");
  canvas.width = 64;
  canvas.height = 64;
  const ctx = canvas.getContext("2d")!;
  ctx.beginPath();
  ctx.arc(32, 32, 30, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  return canvas;
}

export function StatsGraph({
  seriesList,
  active,
}: {
  seriesList: SeriesGraphNode[];
  active: boolean;
}) {
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

  const { nodes, links } = useMemo(() => {
    const built = buildGraphData(seriesList);
    for (const n of built.nodes) {
      const saved = positionsRef.current.get(n.id);
      if (saved) Object.assign(n as NodeObject<GNode>, saved);
    }
    return built;
  }, [seriesList]);
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
  }, [nodes, structuralKey]);

  const handleNodeThreeObject = useCallback(
    (node: NodeObject<GNode>) => {
      const existing = nodeObjectsRef.current.get(node.id as string);
      if (existing) disposeNodeObject(existing);
      let obj: THREE.Object3D;
      try {
        if (node.kind === "series") {
          const hasCover = !!node.coverUrl && node.coverUrl.startsWith("data:");
          const texture = hasCover
            ? new THREE.TextureLoader().load(node.coverUrl as string)
            : new THREE.CanvasTexture(fallbackCircleCanvas(node.color ?? categoryColor(SIN_GENERO_LABEL)));
          const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: texture, transparent: true }));
          sprite.scale.set(SERIES_SPRITE_SIZE, SERIES_SPRITE_SIZE, 1);
          obj = sprite;
        } else {
          const color = node.kind === "root" ? ROOT_COLOR : node.color ?? "#4aa8ff";
          const radius = node.kind === "root" ? HUB_MAX_R * 1.1 : hubRadius(node.count ?? 0, maxHubCount);
          // MeshBasicMaterial (unlit) rather than MeshLambertMaterial: a
          // lit material renders pure black without a scene light hitting
          // it, and this graph doesn't add one (react-force-graph-3d's
          // default lighting isn't guaranteed) — flat, always-visible
          // color is also just the right look for a data-viz sphere, not
          // realistic shading.
          obj = new THREE.Mesh(
            new THREE.SphereGeometry(radius, 16, 16),
            new THREE.MeshBasicMaterial({ color })
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
    [maxHubCount]
  );

  // The ref-bearing div below must always mount, even while seriesList is
  // still empty (the normal state on first render — data loads async, so
  // this is briefly true on every real app launch, not just the true-empty
  // case). The one-shot mount effect above measures containerRef.current
  // exactly once; if the empty case returned a differently-shaped tree
  // instead of this same div, the ref would be null on that first pass and
  // never get a second chance to attach, permanently starving the graph.
  return (
    <div ref={containerRef} style={{ borderRadius: "var(--radius-sm)", overflow: "hidden" }}>
      {seriesList.length === 0 ? (
        <div className="empty">Sin series seguidas todavía.</div>
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
