// The `scene.changed` event's decision, extracted from `App.tsx`'s handler so the foreign-vs-own
// distinction the scene picker depends on (`Plan/editor-open-project.md` decision 7) is testable
// without a client or a subscription. Pure: `own` is `expectOwnSaveRef.current` at the moment the
// event arrives, already read and cleared by the caller.

/** Was this our own save? (Set synchronously before we issued `scene.save`.) If so, the stack still
 *  describes valid live entities -- just clear dirty. A FOREIGN change (another client loaded or
 *  saved, or a picker load, which never arms the flag) may have invalidated our ids, so drop the
 *  history and selection with it. */
export function sceneChangeOutcome(
  own: boolean,
  path: string | null | undefined,
): { resetEditState: boolean; savePath: string | null } {
  return {
    resetEditState: !own,
    savePath: path !== null && path !== undefined ? path : null,
  };
}
