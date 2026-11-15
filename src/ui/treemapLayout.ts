import { hierarchy, treemap, treemapSquarify } from "d3-hierarchy";
import type { TreemapNode, VolumeInfo } from "../api/types";
import { frameworkColor } from "./frameworkColors";

export const HEADER_H = 18;
const HEADER_MIN_W = 70;
const HEADER_MIN_H = 48;

export interface Block {
  id: number;
  /** 1 = direct child of the current folder, 2 = nested inside one. */
  depth: 1 | 2;
  x: number;
  y: number;
  w: number;
  h: number;
  /** null for the free-space block. */
  node: TreemapNode | null;
  free: boolean;
  /** Folder with nested children drawn inside it and a header strip. */
  container: boolean;
  /** Stripe overlay: the block is regenerable, or sits inside a regenerable folder. */
  regen: boolean;
  fill: string;
  headerFill: string;
  text: string;
}

interface Datum {
  node: TreemapNode | null;
  free: boolean;
  value: number;
  regen: boolean;
  children?: Datum[];
}

function toDatum(n: TreemapNode, inheritedRegen: boolean): Datum {
  const regen = inheritedRegen || n.regenerable;
  const kids = n.children?.filter((c) => c.allocated > 0) ?? [];
  const sum = kids.reduce((a, c) => a + c.allocated, 0);
  // A folder whose listed children add up to nothing is drawn as a plain block.
  if (kids.length > 0 && sum > 0) {
    return { node: n, free: false, value: 0, regen, children: kids.map((c) => toDatum(c, regen)) };
  }
  return { node: n, free: false, value: n.allocated, regen };
}

// ---- colors ----

const GRAY = "#8a8f99";
const FOLDER = "#939aa7";
const FILE = "#b7bcc6";
const OTHERS = "#a9aeb8";

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

function mix(a: string, b: string, t: number): string {
  const [ar, ag, ab] = hexToRgb(a);
  const [br, bg, bb] = hexToRgb(b);
  const c = (x: number, y: number) =>
    Math.round(x + (y - x) * t)
      .toString(16)
      .padStart(2, "0");
  return `#${c(ar, br)}${c(ag, bg)}${c(ab, bb)}`;
}

function luminance(hex: string): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Picks whichever of the two text colors has the better contrast against `bg`. */
export function textOn(bg: string): string {
  const l = luminance(bg);
  const dark = "#171a20";
  const light = "#f6f7f9";
  const contrastDark = (l + 0.05) / (luminance(dark) + 0.05);
  const contrastLight = (luminance(light) + 0.05) / (l + 0.05);
  return contrastDark >= contrastLight ? dark : light;
}

/** Framework colors are loud at full strength; pull them slightly toward gray for large areas. */
export function toned(label: string): string {
  return mix(frameworkColor(label), GRAY, 0.18);
}

export const SWATCH = { folder: FOLDER, file: FILE, others: OTHERS };

function baseFill(n: TreemapNode): string {
  if (n.label) return toned(n.label);
  if (n.kind === "others") return OTHERS;
  return n.kind === "file" ? FILE : FOLDER;
}

// ---- layout ----

export function hasHeader(w: number, h: number): boolean {
  return w >= HEADER_MIN_W && h >= HEADER_MIN_H;
}

export function layoutBlocks(
  current: TreemapNode,
  volume: VolumeInfo | null,
  showFree: boolean,
  width: number,
  height: number,
): Block[] {
  if (width < 2 || height < 2) return [];
  const rootRegen = current.regenerable;
  const kids = (current.children ?? []).filter((c) => c.allocated > 0);
  const top: Datum[] = kids.map((c) => toDatum(c, rootRegen));
  if (showFree && volume && volume.free > 0) {
    top.push({ node: null, free: true, value: volume.free, regen: false });
  }
  if (top.length === 0) return [];

  const tree = hierarchy<Datum>({ node: current, free: false, value: 0, regen: false, children: top }, (d) => d.children)
    .sum((d) => (d.children ? 0 : d.value))
    .sort((a, b) => (b.value ?? 0) - (a.value ?? 0));

  const root = treemap<Datum>()
    .tile(treemapSquarify)
    .size([width, height])
    .paddingInner(1)
    .paddingOuter(0)
    // Nested folders get a header strip (when big enough) and a 1px rim so children read as inside.
    .paddingTop((n) => (n.depth === 1 && n.children && hasHeader(n.x1 - n.x0, n.y1 - n.y0) ? HEADER_H : n.depth === 1 && n.children ? 1 : 0))
    .paddingLeft((n) => (n.depth === 1 && n.children ? 1 : 0))
    .paddingRight((n) => (n.depth === 1 && n.children ? 1 : 0))
    .paddingBottom((n) => (n.depth === 1 && n.children ? 1 : 0))(tree);

  const blocks: Block[] = [];
  let id = 0;
  for (const n of root.descendants()) {
    if (n.depth === 0) continue;
    const parent = n.parent;
    // A folder too small for a header is drawn as one block; its children would be unreadable slivers.
    if (n.depth === 2 && parent && !hasHeader(parent.x1 - parent.x0, parent.y1 - parent.y0)) continue;
    const w = n.x1 - n.x0;
    const h = n.y1 - n.y0;
    if (w < 0.5 || h < 0.5) continue;
    const d = n.data;
    const container = n.depth === 1 && !!n.children && hasHeader(w, h);
    let fill: string;
    if (d.free) fill = "none";
    else fill = baseFill(d.node as TreemapNode);
    const headerFill = d.free ? "none" : mix(fill, "#000000", 0.22);
    const backdrop = container ? mix(fill, "#000000", 0.4) : fill;
    blocks.push({
      id: id++,
      depth: n.depth as 1 | 2,
      x: n.x0,
      y: n.y0,
      w,
      h,
      node: d.node,
      free: d.free,
      container,
      regen: d.regen,
      fill: backdrop,
      headerFill,
      text: d.free ? "" : textOn(container ? headerFill : fill),
    });
  }
  return blocks;
}
