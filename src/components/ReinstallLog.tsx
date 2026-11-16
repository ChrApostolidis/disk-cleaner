import { useEffect, useRef } from "react";
import type { ReinstallState } from "../useReinstalls";

export function ReinstallLog({ state }: { state: ReinstallState }) {
  const box = useRef<HTMLPreElement>(null);

  // Follow the output like a terminal.
  useEffect(() => {
    const el = box.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state.lines.length]);

  return (
    <div className="rlog">
      <pre className="rlog-body" ref={box} tabIndex={0} aria-label="Reinstall output">
        {state.lines.map((l, i) => (
          <div key={i} className={l.stream === "stderr" ? "rlog-err" : undefined}>
            {l.text || " "}
          </div>
        ))}
      </pre>
      <div className="rlog-status small" role="status">
        {state.running && (
          <>
            <span className="spinner" aria-hidden /> Reinstalling…
          </>
        )}
        {state.startError && <span className="err-text">Could not start: {state.startError}</span>}
        {state.result &&
          (state.result.success ? (
            <span className="ok-text">Dependencies reinstalled.</span>
          ) : (
            <span className="err-text">
              Reinstall failed{state.result.exitCode !== null ? ` (exit code ${state.result.exitCode})` : ""}.
            </span>
          ))}
      </div>
    </div>
  );
}
