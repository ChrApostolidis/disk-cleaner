// Shared with the treemap later: one stable color per framework label.
export const FRAMEWORK_COLORS: Record<string, string> = {
  "Next.js": "#4b5563",
  React: "#0ea5c6",
  "React (Vite)": "#7c5cf0",
  Vue: "#2fa36b",
  Nuxt: "#00b36b",
  Angular: "#dd2c3c",
  SvelteKit: "#f0562a",
  Astro: "#8b5cf6",
  Remix: "#3b82f6",
  "React Native": "#2b8ce0",
  NestJS: "#e0234e",
  "Node.js": "#5fa04e",
  "Node.js (Express)": "#7a9a3a",
  Tauri: "#e8a317",
  Electron: "#4a8fa8",
  Laravel: "#f0483e",
  Symfony: "#2a2a2a",
  Rust: "#c2562b",
  Python: "#3b78b4",
  Django: "#1f7a5a",
  FastAPI: "#059e8a",
  Flask: "#6b7280",
  "Spring Boot": "#5aa832",
  Maven: "#c0392b",
  Gradle: "#2e7d9a",
};

const FALLBACK = "#7b8494";

export function frameworkColor(label: string): string {
  return FRAMEWORK_COLORS[label] ?? FALLBACK;
}
