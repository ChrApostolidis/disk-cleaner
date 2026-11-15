import type { Project } from "../api/types";
import { formatAge, formatBytes, relativePath } from "../format";
import { frameworkColor } from "../ui/frameworkColors";
import { canReveal, reveal } from "../reveal";

interface Props {
  project: Project;
  parentName: string | null;
  root: string;
}

export function ProjectCard({ project: p, parentName, root }: Props) {
  const color = frameworkColor(p.label);
  return (
    <article className="card" style={{ borderTopColor: color }}>
      <div className="card-head">
        <span className="badge" style={{ color, background: `color-mix(in srgb, ${color} 14%, transparent)` }}>
          {p.label}
        </span>
        {p.version && <span className="muted num small">v{p.version}</span>}
      </div>
      <h3 className="card-title" title={p.displayName}>
        {p.displayName}
      </h3>
      <div className="card-path muted small" title={p.path}>
        {relativePath(p.path, root)}
      </div>
      {parentName && <div className="small muted">inside {parentName}</div>}

      <div className="card-sizes">
        <div>
          <div className="stat-label">Reclaimable</div>
          <div className="reclaim num">{formatBytes(p.reclaimable)}</div>
        </div>
        <div className="right">
          <div className="stat-label">Size on disk</div>
          <div className="num">{formatBytes(p.totalAllocated)}</div>
        </div>
      </div>

      {p.regenerable.length > 0 && (
        <div className="folder-chips">
          {p.regenerable.map((f) => (
            <span className="folder-chip" key={f.path} title={f.path}>
              {f.name} <span className="num">{formatBytes(f.allocated)}</span>
            </span>
          ))}
        </div>
      )}

      <div className="card-foot small">
        <span className="muted">Active {formatAge(p.lastActivity)}</span>
        {p.hasLockfile ? (
          <span className="muted">{p.packageManager ?? "lockfile"}</span>
        ) : (
          <span className="warn">no lockfile</span>
        )}
      </div>
      {canReveal && (
        <button className="btn btn-small" onClick={() => reveal(p.path)}>
          Show in Explorer
        </button>
      )}
    </article>
  );
}
