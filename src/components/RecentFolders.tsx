import { useEffect, useState } from "react";
import { recentFolders } from "../bridge";

/** The recent folders (up to 10, newest first), read once when the form shows. */
export function useRecentFolders(): string[] {
  const [folders, setFolders] = useState<string[]>([]);
  useEffect(() => {
    let current = true;
    recentFolders().then(
      (list) => current && setFolders(list),
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, []);
  return folders;
}

/** A `<datalist>` a folder input points at with `list={id}`. */
export function RecentFolderOptions({ id, folders }: { id: string; folders: string[] }) {
  return (
    <datalist id={id}>
      {folders.map((folder) => (
        <option key={folder} value={folder} />
      ))}
    </datalist>
  );
}
