// The Systems tab: the game's schedule as its table of contents, and — with an entity selected — the
// systems that drive it floated to the top. Reads `engine.describe` once per client (the same call the
// inspector uses for component schemas), parses its `systems` array, and partitions by the pure match
// rule (match.ts). Clicking a row opens its source in the code panel. Read-only navigation; no wire
// writes here.

import { useEffect, useState } from 'react';
import type { ProtocolClient } from '../../protocol/client';
import {
  fnNameOf,
  isUndeclared,
  moduleOf,
  parseSystems,
  partitionBySelection,
  type SystemDesc,
} from './match';

interface SystemsPanelProps {
  readonly client: ProtocolClient;
  /** The selected entity's component names, or null when nothing is selected (full schedule). */
  readonly selectionComponents: readonly string[] | null;
  /** The system whose source is open in the code panel (highlighted row), by name. */
  readonly openSystemName: string | null;
  readonly onOpenSystem: (system: SystemDesc) => void;
}

export function SystemsPanel({ client, selectionComponents, openSystemName, onOpenSystem }: SystemsPanelProps) {
  const [systems, setSystems] = useState<SystemDesc[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Fetch the schedule once per client (systems don't change without a reconnect/rebuild, which mounts
  // a fresh client and re-runs this).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const body = await client.call({ method: 'engine.describe' });
        if (!cancelled) {
          setSystems(parseSystems(body));
          setError(null);
        }
      } catch (e) {
        if (!cancelled) {
          setError(String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [client]);

  const hasSelection = selectionComponents !== null;
  const { matching, rest } = partitionBySelection(systems, selectionComponents);

  const row = (system: SystemDesc, subdued: boolean) => (
    <SysRow
      key={system.name + ':' + system.order}
      system={system}
      active={system.name === openSystemName}
      subdued={subdued}
      onOpen={() => onOpenSystem(system)}
    />
  );

  return (
    <section className="systems">
      <div className="panel-head">
        Systems
        <span className="count">
          {hasSelection ? (
            <>
              <span className="num">{matching.length}</span> driving this entity
            </>
          ) : (
            <>
              <span className="num">{systems.length}</span> in schedule
            </>
          )}
        </span>
      </div>

      {error !== null && <div className="sys-error">{error}</div>}

      <div className="sys-scroll">
        {hasSelection &&
          (matching.length > 0 ? (
            matching.map((system) => row(system, false))
          ) : (
            <div className="sys-note">No declared system matches this entity&apos;s components.</div>
          ))}

        {hasSelection && rest.length > 0 && <div className="sys-divider">not matching</div>}
        {rest.map((system) => row(system, hasSelection))}

        {systems.length === 0 && error === null && <div className="sys-note">No systems registered.</div>}
      </div>
    </section>
  );
}

/** One system row: execution order, name, its module·fn tail, and a compact query summary. */
function SysRow({
  system,
  active,
  subdued,
  onOpen,
}: {
  readonly system: SystemDesc;
  readonly active: boolean;
  readonly subdued: boolean;
  readonly onOpen: () => void;
}) {
  const module = moduleOf(system.fnPath);
  const fn = fnNameOf(system.fnPath);
  return (
    <button
      type="button"
      className={`sys-row${active ? ' active' : ''}${subdued ? ' subdued' : ''}`}
      onClick={onOpen}
      title={system.fnPath}
    >
      <span className="sys-order num">{system.order}</span>
      <span className="sys-main">
        <span className="sys-name">{system.name}</span>
        <span className="sys-fn num">{module === '' ? fn : `${module}::${fn}`}</span>
      </span>
      <QuerySummary system={system} />
    </button>
  );
}

/** The compact query summary: one chip per unique queried component (write access wins over read),
 *  or an "undeclared" badge for a system that declares no queries. */
function QuerySummary({ system }: { readonly system: SystemDesc }) {
  if (isUndeclared(system)) {
    return <span className="sys-undeclared">undeclared</span>;
  }
  const access = new Map<string, string>();
  for (const query of system.queries) {
    for (const component of query.components) {
      if (component.access === 'write' || !access.has(component.name)) {
        access.set(component.name, component.access);
      }
    }
  }
  return (
    <span className="sys-chips">
      {[...access].map(([name, acc]) => (
        <span key={name} className={`sys-chip ${acc}`} title={`${name} (${acc})`}>
          {name}
        </span>
      ))}
    </span>
  );
}
