import { useEffect, useState } from "react";
import { api } from "../api";
import type { DevInstall, DevItem, DevSizeSource, DevTools } from "../api/types";
import { formatBytes } from "../format";
import { canReveal, reveal } from "../reveal";

type Section = "languages" | "tools";

const SECTIONS: { key: Section; label: string }[] = [
  { key: "languages", label: "Languages" },
  { key: "tools", label: "Tools" },
];

const SOURCE_TITLE: Record<DevSizeSource, string> = {
  scan: "From your scan",
  measured: "Measured just now (outside your scan)",
  windows: "The installer's own estimate",
};

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

function Size({ install }: { install: DevInstall }) {
  if (install.size === null) return <span className="muted">—</span>;
  return (
    <span title={install.sizeSource ? SOURCE_TITLE[install.sizeSource] : undefined}>
      {install.sizeSource === "windows" && <span className="tag">est.</span>} {formatBytes(install.size)}
    </span>
  );
}

function ItemCard({ item }: { item: DevItem }) {
  const versions = item.installs.filter((i) => i.version !== null).length;
  const count =
    item.kind === "language"
      ? `${item.installs.length} ${item.installs.length === 1 ? "version" : "versions"}`
      : `${item.installs.length} ${item.installs.length === 1 ? "part" : "parts"}`;
  return (
    <section className="batch" aria-label={item.name}>
      <header className="batch-head">
        <div>
          <span className="batch-date">{item.name}</span>
          <span className="muted small"> · {count}</span>
        </div>
        <span className="num dev-total">{item.size === null ? "—" : formatBytes(item.size)}</span>
      </header>
      <ul className="hlist">
        {item.installs.map((i, n) => (
          <li className="hitem dev-row" key={`${i.label}|${i.path ?? n}`}>
            <div className="hitem-main">
              <div className="dev-label">
                <span className="ellipsis">{item.kind === "language" && i.version !== null ? i.version : i.label}</span>
                {i.active && (
                  <span className="tag tag-regen" title="Runs when you type the command">
                    Active
                  </span>
                )}
                {item.kind === "tool" && i.version !== null && <span className="muted small num">{i.version}</span>}
              </div>
              <div className="small muted ellipsis" title={i.path ?? undefined}>
                {item.kind === "language" && i.version !== null && i.label !== i.version ? `${i.label} · ` : ""}
                {i.path ?? "Location unknown"}
              </div>
            </div>
            <div className="right num">
              <Size install={i} />
            </div>
            <div className="right">
              {canReveal && i.path && (
                <button className="btn btn-small btn-ghost" onClick={() => reveal(i.path ?? "")}>
                  Show in Explorer
                </button>
              )}
            </div>
          </li>
        ))}
      </ul>
      {item.kind === "language" && versions > 1 && (
        <p className="small muted dev-hint">
          Only the active version runs by default. Others stay installed until you remove them with their version manager.
        </p>
      )}
    </section>
  );
}

export function DevToolsTab() {
  const [data, setData] = useState<DevTools | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [section, setSection] = useState<Section>("languages");

  useEffect(() => {
    let live = true;
    api
      .listDevTools()
      .then((r) => live && setData(r))
      .catch((e: unknown) => live && setError(message(e)));
    return () => {
      live = false;
    };
  }, []);

  if (error) {
    return (
      <div className="banner banner-error" role="alert">
        Could not list languages and tools: {error}
      </div>
    );
  }
  if (!data) {
    return (
      <div className="empty">
        <h2>Finding languages and tools…</h2>
        <p className="muted">Folders outside your scan are measured now, which can take a few seconds.</p>
      </div>
    );
  }

  const items = data[section];
  const total = items.reduce((sum, i) => sum + (i.size ?? 0), 0);

  return (
    <div className="history dev">
      <div className="apps-bar">
        <div className="chips" role="tablist" aria-label="Dev tools section">
          {SECTIONS.map((s) => (
            <button
              key={s.key}
              role="tab"
              aria-selected={section === s.key}
              className={`chip${section === s.key ? " chip-on" : ""}`}
              onClick={() => setSection(s.key)}
            >
              {s.label} <span className="num">{data[s.key].length}</span>
            </button>
          ))}
        </div>
        <span className="small muted">
          Total <span className="num">{formatBytes(total)}</span>
        </span>
      </div>
      {items.length === 0 ? (
        <div className="empty">
          <h2>{section === "languages" ? "No languages found" : "No dev tools found"}</h2>
          <p className="muted">
            {section === "languages"
              ? "DevDisk looks for Node.js, Python, Java, .NET, Rust, Go and PHP in their usual folders and on PATH."
              : "DevDisk looks for Git, Docker, WSL, VS Code, Visual Studio, JetBrains IDEs and Android."}
          </p>
        </div>
      ) : (
        items.map((item) => <ItemCard key={item.id} item={item} />)
      )}
    </div>
  );
}
