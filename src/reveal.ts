import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { inTauri } from "./api";

export const canReveal = inTauri;

export function reveal(path: string): void {
  if (!canReveal) return;
  // A failed reveal (path deleted since the scan) is not worth interrupting the user for.
  revealItemInDir(path).catch(() => undefined);
}
