// The lower-left panel of the Game Designer view: a two-tab surface. "Systems" is the schedule
// navigator (SystemsPanel); "Data" reuses the existing Inspector's component cards unchanged (later
// waves grow it into schema-driven doc forms). Thin wiring — both children own their own polling and
// wire reads.

import { useState } from 'react';
import type { ProtocolClient } from '../../protocol/client';
import type { EditorMode, EntityId } from '../../App';
import type { MutationReport } from '../../viewer/useComponentCommit';
import { Inspector } from '../../viewer/Inspector';
import { SystemsPanel } from './SystemsPanel';
import type { SystemDesc } from './match';

interface SystemsDataTabsProps {
  readonly client: ProtocolClient;
  readonly mode: EditorMode;
  readonly selection: EntityId | null;
  readonly onMutation: (report: MutationReport) => void;
  readonly selectionComponents: readonly string[] | null;
  readonly openSystemName: string | null;
  readonly onOpenSystem: (system: SystemDesc) => void;
}

type Tab = 'systems' | 'data';

export function SystemsDataTabs({
  client,
  mode,
  selection,
  onMutation,
  selectionComponents,
  openSystemName,
  onOpenSystem,
}: SystemsDataTabsProps) {
  const [tab, setTab] = useState<Tab>('systems');
  return (
    <section className="sysdata">
      <div className="sysdata-tabs" role="tablist" aria-label="Systems and data">
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'systems'}
          className={`sysdata-tab${tab === 'systems' ? ' active' : ''}`}
          onClick={() => setTab('systems')}
        >
          Systems
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'data'}
          className={`sysdata-tab${tab === 'data' ? ' active' : ''}`}
          onClick={() => setTab('data')}
        >
          Data
        </button>
      </div>
      <div className="sysdata-body">
        {tab === 'systems' ? (
          <SystemsPanel
            client={client}
            selectionComponents={selectionComponents}
            openSystemName={openSystemName}
            onOpenSystem={onOpenSystem}
          />
        ) : (
          <Inspector client={client} selection={selection} onMutation={onMutation} mode={mode} />
        )}
      </div>
    </section>
  );
}
