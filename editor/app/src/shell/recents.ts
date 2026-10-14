// The project switcher's recents list: up to 8 project directories, most-recent first, persisted
// across sessions. Entries are stored in the SAME form as the project setting itself
// (`toStoredProjectDir`), so a checkout-relative entry follows a checkout move.

import { sameProjectPath } from './projectDir';

/** localStorage key holding the recents list, a JSON array of stored-form project directories. */
export const RECENT_PROJECTS_KEY = 'ludion.editor.recentProjects';

/** How many recents to keep — old ones fall off the end. */
export const RECENT_PROJECTS_CAP = 8;

/** Parse a stored recents payload. Anything that isn't a JSON array of non-empty strings reads as
 *  no recents at all — never throws. */
export function parseRecents(raw: string | null): string[] {
  if (raw === null) {
    return [];
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) {
    return [];
  }
  return parsed.filter((entry): entry is string => typeof entry === 'string' && entry !== '');
}

/** `stored` moves to the front; earlier entries naming the same project (`sameProjectPath`) are
 *  removed, and the result is capped at {@link RECENT_PROJECTS_CAP}, oldest dropped first. */
export function pushRecent(list: readonly string[], stored: string): string[] {
  const rest = list.filter((entry) => !sameProjectPath(entry, stored));
  return [stored, ...rest].slice(0, RECENT_PROJECTS_CAP);
}
