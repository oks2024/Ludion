// The view switcher row: one tab per registered view. The active view is App-owned (it swaps the
// main-row layout), so this is a controlled component. The tab set is driven by the typed VIEWS registry.

import { VIEWS } from './views';

interface ViewSwitcherProps {
  readonly active: string;
  readonly onSelect: (viewId: string) => void;
}

export function ViewSwitcher({ active, onSelect }: ViewSwitcherProps) {
  return (
    <div className="viewswitch">
      <div className="vs-tabs" role="tablist" aria-label="Editor views">
        {VIEWS.map((view, i) => {
          const isActive = view.id === active;
          const className = `vs-tab${isActive ? ' active' : ''}${view.enabled ? '' : ' disabled'}`;
          return (
            <button
              key={view.id}
              type="button"
              role="tab"
              aria-selected={isActive}
              aria-disabled={!view.enabled}
              className={className}
              title={view.enabled ? undefined : view.note}
              onClick={() => view.enabled && onSelect(view.id)}
            >
              {view.label}
              <span className="kc">{i + 1}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
