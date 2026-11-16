import type { Project } from "../api/types";
import { projectItems, type SelectedItem, type Selection } from "../selection";
import { ProjectCard } from "./ProjectCard";

interface Props {
  projects: Project[];
  root: string;
  selection: Selection;
  onToggle: (item: SelectedItem) => void;
  onSetMany: (items: SelectedItem[], on: boolean) => void;
  onClear: () => void;
  onClean: (p: Project) => void;
}

export function ProjectsTab({ projects, root, selection, onToggle, onSetMany, onClear, onClean }: Props) {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const shown = projects.flatMap(projectItems);
  return (
    <>
      <div className="grid-tools">
        <button className="btn btn-small" disabled={shown.length === 0} onClick={() => onSetMany(shown, true)}>
          Select all shown
        </button>
        <button className="btn btn-small" disabled={selection.size === 0} onClick={onClear}>
          Clear selection
        </button>
      </div>
      <div className="grid">
        {projects.map((p) => (
          <ProjectCard
            key={p.id}
            project={p}
            root={root}
            selection={selection}
            onToggle={onToggle}
            onSetMany={onSetMany}
            onClean={onClean}
            // A parent filtered out by the query isn't in this list; then the label is simply dropped.
            parentName={p.parentId === null ? null : (byId.get(p.parentId)?.name ?? null)}
          />
        ))}
      </div>
    </>
  );
}
