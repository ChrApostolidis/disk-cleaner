import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { AppsTab } from "./components/AppsTab";
import { DevToolsTab } from "./components/DevToolsTab";
import { DuplicatesTab } from "./components/DuplicatesTab";
import { SelectionBar } from "./components/SelectionBar";
import { SelectionDrawer } from "./components/SelectionDrawer";
import { projectItems, totalBytes, withItems, withoutPaths, selectionState, type SelectedItem, type Selection } from "./selection";
import { CleanupDialog } from "./components/CleanupDialog";
import { useReinstalls } from "./useReinstalls";
import { RecycleBinDialog } from "./components/RecycleBinDialog";
import { useRecycleBin } from "./useRecycleBin";
import { useDebounced } from "./useDebounced";

type Tab = "projects" | "files" | "dups" | "map" | "history" | "apps" | "dev";

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
  // Dev tools measure folders outside the scan, so the tab is kept once opened instead of re-measuring per visit.
  const [devVisited, setDevVisited] = useState(false);
  // A running duplicate search reports through events, so the tab stays mounted once opened.
  const [dupsVisited, setDupsVisited] = useState(false);

  // Everything picked for cleanup, kept here so the selection survives tab switches.
  const [selection, setSelection] = useState<Selection>(new Map());
  const [drawerOpen, setDrawerOpen] = useState(false);
  const viewRef = useRef<HTMLButtonElement>(null);
  // Paths frozen when the review dialog opens, so list refreshes cannot change what is being reviewed.
  const [reviewPaths, setReviewPaths] = useState<string[] | null>(null);
  // Bumped after a cleanup so the map remounts and shows the new tree.
  const [dataVersion, setDataVersion] = useState(0);
  const { reinstalls, startReinstall } = useReinstalls();
  const { bin, setBin, refresh: refreshBin } = useRecycleBin();
  const [binOpen, setBinOpen] = useState(false);
  // Read inside the scan-done handler, which is registered once.
  const scanningRef = useRef(false);

  const setMany = useCallback((items: SelectedItem[], on: boolean) => {
    setSelection((prev) => (on ? withItems(prev, items) : withoutPaths(prev, items.map((i) => i.path))));
  }, []);
  const toggle = useCallback(
    (item: SelectedItem) =>
      setSelection((prev) => {
        const state = selectionState(prev, item.path);
        if (state === "selected") return withoutPaths(prev, [item.path]);
        // Already inside a selected folder: nothing to add.
        return state === "covered" ? prev : withItems(prev, [item]);
      }),
    [],
  );
  const removeOne = useCallback((path: string) => setSelection((prev) => withoutPaths(prev, [path])), []);
  const clearSelection = useCallback(() => setSelection(new Map()), []);
  const cleanProject = useCallback(
    (p: Project) => {
      const picked = withItems(selection, projectItems(p));
      setSelection(picked);
      setReviewPaths([...picked.values()].map((i) => i.path));
    },
    [selection],
  );
  const closeDrawer = useCallback(() => {
    setDrawerOpen(false);
    requestAnimationFrame(() => viewRef.current?.focus());
  }, []);
  const items = useMemo(() => [...selection.values()], [selection]);
  useEffect(() => {
    if (selection.size === 0) setDrawerOpen(false);
  }, [selection.size]);

  const afterCleanup = useCallback(() => {
    setSelection(new Map());
    setDrawerOpen(false);
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
        // A scan nobody started here is the one restored from disk at startup: show it as the current scan.
        if (!scanningRef.current) setRoot(s.root);
        scanningRef.current = false;
        setScanning(false);
        setProgress(null);
        setScanError(null);
        setSummary(s);
      }),
    );
    void track(
      api.onError((e) => {
        scanningRef.current = false;
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
    if (!summary || tab === "map" || tab === "history" || tab === "apps" || tab === "dev" || tab === "dups") return;
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
    setDupsVisited(false);
    setSelection(new Map());
    setDrawerOpen(false);
    setProgress(null);
    scanningRef.current = true;
    setScanning(true);
    api.startScan(root.trim()).catch((e: unknown) => {
      scanningRef.current = false;
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
        hasScan={summary !== null}
        bin={bin}
        onBin={() => setBinOpen(true)}
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
                aria-selected={tab === "dups"}
                className="tab"
                onClick={() => {
                  setTab("dups");
                  setDupsVisited(true);
                }}
              >
                Duplicates
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
              <button role="tab" aria-selected={tab === "apps"} className="tab" onClick={() => setTab("apps")}>
                Apps
              </button>
              <button
                role="tab"
                aria-selected={tab === "dev"}
                className="tab"
                onClick={() => {
                  setTab("dev");
                  setDevVisited(true);
                }}
              >
                Dev tools
              </button>
            </div>
          </div>
          {(tab === "projects" || tab === "files" || tab === "dups") && (
            <SearchBar value={query} onChange={setQuery} error={searchError} />
          )}
          <main className="content">
            {mapVisited && (
              <div className="map-host" hidden={tab !== "map"}>
                <Treemap
                  key={`${summary.root}|${summary.elapsedMs}|${summary.files}|${dataVersion}`}
                  root={summary.root}
                  selection={selection}
                  onToggle={toggle}
                />
              </div>
            )}
            {devVisited && (
              <div className="map-host" hidden={tab !== "dev"}>
                {/* Remounted per scan and cleanup, since sizes come from the scan where it covers them. */}
                <DevToolsTab key={`${summary.scannedAt}|${dataVersion}`} />
              </div>
            )}
            {dupsVisited && (
              <div className="map-host" hidden={tab !== "dups"}>
                {/* Keyed by scan: a search belongs to one scan, while a cleanup keeps it and only refetches. */}
                <DuplicatesTab
                  key={summary.scannedAt}
                  query={debounced}
                  root={summary.root}
                  dataVersion={dataVersion}
                  selection={selection}
                  onSetMany={setMany}
                  onSearchError={setSearchError}
                />
              </div>
            )}
            {tab === "map" || tab === "dev" || tab === "dups" ? null : tab === "apps" ? (
              // Remounted per scan and cleanup, since app sizes come from the scan.
              <AppsTab key={`${summary.scannedAt}|${dataVersion}`} />
            ) : tab === "history" ? (
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
                onSetMany={setMany}
                onClear={clearSelection}
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
          bytes={totalBytes(selection)}
          drawerOpen={drawerOpen}
          viewRef={viewRef}
          onClear={clearSelection}
          onView={() => (drawerOpen ? closeDrawer() : setDrawerOpen(true))}
          onReview={() => setReviewPaths(items.map((i) => i.path))}
        />
      )}

      {drawerOpen && selection.size > 0 && summary && (
        <SelectionDrawer
          items={items}
          root={summary.root}
          onRemove={removeOne}
          onClear={clearSelection}
          onReview={() => {
            setDrawerOpen(false);
            setReviewPaths(items.map((i) => i.path));
          }}
          onClose={closeDrawer}
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

      {binOpen && (
        <RecycleBinDialog bin={bin} onBinChange={setBin} onRefreshBin={refreshBin} onClose={() => setBinOpen(false)} />
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
