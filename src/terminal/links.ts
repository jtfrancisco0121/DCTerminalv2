/** Cmd-click on macOS, Ctrl-click elsewhere, opens a URL in the terminal. A plain click only selects. */
export function isLinkModifier(
  event: { metaKey: boolean; ctrlKey: boolean },
  platform: "mac" | "windows" | "linux",
): boolean {
  return platform === "mac" ? event.metaKey : event.ctrlKey;
}
