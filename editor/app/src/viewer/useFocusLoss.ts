// Calls `onLoss` when the window loses focus (`window` blur) or the page becomes hidden
// (`document` visibilitychange to `hidden`). Nothing fires on regaining focus: a released input is
// never re-pressed. The callback lives in a ref so the listeners bind once.

import { useEffect, useRef } from 'react';

export function useFocusLoss(onLoss: () => void): void {
  const onLossRef = useRef(onLoss);
  onLossRef.current = onLoss;

  useEffect(() => {
    const onBlur = (): void => onLossRef.current();
    const onVisibility = (): void => {
      if (document.visibilityState === 'hidden') {
        onLossRef.current();
      }
    };
    window.addEventListener('blur', onBlur);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('blur', onBlur);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);
}
