import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { TreemapNode, VolumeInfo } from "../api/types";
import { formatBytes, formatCount, relativePath, truncateMiddle } from "../format";
import { canReveal, reveal } from "../reveal";
import { selectionState, type SelectedItem, type Selection, type SelectionState } from "../selection";
import { frameworkColor } from "../ui/frameworkColors";
import { HEADER_H, SWATCH, layoutBlocks, toned, type Block } from "../ui/treemapLayout";

const CHAR_W = 6.3; // average glyph width at 11px Segoe UI, slightly pessimistic so text never overflows
const PAD = 6;

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

function samePath(a: string, b: string): boolean {
  const n = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  return n(a) === n(b);
}

function isDriveRoot(p: string): boolean {
  return /^[A-Za-z]:[\\/]*$/.test(p.trim());
}

function fit(text: string, width: number): string {
  const max = Math.floor((width - PAD * 2) / CHAR_W);
  if (max < 3) return "";
  return text.length <= max ? text : `${text.slice(0, max - 1)}…`;
}

function blockName(b: Block, volume: VolumeInfo | null): string {
  return b.free ? `Free space on ${volume?.mount ?? ""}` : (b.node?.name ?? "");
}

interface Crumb {
  label: string;
  path: string;
}

function crumbsFor(root: string, path: string): Crumb[] {
  const out: Crumb[] = [{ label: root, path: root }];
  const rel = relativePath(path, root);
  if (rel === "." || rel === path) return out;
  const base = root.replace(/[\\/]+$/, "");
  let acc = base;
  for (const part of rel.split("\\").filter(Boolean)) {
    acc = `${acc}\\${part}`;
    out.push({ label: part, path: acc });
  }
  return out;
}

interface Hover {
  block: Block;
  x: number;
  y: number;
}

interface TreemapProps {
  root: string;
  selection: Selection;
  onToggle: (item: SelectedItem) => void;
}

