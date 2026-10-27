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

/** A topic name that is safe in `<repo id>/topics/<name>.md`. */
export function topicNameOk(name: string): boolean {
  return /^[A-Za-z0-9._-]+$/.test(name) && !name.startsWith(".");
}

type ViewProps = {
  status: MemoryStatus | null;
  onOnboard: () => void;
  onRefresh: () => void;
  onOpen: (repo: MemoryRepo) => void;
  /** Opens `<repo id>/topics/<name>.md` read-only. */
  onOpenTopic?: (repo: MemoryRepo, topic: string) => void;
  /** Starts the onboard role with the request `stale <repo name>`. */
  onRefreshStale?: (repo: MemoryRepo) => void;
  /** A launch is in progress: Refresh stale waits. */
  busy?: boolean;
};

function staleOf(repo: MemoryRepo): number {
  return repo.staleCount ?? (repo.topics ?? []).filter((t) => t.stale).length;
}

function RepoTopics({ repo, onOpenTopic }: { repo: MemoryRepo; onOpenTopic?: ViewProps["onOpenTopic"] }) {
  const topics = repo.topics ?? [];
  return topics.length === 0 ? (
    <p className="hint">No topics yet.</p>
  ) : (
    <ul className="memory-topics">
      {topics.map((topic) => (
        <li key={topic.name}>
          {topicNameOk(topic.name) && repo.id && onOpenTopic ? (
            <button type="button" className="link-button" onClick={() => onOpenTopic(repo, topic.name)}>
              {topic.name}
            </button>
          ) : (
            <span>{topic.name}</span>
          )}
          {topic.stale && (
            <span className="stale-badge" title={`checked at ${topic.checkedAt ?? "unknown"}`}>
              stale
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

function RepoRow({
  repo,
  onOpen,
  onOpenTopic,
  onRefreshStale,
  busy,
}: { repo: MemoryRepo } & Pick<ViewProps, "onOpen" | "onOpenTopic" | "onRefreshStale" | "busy">) {
  const stale = staleOf(repo);
  // Collapsed by default; a repo with stale topics starts expanded.
  const [toggled, setToggled] = useState<boolean | null>(null);
  const expanded = toggled ?? stale > 0;
  const name = repo.name ?? repo.id ?? "repo";
  return (
    <li>
      {repo.present && (
        <button
          type="button"
          className="link-button memory-disclosure"
          aria-expanded={expanded}
          aria-label={`${expanded ? "Hide" : "Show"} topics of ${name}`}
          onClick={() => setToggled(!expanded)}
        >
          {expanded ? "▾" : "▸"}
        </button>
      )}
      <strong>{name}</strong>: {memoryLine(repo)}
      {repo.present && repo.id && (
        <button type="button" className="link-button" onClick={() => onOpen(repo)}>
          read
        </button>
      )}
      {repo.present && stale > 0 && onRefreshStale && (
        <button type="button" className="secondary-button" disabled={busy} onClick={() => onRefreshStale(repo)}>
          Refresh stale ({stale})
        </button>
      )}
      {repo.present && expanded && <RepoTopics repo={repo} onOpenTopic={onOpenTopic} />}
    </li>
  );
}

export function MemoryPanelView({ status, onOnboard, onRefresh, onOpen, onOpenTopic, onRefreshStale, busy }: ViewProps) {
  return (
    <section className="memory-panel" aria-label="Codebase memory">
      <h3>Codebase memory</h3>
      {!status && <p className="hint">Looking…</p>}
      {status && !status.available && <p className="hint">Not available: {status.problem ?? "unknown"}</p>}
      {status?.available && (
        <ul className="memory-repos">
          {status.workspace && <li>Workspace: {status.workspace.present ? "present" : "missing"}</li>}
          {(status.repos ?? []).map((repo) => (
            <RepoRow
              key={repo.id ?? repo.path ?? repo.name}
              repo={repo}
              onOpen={onOpen}
              onOpenTopic={onOpenTopic}
              onRefreshStale={onRefreshStale}
              busy={busy}
            />
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
export function MemoryPanel({
  folder,
  onOnboard,
  onRefreshStale,
  busy,
}: {
  folder: string;
  onOnboard: () => void;
  /** Launch onboard with this request (`stale <repo name>`). */
  onRefreshStale?: (request: string) => void;
  busy?: boolean;
}) {
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

  const openTopic = (repo: MemoryRepo, topic: string) => {
    const title = `${repo.name ?? repo.id} · ${topic}`;
    readMemoryFile(`${repo.id}/topics/${topic}.md`).then(
      (text) => setFile({ title, text }),
      (err: unknown) => setFile({ title, text: errorText(err) }),
    );
  };

  return (
    <>
      <MemoryPanelView
        status={status}
        onOnboard={onOnboard}
        onRefresh={refresh}
        onOpen={open}
        onOpenTopic={openTopic}
        onRefreshStale={onRefreshStale && ((repo) => onRefreshStale(`stale ${repo.name ?? repo.id}`))}
        busy={busy}
      />
      {file && <TextDialog title={file.title} text={file.text} onClose={() => setFile(null)} />}
    </>
  );
}
