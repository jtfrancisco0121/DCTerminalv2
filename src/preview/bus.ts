import { dirName, splitAbsolute } from "../paths";

/** Opening a file in the preview pane: any component asks through this window event. */
export const PREVIEW_EVENT = "dcv2:preview";

export type PreviewTarget = { path: string; title: string };

export function openPreview(target: PreviewTarget): void {
  window.dispatchEvent(new CustomEvent<PreviewTarget>(PREVIEW_EVENT, { detail: target }));
}

/** Folder of an absolute path (no trailing separator; a Windows drive root is kept). */
export function dirname(path: string): string {
  return dirName(path);
}

/**
 * A relative link of a previewed file resolved against its folder, `..` and `.` folded.
 * Null for anything that is not a plain relative path (a scheme, an absolute path, a query-only link).
 */
export function resolveRelative(from: string, href: string): string | null {
  const bare = href.split("#")[0]!.split("?")[0]!;
  if (!bare || /^[a-z][a-z0-9+.-]*:/i.test(bare) || bare.startsWith("/") || bare.startsWith("\\") || bare.includes("\0")) return null;
  const { root, sep, parts } = splitAbsolute(dirname(from));
  for (const piece of decodeURIComponentSafe(bare).split("/")) {
    if (piece === "" || piece === ".") continue;
    if (piece === "..") parts.pop();
    else parts.push(piece);
  }
  return root + parts.join(sep);
}

function decodeURIComponentSafe(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    return text;
  }
}

/** The id a heading's text gets, for `#anchor` links. */
export function slug(text: string): string {
  return text
    .toLowerCase()
    .trim()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s+/g, "-");
}
