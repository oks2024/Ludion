// The state card every stage viewport shows INSTEAD of the canvas — title, detail, a spinner on the
// `busy` tone — with its actions as children. Extracted out of `StageViewport` (`Plan/editor-open-
// project.md` decision 5) so the connect panel's run-game card renders the exact same look with
// Run/Attach/Replace buttons instead of a restart button.

import type { ReactNode } from 'react';
import type { StageSlot } from './stage';

interface StageSlotCardProps {
  readonly slot: StageSlot;
  readonly children?: ReactNode;
}

export function StageSlotCard({ slot, children }: StageSlotCardProps) {
  return (
    <div className={`vx-slot ${slot.tone}`}>
      <span className="vx-slot-title">
        {slot.tone === 'busy' && <span className="reconnect-spinner" aria-hidden="true" />}
        {slot.title}
      </span>
      <p className="vx-slot-detail">{slot.detail}</p>
      {children}
    </div>
  );
}
