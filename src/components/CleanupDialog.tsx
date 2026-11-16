import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { CleanupDone, CleanupPreview, CleanupProgress } from "../api/types";
import { baseName, formatBytes, relativePath, truncateMiddle } from "../format";
import type { ReinstallState } from "../useReinstalls";
import { Modal } from "./Modal";
import { ReinstallLog } from "./ReinstallLog";

interface Props {
  paths: string[];
  root: string;
  reinstalls: Record<string, ReinstallState>;
  onReinstall: (projectPath: string) => void;
  /** The backend has finished moving items: data is stale, selection should clear. */
  onCleaned: () => void;
  onClose: () => void;
}

type PreviewPhase = { kind: "preview"; preview: CleanupPreview; startError: string | null; starting: boolean };

type Phase =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | PreviewPhase
  | { kind: "running"; progress: CleanupProgress | null }
  | { kind: "done"; done: CleanupDone };

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

export function CleanupDialog({ paths, root, reinstalls, onReinstall, onCleaned, onClose }: Props) {
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const onCleanedRef = useRef(onCleaned);
  onCleanedRef.current = onCleaned;
  const running = phase.kind === "running";

  // The paths are fixed for the life of the dialog; closing and reopening re-runs the checks.
  useEffect(() => {
    let alive = true;
    api
      .previewCleanup(paths)
      .then((preview) => {
        if (alive) setPhase({ kind: "preview", preview, startError: null, starting: false });
      })
      .catch((e: unknown) => {
        if (alive) setPhase({ kind: "error", message: message(e) });
      });
    return () => {
      alive = false;
    };
  }, [paths]);

  useEffect(() => {
    let cancelled = false;
    const unsubs: Array<() => void> = [];
    const track = (p: Promise<() => void>) =>
      p.then((u) => {
        if (cancelled) u();
        else unsubs.push(u);
      });
    void track(
      api.onCleanupProgress((progress) => setPhase((s) => (s.kind === "running" ? { kind: "running", progress } : s))),
    );
    void track(
      api.onCleanupDone((done) => {
        setPhase((s) => (s.kind === "running" ? { kind: "done", done } : s));
        onCleanedRef.current();
      }),
    );
    return () => {
      cancelled = true;
      unsubs.forEach((u) => u());
    };
  }, []);

  const start = (preview: CleanupPreview) => {
    setPhase({ kind: "preview", preview, startError: null, starting: true });
    api
      .startCleanup(preview.items.filter((i) => i.verdict !== "refuse").map((i) => i.path))
      .then(() =>
        // The done event can beat this resolution; then the phase is already "done" and stays so.
        setPhase((s) => (s.kind === "preview" ? { kind: "running", progress: null } : s)),
      )
      .catch((e: unknown) => setPhase({ kind: "preview", preview, startError: message(e), starting: false }));
  };

  const title =
    phase.kind === "done" ? "Cleanup finished" : phase.kind === "running" ? "Moving to the Recycle Bin" : "Review cleanup";

  return (
    <Modal title={title} labelId="cleanup-title" phase={phase.kind} onClose={running ? null : onClose}>
      {phase.kind === "loading" && (
        <div className="modal-body">
          <p className="muted">
            <span className="spinner" aria-hidden /> Checking what can be removed…
          </p>
        </div>
      )}

      {phase.kind === "error" && (
        <>
          <div className="modal-body">
            <p className="err-text" role="alert">
              {phase.message}
            </p>
          </div>
          <div className="modal-foot">
            <div className="modal-foot-row">
              <span />
              <button className="btn" data-initial-focus onClick={onClose}>
                Close
              </button>
            </div>
          </div>
        </>
      )}

      {phase.kind === "preview" && <PreviewView phase={phase} root={root} onStart={start} onClose={onClose} />}

      {phase.kind === "running" && (
        <div className="modal-body">
          <p>
            <span className="num">
              {phase.progress?.done ?? 0} of {phase.progress?.total ?? "…"}
            </span>{" "}
            items moved
          </p>
          <div
            className="meter"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={phase.progress?.total ?? 0}
            aria-valuenow={phase.progress?.done ?? 0}
          >
            <div
              className="meter-fill"
              style={{
                width: `${phase.progress && phase.progress.total ? (phase.progress.done / phase.progress.total) * 100 : 0}%`,
              }}
            />
          </div>
          <p className="muted small ellipsis" title={phase.progress?.currentPath}>
            {phase.progress ? truncateMiddle(phase.progress.currentPath, 90) : "Starting…"}
          </p>
        </div>
      )}

      {phase.kind === "done" && (
        <DoneView done={phase.done} reinstalls={reinstalls} onReinstall={onReinstall} onClose={onClose} root={root} />
      )}
    </Modal>
  );
}

