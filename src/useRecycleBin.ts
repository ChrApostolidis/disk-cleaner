import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import type { RecycleBinSummary } from "./api/types";

/** Total for the top-bar indicator. Anything that can change the bin from outside triggers a refresh. */
export function useRecycleBin() {
  const [bin, setBin] = useState<RecycleBinSummary | null>(null);

  const refresh = useCallback(() => {
    api
      .getRecycleBin()
      .then(setBin)
      // Keep the last known total; a failed read should not blank the indicator.
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    let cancelled = false;
    let unsub: (() => void) | null = null;
    void api.onCleanupDone(refresh).then((u) => {
      if (cancelled) u();
      else unsub = u;
    });
    const onVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      cancelled = true;
      unsub?.();
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh]);

  return { bin, setBin, refresh };
}