export function Treemap({ root, selection, onToggle }: TreemapProps) {
  const [path, setPath] = useState(root);
  const [node, setNode] = useState<TreemapNode | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [volume, setVolume] = useState<VolumeInfo | null>(null);
  const [showFree, setShowFree] = useState(isDriveRoot(root));
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [hover, setHover] = useState<Hover | null>(null);
  const [selectMode, setSelectMode] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const requestId = useRef(0);

  useEffect(() => {
    const id = ++requestId.current;
    setLoading(true);
    api
      .getTreemap(path, 2)
      .then((n) => {
        if (id !== requestId.current) return;
        setNode(n);
        setError(null);
        setLoading(false);
      })
      .catch((e: unknown) => {
        // The previous map stays on screen; only the message appears.
        if (id !== requestId.current) return;
        setError(message(e));
        setLoading(false);
      });
  }, [path]);

  useEffect(() => {
    let cancelled = false;
    api
      .getVolume()
      .then((v) => {
        if (!cancelled) setVolume(v);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    // Measure once up front; the observer then follows later resizes.
    setSize({ w: Math.floor(el.clientWidth), h: Math.floor(el.clientHeight) });
    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setSize({ w: Math.floor(width), h: Math.floor(height) });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const atTop = node !== null && samePath(node.path, root);
  const free = atTop && showFree && volume !== null;

  const blocks = useMemo(
    () => (node ? layoutBlocks(node, volume, free, size.w, size.h) : []),
    [node, volume, free, size.w, size.h],
  );

  const legend = useMemo(() => {
    const labels = new Set<string>();
    let regen = false;
    let other = false;
    let freeShown = false;
    for (const b of blocks) {
      if (b.free) freeShown = true;
      else if (b.node?.label) labels.add(b.node.label);
      else other = true;
      if (b.regen) regen = true;
    }
    return { labels: [...labels].sort(), regen, other, freeShown };
  }, [blocks]);

  const drill = useCallback((b: Block) => {
    if (b.free || !b.node || b.node.kind !== "dir") return;
    setHover(null);
    setPath(b.node.path);
  }, []);

  // Not selectable: "others" (its path is the parent's), free space, and the folder being viewed.
  const selectable = (b: Block): boolean =>
    !b.free && b.node !== null && b.node.kind !== "others" && !samePath(b.node.path, path) && !samePath(b.node.path, root);
  const stateOf = (b: Block): SelectionState => (b.node && b.node.kind !== "others" ? selectionState(selection, b.node.path) : null);
  const pick = (b: Block) => {
    const n = b.node;
    if (!n || !selectable(b)) return;
    onToggle({
      path: n.path,
      name: n.name,
      kind: n.kind === "file" ? "file" : "dir",
      allocated: n.allocated,
      projectPath: null,
      projectLabel: n.label,
      source: "map",
    });
  };

  const blockAt = (target: EventTarget): Block | null => {
    const el = (target as Element).closest?.("[data-i]");
    const i = el ? Number(el.getAttribute("data-i")) : NaN;
    return Number.isNaN(i) ? null : (blocks[i] ?? null);
  };

  const onMove = (e: React.MouseEvent) => {
    const block = blockAt(e.target);
    const stage = stageRef.current;
    if (!block || !stage) {
      setHover(null);
      return;
    }
    const r = stage.getBoundingClientRect();
    setHover({ block, x: e.clientX - r.left, y: e.clientY - r.top });
  };

  const crumbs = crumbsFor(root, path);
  const empty = node !== null && (node.children?.length ?? 0) === 0;

  return (
    <div className="map">
      <div className="map-bar">
        <nav className="crumbs" aria-label="Folder path">
          {crumbs.map((c, i) => {
            const last = i === crumbs.length - 1;
            return (
              <span key={c.path} className="crumb-item">
                {i > 0 && <span className="crumb-sep" aria-hidden="true">/</span>}
                {last ? (
                  <span className="crumb crumb-current" title={c.path}>{truncateMiddle(c.label, 48)}</span>
                ) : (
                  <button className="crumb" title={c.path} onClick={() => setPath(c.path)}>
                    {truncateMiddle(c.label, 48)}
                  </button>
                )}
              </span>
            );
          })}
        </nav>
        <div className="map-actions">
          <label className={`toggle ${volume ? "" : "toggle-off"}`} title={atTop ? undefined : "Free space is only shown at the top level"}>
            <input type="checkbox" checked={showFree} disabled={!volume} onChange={(e) => setShowFree(e.target.checked)} />
            Show free space
          </label>
          <button
            className={`btn btn-small btn-toggle${selectMode ? " btn-pressed" : ""}`}
            aria-pressed={selectMode}
            title="Click blocks to select them; double-click a folder to open it"
            onClick={() => setSelectMode((m) => !m)}
          >
            Select
          </button>
          {canReveal && (
            <button className="btn btn-small" onClick={() => reveal(path)}>
              Show in Explorer
            </button>
          )}
        </div>
      </div>

      {error && <div className="banner banner-error" role="alert">{error}</div>}

      <div className={`map-stage ${loading ? "map-loading" : ""}`} ref={stageRef} aria-busy={loading}>
        {empty ? (
          <div className="empty">
            <h2>This folder is empty</h2>
            <p className="muted">Nothing here takes up space on disk.</p>
          </div>
        ) : (
          size.w > 0 && (
            <svg
              width={size.w}
              height={size.h}
              role="group"
              aria-label="Treemap of folder sizes"
              onMouseMove={onMove}
              onMouseLeave={() => setHover(null)}
              onClick={(e) => {
                const b = blockAt(e.target);
                if (!b) return;
                if (selectMode) pick(b);
                else drill(b);
              }}
              // The two clicks of a double-click cancel out, so only the drill remains.
              onDoubleClick={(e) => {
                const b = selectMode ? blockAt(e.target) : null;
                if (b) drill(b);
              }}
            >
              <defs>
                <pattern id="tm-stripe" width="7" height="7" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                  <rect width="7" height="7" fill="none" />
                  <rect width="2.5" height="7" fill="rgb(0 0 0 / 0.2)" />
                </pattern>
                <pattern id="tm-dots" width="6" height="6" patternUnits="userSpaceOnUse">
                  <circle cx="3" cy="3" r="0.9" fill="rgb(0 0 0 / 0.16)" />
                </pattern>
              </defs>
              {blocks.map((b) => (
                <BlockView
                  key={`${b.node?.path ?? "free"}|${b.node?.kind ?? ""}|${b.id}`}
                  b={b}
                  volume={volume}
                  hovered={hover?.block.id === b.id}
                  sel={stateOf(b)}
                  pick={selectMode && selectable(b)}
                />
              ))}
            </svg>
          )
        )}
        {hover && !empty && <Tooltip hover={hover} volume={volume} stage={size} sel={stateOf(hover.block)} />}
      </div>

      <Legend legend={legend} volume={volume} />
    </div>
  );
}

function BlockView({
  b,
  volume,
  hovered,
  sel,
  pick,
}: {
  b: Block;
  volume: VolumeInfo | null;
  hovered: boolean;
  sel: SelectionState;
  pick: boolean;
}) {
  const n = b.node;
  const clickable = !b.free && n?.kind === "dir";
  const cls = `tm-block${clickable || pick ? " tm-click" : ""}`;
  const name = blockName(b, volume);
  const sizeText = formatBytes(b.free ? (volume?.free ?? 0) : (n?.allocated ?? 0));

  if (b.free) {
    return (
      <g data-i={b.id} className="tm-free">
        <rect x={b.x} y={b.y} width={b.w} height={b.h} className="tm-free-rect" />
        {b.w >= 56 && b.h >= 22 && (
          <text x={b.x + PAD} y={b.y + 15} className="tm-free-text">{fit(name, b.w)}</text>
        )}
        {b.w >= 56 && b.h >= 38 && (
          <text x={b.x + PAD} y={b.y + 29} className="tm-free-text tm-sub">{fit(sizeText, b.w)}</text>
        )}
      </g>
    );
  }

  const headerH = b.container ? HEADER_H : 0;
  const showName = b.container ? b.w >= 40 : b.w >= 44 && b.h >= 20;
  const showSize = b.container ? b.w >= 110 : b.h >= 36 && b.w >= 56;
  const sizeLabel = formatBytes(n?.allocated ?? 0);
  // Nested blocks are drawn as plain leaves; only containers carry a header strip.
  return (
    <g data-i={b.id} className={cls} data-hover={hovered || undefined}>
      <rect x={b.x} y={b.y} width={b.w} height={b.h} fill={b.fill} />
      {b.container && <rect x={b.x} y={b.y} width={b.w} height={headerH} fill={b.headerFill} />}
      {b.regen && <rect x={b.x} y={b.y} width={b.w} height={b.h} fill="url(#tm-stripe)" pointerEvents="none" />}
      {n?.kind === "others" && <rect x={b.x} y={b.y} width={b.w} height={b.h} fill="url(#tm-dots)" pointerEvents="none" />}
      {sel === "covered" && <rect x={b.x} y={b.y} width={b.w} height={b.h} className="tm-cov" pointerEvents="none" />}
      {sel === "selected" && (
        <>
          <rect x={b.x + 3} y={b.y + 3} width={Math.max(0, b.w - 6)} height={Math.max(0, b.h - 6)} className="tm-sel-halo" pointerEvents="none" />
          <rect x={b.x + 1} y={b.y + 1} width={Math.max(0, b.w - 2)} height={Math.max(0, b.h - 2)} className="tm-sel" pointerEvents="none" />
          {b.w >= 30 && b.h >= 30 && (
            <g pointerEvents="none">
              <circle cx={b.x + b.w - 14} cy={b.y + 14} r={8} className="tm-badge" />
              <path d={`M ${b.x + b.w - 18} ${b.y + 14} l 3 3 l 5 -6`} className="tm-badge-check" />
            </g>
          )}
        </>
      )}
      {hovered && <rect x={b.x + 0.5} y={b.y + 0.5} width={Math.max(0, b.w - 1)} height={Math.max(0, b.h - 1)} className="tm-outline" pointerEvents="none" />}
      {showName && (
        <text x={b.x + PAD} y={b.y + (b.container ? 13 : 15)} fill={b.text} className="tm-text" pointerEvents="none">
          {fit(name, b.container && showSize ? b.w - 64 : b.w)}
        </text>
      )}
      {showSize && b.container && (
        <text x={b.x + b.w - PAD} y={b.y + 13} fill={b.text} className="tm-text tm-sub" textAnchor="end" pointerEvents="none">
          {sizeLabel}
        </text>
      )}
      {showSize && !b.container && (
        <text x={b.x + PAD} y={b.y + 29} fill={b.text} className="tm-text tm-sub" pointerEvents="none">
          {fit(sizeLabel, b.w)}
        </text>
      )}
    </g>
  );
}

function Tooltip({
  hover,
  volume,
  stage,
  sel,
}: {
  hover: Hover;
  volume: VolumeInfo | null;
  stage: { w: number; h: number };
  sel: SelectionState;
}) {
  const { block: b } = hover;
  const n = b.node;
  const ref = useRef<HTMLDivElement>(null);
  const [dims, setDims] = useState({ w: 280, h: 120 });
  useEffect(() => {
    const el = ref.current;
    if (el) setDims({ w: el.offsetWidth, h: el.offsetHeight });
  }, [b.id]);
  // Flip to the other side of the cursor near the right and bottom edges.
  const left = hover.x + 14 + dims.w > stage.w ? Math.max(0, hover.x - 14 - dims.w) : hover.x + 14;
  const top = hover.y + 14 + dims.h > stage.h ? Math.max(0, hover.y - 14 - dims.h) : hover.y + 14;

  return (
    <div className="tm-tip" ref={ref} style={{ left, top }} role="tooltip">
      {b.free ? (
        <>
          <div className="tm-tip-name">Free space on {volume?.mount}</div>
          <div className="num">{formatBytes(volume?.free ?? 0)} free of {formatBytes(volume?.total ?? 0)}</div>
        </>
      ) : (
        n && (
          <>
            <div className="tm-tip-name">{n.name}</div>
            <div className="tm-tip-path muted small">{n.kind === "others" ? `In ${n.path}` : n.path}</div>
            <div className="num">
              {formatBytes(n.allocated)} on disk
              <span className="muted"> · {formatCount(n.fileCount)} {n.fileCount === 1 ? "file" : "files"}</span>
            </div>
            {n.label && (
              <div className="small">
                <span className="tm-dot" style={{ background: frameworkColor(n.label) }} />
                {n.label}{n.projectRoot ? " project root" : " project"}
              </div>
            )}
            {sel && <div className="small tm-selected">{sel === "selected" ? "Selected" : "Included in a selected folder"}</div>}
            {(n.regenerable || b.regen) && <div className="small tm-regen">{n.regenerable ? "Regenerable" : "Inside a regenerable folder"}</div>}
          </>
        )
      )}
    </div>
  );
}

function Legend({
  legend,
  volume,
}: {
  legend: { labels: string[]; regen: boolean; other: boolean; freeShown: boolean };
  volume: VolumeInfo | null;
}) {
  return (
    <div className="legend" aria-label="Legend">
      {legend.labels.map((l) => (
        <span key={l} className="legend-item">
          <span className="swatch" style={{ background: toned(l) }} />
          {l}
        </span>
      ))}
      {legend.regen && (
        <span className="legend-item">
          <span className="swatch swatch-stripe" />
          Regenerable
        </span>
      )}
      {legend.other && (
        <span className="legend-item">
          <span className="swatch" style={{ background: SWATCH.folder }} />
          Other files
        </span>
      )}
      {legend.freeShown && (
        <span className="legend-item">
          <span className="swatch swatch-free" />
          Free space{volume ? ` on ${volume.mount}` : ""}
        </span>
      )}
    </div>
  );
}