function PreviewView({
  phase,
  root,
  onStart,
  onClose,
}: {
  phase: PreviewPhase;
  root: string;
  onStart: (p: CleanupPreview) => void;
  onClose: () => void;
}) {
  const { preview } = phase;
  const movable = preview.items.filter((i) => i.verdict !== "refuse").length;
  return (
    <>
      <div className="modal-body">
        <ul className="plist">
          {preview.items.map((i) => (
            <li key={i.path} className={`pitem pitem-${i.verdict}`}>
              <div className="pitem-main">
                <div className="pitem-line">
                  <span className="pitem-name" title={i.name}>
                    {i.name}
                  </span>
                  {i.regenerable && <span className="tag tag-regen">regenerable</span>}
                  {i.projectLabel && <span className="tag">{i.projectLabel}</span>}
                  {i.verdict === "refuse" && <span className="skip-label">Will be skipped</span>}
                </div>
                <div className="pitem-path small muted" title={i.path}>
                  {relativePath(i.path, root)}
                </div>
              </div>
              <div className="pitem-size num">{formatBytes(i.allocated)}</div>
              {i.reasons.length > 0 && (
                <ul className={`reasons ${i.verdict === "refuse" ? "reasons-refuse" : "reasons-warn"}`}>
                  {i.reasons.map((r) => (
                    <li key={r}>{r}</li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      </div>
      <div className="modal-foot">
        <p className="small muted">Everything goes to the Recycle Bin. You can put it back from History.</p>
        {phase.startError && (
          <p className="err-text small" role="alert">
            {phase.startError}
          </p>
        )}
        <div className="modal-foot-row">
          <span className="free-total">
            Frees <strong className="num">{formatBytes(preview.totalFreed)}</strong>
          </span>
          <span className="modal-foot-actions">
            <button className="btn" data-initial-focus onClick={onClose} disabled={phase.starting}>
              Cancel
            </button>
            <button className="btn btn-primary" disabled={movable === 0 || phase.starting} onClick={() => onStart(preview)}>
              Move {movable} {movable === 1 ? "item" : "items"} to Recycle Bin
            </button>
          </span>
        </div>
      </div>
    </>
  );
}

function DoneView({
  done,
  reinstalls,
  onReinstall,
  onClose,
  root,
}: {
  done: CleanupDone;
  reinstalls: Record<string, ReinstallState>;
  onReinstall: (projectPath: string) => void;
  onClose: () => void;
  root: string;
}) {
  const failed = done.items.filter((i) => !i.ok);
  const moved = done.items.length - failed.length;
  return (
    <>
      <div className="modal-body">
        <p>
          Moved <span className="num">{moved}</span> {moved === 1 ? "item" : "items"} to the Recycle Bin and freed{" "}
          <strong className="num">{formatBytes(done.freed)}</strong>. You can put {moved === 1 ? "it" : "them"} back from
          History.
        </p>

        {failed.length > 0 && (
          <div className="result-block">
            <h3 className="result-h">Not moved</h3>
            <ul className="fail-list">
              {failed.map((i) => (
                <li key={i.path}>
                  <span title={i.path}>{relativePath(i.path, root)}</span>
                  <span className="err-text small">{i.error ?? "Unknown error"}</span>
                </li>
              ))}
            </ul>
          </div>
        )}

        {done.reinstall.length > 0 && (
          <div className="result-block">
            <h3 className="result-h">Dependencies were removed</h3>
            {done.reinstall.map((o) => {
              const st = reinstalls[o.projectPath];
              return (
                <div key={o.projectPath} className="reinstall-row">
                  <div className="reinstall-head">
                    <span title={o.projectPath}>{baseName(o.projectPath)}</span>
                    <button className="btn btn-small" disabled={st?.running} onClick={() => onReinstall(o.projectPath)}>
                      {st?.result ? "Reinstall again" : `Reinstall dependencies (${o.command})`}
                    </button>
                  </div>
                  {st && <ReinstallLog state={st} />}
                </div>
              );
            })}
          </div>
        )}
      </div>
      <div className="modal-foot">
        <div className="modal-foot-row">
          <span />
          <button className="btn btn-primary" data-initial-focus onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </>
  );
}
