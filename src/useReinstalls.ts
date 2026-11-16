import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { ReinstallDone } from "./api/types";

export interface ReinstallLine {
  stream: "stdout" | "stderr";
  text: string;
}

export interface ReinstallState {
  lines: ReinstallLine[];
  running: boolean;
  result: ReinstallDone | null;
  /** The start command itself was rejected. */
  startError: string | null;
}

const MAX_LINES = 2000;

/** Lives at App level so a running reinstall survives closing the cleanup dialog. */
export function useReinstalls() {
  const [state, setState] = useState<Record<string, ReinstallState>>({});

  useEffect(() => {
    let cancelled = false;
    const unsubs: Array<() => void> = [];
    const track = (p: Promise<() => void>) =>
      p.then((u) => {
        if (cancelled) u();
        else unsubs.push(u);
      });
    void track(
      api.onReinstallOutput((o) =>
        setState((s) => {
          const cur = s[o.projectPath];
          if (!cur) return s;
          const lines = [...cur.lines, { stream: o.stream, text: o.line }];
          return { ...s, [o.projectPath]: { ...cur, lines: lines.slice(-MAX_LINES) } };
        }),
      ),
    );
    void track(
      api.onReinstallDone((d) => {
        active.current.delete(d.projectPath);
        setState((s) => {
          const cur = s[d.projectPath];
          return cur ? { ...s, [d.projectPath]: { ...cur, running: false, result: d } } : s;
        });
      }),
    );
    return () => {
      cancelled = true;
      unsubs.forEach((u) => u());
    };
  }, []);

  // A ref, not state: the guard must hold even for two clicks inside one render.
  const active = useRef(new Set<string>());

  const start = useCallback((projectPath: string) => {
    if (active.current.has(projectPath)) return;
    active.current.add(projectPath);
    setState((s) => ({ ...s, [projectPath]: { lines: [], running: true, result: null, startError: null } }));
    api.startReinstall(projectPath).catch((e: unknown) => {
      active.current.delete(projectPath);
      setState((t) => ({
        ...t,
        [projectPath]: {
          ...t[projectPath],
          running: false,
          startError: typeof e === "string" ? e : e instanceof Error ? e.message : "Could not start the reinstall.",
        },
      }));
    });
  }, []);

  return { reinstalls: state, startReinstall: start };
}
