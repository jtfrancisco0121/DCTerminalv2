/**
 * Path helpers that accept both separators and keep a Windows drive root
 * (`C:\`). Every place that splits a path for display or resolution uses these,
 * so a Windows path and a POSIX path behave the same way.
 */

const SEP = /[\\/]/;

/** `C:\` or `C:/` at the start, with the separator that was written. */
function driveRoot(path: string): string | null {
  const match = /^[A-Za-z]:[\\/]/.exec(path);
  return match ? match[0] : null;
}

/** The last component of `path`. A drive root is its own name (`C:\`); `/` has none (""). */
export function baseName(path: string): string {
  const root = driveRoot(path);
  if (root && path.length === root.length) return root;
  const trimmed = path.length > 1 ? path.replace(/[\\/]+$/, "") : path;
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut < 0 ? trimmed : trimmed.slice(cut + 1);
}

/** The folder of `path` (no trailing separator), keeping a drive root: `C:\a\b.rs` is `C:\a`, `C:\a` is `C:\`, `/a` is `/`. */
export function dirName(path: string): string {
  const root = driveRoot(path);
  if (root && path.length === root.length) return root;
  const trimmed = path.length > 1 ? path.replace(/[\\/]+$/, "") : path;
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (root && cut < root.length) return root;
  return cut <= 0 ? "/" : trimmed.slice(0, cut);
}

/** The pieces of an absolute path: its root (`/` or `C:\`), the separator it uses, and the names after the root. */
export function splitAbsolute(path: string): { root: string; sep: string; parts: string[] } {
  const root = driveRoot(path);
  if (root) {
    return { root, sep: root.endsWith("\\") ? "\\" : "/", parts: path.slice(root.length).split(SEP).filter(Boolean) };
  }
  const sep = !path.includes("/") && path.includes("\\") ? "\\" : "/";
  return { root: "/", sep, parts: path.split(SEP).filter(Boolean) };
}
