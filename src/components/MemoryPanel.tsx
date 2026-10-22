import { useCallback, useEffect, useState } from "react";
import { memoryStatus, readMemoryFile, type MemoryRepo, type MemoryStatus } from "../run/api";
import { errorText } from "./InlineNotice";
import { TextDialog } from "./RunPanel";

/** One line per repo: present / N stale / missing. */
export function memoryLine(repo: MemoryRepo): string {
  if (!repo.present) return "missing";
  const stale = repo.staleCount ?? (repo.topics ?? []).filter((t) => t.stale).length;
  return stale > 0 ? `present, ${stale} stale` : "present";
}

type ViewProps = {
  status: MemoryStatus | null;
  onOnboard: () => void;
  onRefresh: () => void;
  onOpen: (repo: MemoryRepo) => void;
};

export function MemoryPanelView({ status, onOnboard, onRefresh, onOpen }: ViewProps) {
  return (
    <section className="memory-panel" aria-label="Codebase memory">
      <h3>Codebase memory</h3>
      {!status && <p className="hint">Looking…</p>}
      {status && !status.available && <p className="hint">Not available: {status.problem ?? "unknown"}</p>}
      {status?.available && (
        <ul className="memory-repos">
          {status.workspace && <li>Workspace: {status.workspace.present ? "present" : "missing"}</li>}
          {(status.repos ?? []).map((repo) => (
            <li key={repo.id ?? repo.path ?? repo.name}>
              <strong>{repo.name ?? repo.id}</strong>: {memoryLine(repo)}
              {repo.present && repo.id && (
                <button type="button" className="link-button" onClick={() => onOpen(repo)}>
                  read
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="button-row">
        <button type="button" className="secondary-button" onClick={onOnboard}>
          Onboard
        </button>
        <button type="button" className="secondary-button" onClick={onRefresh}>
          Refresh
        </button>
      </div>
    </section>
  );
}

/** Memory for the start screen's folder; Onboard launches /dcv2:onboard. */
export function MemoryPanel({ folder, onOnboard }: { folder: string; onOnboard: () => void }) {
  const [status, setStatus] = useState<MemoryStatus | null>(null);
  const [file, setFile] = useState<{ title: string; text: string } | null>(null);

  const refresh = useCallback(() => {
    setStatus(null);
    memoryStatus(folder).then(setStatus, (err: unknown) => setStatus({ available: false, problem: errorText(err) }));
  }, [folder]);

  useEffect(() => refresh(), [refresh]);

  const open = (repo: MemoryRepo) => {
    const title = `${repo.name ?? repo.id} memory`;
    readMemoryFile(`${repo.id}/index.md`).then(
      (text) => setFile({ title, text }),
      (err: unknown) => setFile({ title, text: errorText(err) }),
    );
  };

  return (
    <>
      <MemoryPanelView status={status} onOnboard={onOnboard} onRefresh={refresh} onOpen={open} />
      {file && <TextDialog title={file.title} text={file.text} onClose={() => setFile(null)} />}
    </>
  );
}
