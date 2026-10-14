// The PARTS panel: an assembly's hierarchy, and what the selected part is placed as.
//
// It answers the one question a rig file cannot answer by being read — which piece is this. The tree
// is the document's own (`partTree`), indented by depth and in authored order, so a row's position
// here is the position an engine diagnostic's `/parts/<i>` pointer names.
//
// THE PIVOT IS QUOTED TWICE, DELIBERATELY. `pivot` is part-local and unmirrored — the numbers in the
// file, and the frame the slice grid below draws in — while `pivotGlobal` is where that joint lands
// in the assembled grid, which is the number the rig's own comment block states and the only one two
// limbs can be compared in. A mirrored part quotes the same `pivot` as its twin and a different
// global, which is exactly the trap worth showing rather than hiding.
//
// Read-only, like everything else an assembly touches: no control here writes.

import { partTree, pivotGlobal, type AssemblyDoc, type AssemblyPart, type PartNode } from './assembly';
import type { Vec3 } from '../gizmo/vecmath';

interface PartsPanelProps {
  readonly assembly: AssemblyDoc;
  /** The selected part's name, or `null` for the whole model. */
  readonly selected: string | null;
  /** `null` deselects — the whole model is framed again. */
  readonly onSelect: (name: string | null) => void;
  /** The selected part DOCUMENT's grid extent, or `null` while it loads. Its own size, not the
   *  assembly's: a mirrored pivot is carried across the part's own width. */
  readonly partSize: Vec3 | null;
  /** The part file could not be read. */
  readonly error: string | null;
}

/** The tree flattened for rendering, depth-first in authored order. */
function flatten(nodes: readonly PartNode[]): readonly PartNode[] {
  return nodes.flatMap((node) => [node, ...flatten(node.children)]);
}

/** `parts/player-torso.part.yaml` → `player-torso.part.yaml`. The directory is the same for every
 *  part of a rig, so repeating it on eleven rows says nothing. */
function basename(file: string): string {
  return file.split(/[/\\]/).at(-1) ?? file;
}

/** `[4.5, 3, 3.5]` as `4.5 3 3.5` — the file's own spelling, never rounded: a `.5` is a cell
 *  boundary and rounding it would move a joint by half a voxel. */
function triple(value: Vec3 | null): string {
  return value === null ? '—' : value.join(' ');
}

export function PartsPanel({ assembly, selected, onSelect, partSize, error }: PartsPanelProps) {
  const rows = flatten(partTree(assembly.parts));

  return (
    <section className="hierarchy vx-parts">
      <div className="panel-head">
        Parts
        <span className="count">
          <span className="num">{assembly.parts.length}</span> placed
        </span>
      </div>

      <div className="tree-scroll">
        <ul className="tree">
          <li>
            <button
              type="button"
              className={selected === null ? 'tree-row selected' : 'tree-row'}
              style={{ paddingLeft: 8 }}
              title="Frame the whole assembly and show no part"
              onClick={() => onSelect(null)}
            >
              <span className="name">whole model</span>
            </button>
          </li>
          {rows.map((node) => {
            const isSelected = node.part.name === selected;
            return (
              <li key={`${node.index}-${node.part.name}`}>
                <button
                  type="button"
                  className={isSelected ? 'tree-row selected' : 'tree-row'}
                  style={{ paddingLeft: 8 + node.depth * 14 }}
                  title={node.part.file}
                  // Clicking the selected row again goes back to the whole model, so the tree is the
                  // only control the framing needs.
                  onClick={() => onSelect(isSelected ? null : node.part.name)}
                >
                  <span className="node-dot" />
                  <span className="name">{node.part.name}</span>
                  {node.part.mirrorX && <span className="sys-chip">mirror x</span>}
                  <span className="vx-part-file">{basename(node.part.file)}</span>
                </button>
                {isSelected && <PartDetail part={node.part} partSize={partSize} error={error} />}
              </li>
            );
          })}
        </ul>
        {assembly.parts.length === 0 && <div className="tree-empty">no parts read</div>}
      </div>
    </section>
  );
}

function PartDetail({
  part,
  partSize,
  error,
}: {
  readonly part: AssemblyPart;
  readonly partSize: Vec3 | null;
  readonly error: string | null;
}) {
  return (
    <>
      {error !== null && <div className="vx-error">{error}</div>}
      <dl className="vx-part-detail">
        <dt>size</dt>
        <dd className="num">{triple(partSize)}</dd>
        <dt>place</dt>
        <dd className="num">{triple(part.place)}</dd>
        <dt title="Part-local, in the authored (unmirrored) frame — the numbers in the file.">pivot</dt>
        <dd className="num">{triple(part.pivot)}</dd>
        <dt title="Where that joint lands in the assembled grid, mirroring applied.">in assembly</dt>
        <dd className="num">{triple(partSize === null ? null : pivotGlobal(part, partSize))}</dd>
      </dl>
    </>
  );
}
