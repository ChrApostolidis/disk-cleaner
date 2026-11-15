import type { Project } from "../api/types";
import { ProjectCard } from "./ProjectCard";

export function ProjectsTab({ projects, root }: { projects: Project[]; root: string }) {
  const byId = new Map(projects.map((p) => [p.id, p]));
  return (
    <div className="grid">
      {projects.map((p) => (
        <ProjectCard
          key={p.id}
          project={p}
          root={root}
          // A parent filtered out by the query isn't in this list; then the label is simply dropped.
          parentName={p.parentId === null ? null : (byId.get(p.parentId)?.name ?? null)}
        />
      ))}
    </div>
  );
}
