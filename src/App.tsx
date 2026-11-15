import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";
import { api, inTauri } from "./api";
import { MOCK_DEFAULT_ROOT } from "./api/mock";
import type { FileEntry, FileSort, Project, ScanError, ScanProgress, ScanSummary } from "./api/types";
import { TopBar } from "./components/TopBar";
import { SummaryStrip } from "./components/SummaryStrip";
import { SearchBar } from "./components/SearchBar";
import { ProjectsTab } from "./components/ProjectsTab";
import { FilesTab } from "./components/FilesTab";
import { Treemap } from "./components/Treemap";
import { useDebounced } from "./useDebounced";

type Tab = "projects" | "files" | "map";

const FILE_LIMIT = 500;

function message(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Something went wrong.";
}

function App() {
  const [root, setRoot] = useState(inTauri ? "" : MOCK_DEFAULT_ROOT);
  const [scanning, setScanning] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [summary, setSummary] = useState<ScanSummary | null>(null);
  const [scanError, setScanError] = useState<ScanError | null>(null);

  const [tab, setTab] = useState<Tab>("projects");
  const [query, setQuery] = useState("");
  const debounced = useDebounced(query, 200);
  const [sort, setSort] = useState<FileSort>("allocated");
  const [descending, setDescending] = useState(true);

  const [projects, setProjects] = useState<Project[]>([]);
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const requestId = useRef(0);
  // The map keeps its drill-down while another tab is shown, so it mounts on first visit and then stays.
  const [mapVisited, setMapVisited] = useState(false);

  // Scan lifecycle events. Unsubscribing in cleanup keeps StrictMode's double mount from doubling handlers.
  useEffect(() => {
    let cancelled = false;
    const unsubs: Array<() => void> = [];
    const track = (p: Promise<() => void>) =>
      p.then((u) => {
        if (cancelled) u();
        else unsubs.push(u);
      });
    void track(api.onProgress(setProgress));
    void track(
      api.onDone((s) => {
        setScanning(false);
        setProgress(null);
        setScanError(null);
        setSummary(s);
      }),
    );
    void track(
      api.onError((e) => {
        setScanning(false);
        setProgress(null);
        setScanError(e);
      }),
    );
    api
      .getSummary()
      .then((s) => {
        if (!cancelled && s) {
          setSummary(s);
          setRoot(s.root);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unsubs.forEach((u) => u());
    };
  }, []);

  // Refetch the visible tab whenever the query, sort or scan changes; stale replies are dropped.
  useEffect(() => {
    if (!summary || tab === "map") return;
    const id = ++requestId.current;
    const work =
      tab === "projects"
        ? api.listProjects(debounced).then((r) => {
            if (id === requestId.current) setProjects(r);
          })
        : api.listFiles(debounced, sort, descending, FILE_LIMIT).then((r) => {
            if (id === requestId.current) setFiles(r);
          });
    work
      .then(() => {
        if (id === requestId.current) {
          setSearchError(null);
          setLoaded(true);
        }
      })
      .catch((e: unknown) => {
        // Keep the last good results on screen; only the message changes.
        if (id === requestId.current) setSearchError(message(e));
      });
  }, [summary, tab, debounced, sort, descending]);

  const choose = useCallback(async () => {
    const picked = await open({ directory: true });
    if (typeof picked === "string") setRoot(picked);
  }, []);

  const scan = useCallback(() => {
    setScanError(null);
    setSummary(null);
    setProjects([]);
    setFiles([]);
    setSearchError(null);
    setLoaded(false);
    setMapVisited(false);
    setProgress(null);
    setScanning(true);
    api.startScan(root.trim()).catch((e: unknown) => {
      setScanning(false);
      setScanError({ message: message(e), cancelled: false });
    });
  }, [root]);

  const onSort = (key: FileSort) => {
    if (key === sort) setDescending((d) => !d);
    else {
      setSort(key);
      setDescending(key !== "name");
    }
  };

  const results = tab === "files" ? files : projects;
  const showNoResults = tab !== "map" && summary !== null && loaded && results.length === 0 && searchError === null;

  return (
    <div className="app">
      <TopBar
        root={root}
        onRootChange={setRoot}
        hasDialog={inTauri}
        onChoose={() => void choose()}
        scanning={scanning}
        onScan={scan}
        onCancel={() => void api.cancelScan()}
        progress={progress}
      />

      {scanError && (
        <div className={`banner ${scanError.cancelled ? "banner-info" : "banner-error"}`} role="alert">
          {scanError.cancelled ? "Scan cancelled." : `Scan failed: ${scanError.message}`}
        </div>
      )}

      {summary && (
        <>
          <SummaryStrip s={summary} />
          <div className="toolbar">
            <div className="tabs" role="tablist">
              <button role="tab" aria-selected={tab === "projects"} className="tab" onClick={() => setTab("projects")}>
                Projects
              </button>
              <button role="tab" aria-selected={tab === "files"} className="tab" onClick={() => setTab("files")}>
                Biggest files
              </button>
              <button
                role="tab"
                aria-selected={tab === "map"}
                className="tab"
                onClick={() => {
                  setTab("map");
                  setMapVisited(true);
                }}
              >
                Map
              </button>
            </div>
          </div>
          {tab !== "map" && <SearchBar value={query} onChange={setQuery} error={searchError} />}
          <main className="content">
            {mapVisited && (
              <div className="map-host" hidden={tab !== "map"}>
                <Treemap key={`${summary.root}|${summary.elapsedMs}|${summary.files}`} root={summary.root} />
              </div>
            )}
            {tab === "map" ? null : showNoResults ? (
              <div className="empty">
                <h2>No results</h2>
                <p className="muted">Nothing matches this search. Try fewer words or remove a filter.</p>
              </div>
            ) : tab === "projects" ? (
              <ProjectsTab projects={projects} root={summary.root} />
            ) : (
              <FilesTab files={files} root={summary.root} sort={sort} descending={descending} onSort={onSort} />
            )}
          </main>
        </>
      )}

      {!summary && !scanning && !scanError && (
        <div className="empty">
          <h2>Pick a folder to scan</h2>
          <p className="muted">
            DevDisk finds your code projects and the biggest files, and shows what you can safely reclaim.
          </p>
        </div>
      )}
      {scanning && !summary && (
        <div className="empty">
          <h2>Scanning…</h2>
          <p className="muted">Results appear when the scan finishes.</p>
        </div>
      )}
    </div>
  );
}

export default App;
