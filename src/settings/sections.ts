/**
 * The Settings sections, in page order: the `aria-label` of each
 * `section.settings-section`. The command palette, the page's index and the
 * page itself all use this one list (SettingsPage.test.tsx checks that every
 * name is rendered and that nothing renders that is not listed).
 */
export const SETTINGS_SECTIONS = [
  "Claude accounts",
  "ClickUp",
  "Terminal",
  "Runs",
  "Web QA",
  "Activity",
  "Notifications",
  "Phone push",
  "Appearance",
] as const;

export type SettingsSectionName = (typeof SETTINGS_SECTIONS)[number];

/** How far below the top edge a section counts as "in view". */
const SPY_OFFSET = 24;

/**
 * The index of the section in view: the last one whose top has reached the top of the
 * page (`scrollTop` + a little), or 0. `tops` are the sections' offsets in page order; a
 * section that is not laid out (hidden by the filter) has a non-finite top and is skipped.
 */
export function currentSection(tops: readonly number[], scrollTop: number): number {
  let at = 0;
  tops.forEach((top, index) => {
    if (Number.isFinite(top) && top <= scrollTop + SPY_OFFSET) at = index;
  });
  return at;
}

/** Whether a field's `text` (its label and its controls' names) matches the filter `query`. */
export function matchesFilter(text: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const haystack = text.toLowerCase();
  return words.every((word) => haystack.includes(word));
}
