import type { Project } from "../api/types";
import { ProjectCard } from "./ProjectCard";

interface Props {
  projects: Project[];
  root: string;
  selection: Map<string, number>;
  onToggle: (path: string, bytes: number) => void;
  onClean: (p: Project) => void;
}

export function ProjectsTab({ projects, root, selection, onToggle, onClean }: Props) {
  const byId = new Map(projects.map((p) => [p.id, p]));
  return (
    <div className="grid">
      {projects.map((p) => (
        <ProjectCard
          key={p.id}
          project={p}
          root={root}
          selection={selection}
          onToggle={onToggle}
          onClean={onClean}
          // A parent filtered out by the query isn't in this list; then the label is simply dropped.
          parentName={p.parentId === null ? null : (byId.get(p.parentId)?.name ?? null)}
        />
      ))}
    </div>
  );
}
