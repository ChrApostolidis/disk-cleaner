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
import { HistoryTab } from "./components/HistoryTab";
import { SelectionBar } from "./components/SelectionBar";
import { CleanupDialog } from "./components/CleanupDialog";
import { useReinstalls } from "./useReinstalls";
import { useDebounced } from "./useDebounced";

type Tab = "projects" | "files" | "map" | "history";

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

  // Selected paths with their size on disk, kept here so the selection survives tab switches.
  const [selection, setSelection] = useState<Map<string, number>>(new Map());
  // Paths frozen when the review dialog opens, so list refreshes cannot change what is being reviewed.
  const [reviewPaths, setReviewPaths] = useState<string[] | null>(null);
  // Bumped after a cleanup so the map remounts and shows the new tree.
  const [dataVersion, setDataVersion] = useState(0);
  const { reinstalls, startReinstall } = useReinstalls();

  const setMany = useCallback((items: { path: string; bytes: number }[], on: boolean) => {
    setSelection((prev) => {
      const next = new Map(prev);
      for (const it of items) {
        if (on) next.set(it.path, it.bytes);
        else next.delete(it.path);
      }
      return next;
    });
  }, []);
  const toggle = useCallback(
    (path: string, bytes: number) =>
      setSelection((prev) => {
        const next = new Map(prev);
        if (next.has(path)) next.delete(path);
        else next.set(path, bytes);
        return next;
      }),
    [],
  );
  const cleanProject = useCallback(
    (p: Project) => {
      const picked = new Map(selection);
      p.regenerable.forEach((f) => picked.set(f.path, f.allocated));
      setSelection(picked);
      setReviewPaths([...picked.keys()]);
    },
    [selection],
  );

  const afterCleanup = useCallback(() => {
    setSelection(new Map());
    setDataVersion((v) => v + 1);
    api
      .getSummary()
      .then((s) => {
        if (s) setSummary(s);
      })
      .catch(() => undefined);
  }, []);

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
    if (!summary || tab === "map" || tab === "history") return;
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
    setSelection(new Map());
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
  const showNoResults = (tab === "projects" || tab === "files") && summary !== null && loaded && results.length === 0 && searchError === null;

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
              <button role="tab" aria-selected={tab === "history"} className="tab" onClick={() => setTab("history")}>
                History
              </button>
            </div>
          </div>
          {(tab === "projects" || tab === "files") && <SearchBar value={query} onChange={setQuery} error={searchError} />}
          <main className="content">
            {mapVisited && (
              <div className="map-host" hidden={tab !== "map"}>
                <Treemap key={`${summary.root}|${summary.elapsedMs}|${summary.files}|${dataVersion}`} root={summary.root} />
              </div>
            )}
            {tab === "map" ? null : tab === "history" ? (
              // Keyed by dataVersion: a cleanup started from the selection bar while this tab is
              // open must show up here without switching tabs.
              <HistoryTab key={dataVersion} />
            ) : showNoResults ? (
              <div className="empty">
                <h2>No results</h2>
                <p className="muted">Nothing matches this search. Try fewer words or remove a filter.</p>
              </div>
            ) : tab === "projects" ? (
              <ProjectsTab
                projects={projects}
                root={summary.root}
                selection={selection}
                onToggle={toggle}
                onClean={cleanProject}
              />
            ) : (
              <FilesTab
                files={files}
                root={summary.root}
                sort={sort}
                descending={descending}
                onSort={onSort}
                selection={selection}
                onSetMany={setMany}
              />
            )}
          </main>
        </>
      )}

      {selection.size > 0 && summary && (
        <SelectionBar
          count={selection.size}
          bytes={[...selection.values()].reduce((a, b) => a + b, 0)}
          onClear={() => setSelection(new Map())}
          onReview={() => setReviewPaths([...selection.keys()])}
        />
      )}

      {reviewPaths && summary && (
        <CleanupDialog
          paths={reviewPaths}
          root={summary.root}
          reinstalls={reinstalls}
          onReinstall={startReinstall}
          onCleaned={afterCleanup}
          onClose={() => setReviewPaths(null)}
        />
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
