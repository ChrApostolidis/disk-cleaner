import { isTauri } from "@tauri-apps/api/core";
import type { DevDiskApi } from "./types";
import { tauriApi } from "./tauri";
import { mockApi } from "./mock";

export const inTauri = isTauri();
export const api: DevDiskApi = inTauri ? tauriApi : mockApi;
