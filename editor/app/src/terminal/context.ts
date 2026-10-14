// The editor→agent context bridge (pure shaping). App writes this small JSON to
// `<projectDir>/.ludion/editor-context.json` on selection / view / scene / pointer change; the
// persona reads it to resolve deictic instructions: selection resolves "this/that" ("move this
// house"), pointer resolves "here" ("put a well here").

export interface ContextSelection {
  readonly id: { readonly index: number; readonly generation: number };
  readonly components: Readonly<Record<string, unknown>> | null;
}

export interface ContextPointer {
  readonly position: readonly [number, number, number];
}

/** The open part in an assembly's `assembly` block: what the parts panel has selected, and the
 *  file the persona would open to match it. */
export interface ContextVoxelPart {
  readonly name: string;
  readonly path: string;
}

/** The assembly half of the voxel block (decision 6, `Plan/editor-voxel-assemblies.md`): parity of
 *  perception — enough for the persona to reproduce the human's paused pose verbatim with `ludion
 *  render --model <path> --clip <clip> --time <seconds>`. `tick`/`seconds` are `null` while the clip
 *  PLAYS: the tick moves every sim tick, and a block that named it would defeat its own emit throttle
 *  and, by the time it was read, name a pose already gone. `playing` still says so. */
export interface ContextVoxelAssembly {
  /** The assembly's own path, in the same convention `modelPath` uses. */
  readonly path: string;
  /** The parts-panel selection, or `null` when none is open. */
  readonly part: ContextVoxelPart | null;
  /** The staged clip, or `null` at the rest pose. */
  readonly clip: string | null;
  readonly tick: number | null;
  readonly seconds: number | null;
  readonly playing: boolean;
}

/** The Voxel Artist view's block. `editor/personas/voxel-artist.md` promises this exists and names
 *  its three parts, so the field names are part of that contract: which model is on the stage, which
 *  rig it is being judged under, and where the human's camera is. `modelPath` is relative to the
 *  project directory (the persona's cwd), so it opens with no further resolution.
 *
 *  `assembly` is present only when the staged model is a `kind: assembly` rig — omitted, not `null`,
 *  for a plain voxel model, so that block stays byte-identical to what it was before assemblies. */
export interface ContextVoxel {
  readonly modelPath: string | null;
  readonly rig: 'neutral' | 'night';
  readonly cameraPose: {
    readonly translation: readonly [number, number, number];
    readonly rotation: readonly [number, number, number];
  } | null;
  readonly assembly?: ContextVoxelAssembly;
}

/** The Art Director view's block. `editor/personas/art-director.md` promises this exists and names its
 *  three parts. `entity` is the load-bearing one: the rule for *which* entity a `PostProcess` edit
 *  materializes on (the `Environment` carrier if one exists, else the lowest `EntityId`) lives in the
 *  UI, so carrying its answer here gives that rule exactly one implementation and lets the persona
 *  inherit it. Field *values* are deliberately absent — one `component.get` returns them, and a second
 *  copy would only drift. */
export interface ContextArtDirector {
  readonly entity: { readonly index: number; readonly generation: number };
  /** Whether `PostProcess` is authored on that entity today, or the frame runs the shipped defaults. */
  readonly authored: boolean;
  /** Which effect row the human has open, so "make this stronger" resolves. */
  readonly effect: string | null;
}

/** The Lighter view's block. `entity` is the load-bearing one for the same reason the Art Director's
 *  is: the rule for *which* entity a `GiVolume` edit materializes on (the `Environment` carrier if
 *  one exists, else the lowest `EntityId`) lives in the UI, so carrying its answer here gives that
 *  rule exactly one implementation. `sun` says which `DirectionalLight` the frame is actually lit by
 *  where a scene authors several — the renderer takes the lowest `EntityId`, and so does the tab.
 *  Field *values* are deliberately absent: one `component.get` returns them. */
export interface ContextLighter {
  /** The entity a `GiVolume` edit lands on, or `null` in an empty world. */
  readonly entity: { readonly index: number; readonly generation: number } | null;
  /** Whether `GiVolume` is authored on that entity today, or the frame runs with no volume at all. */
  readonly authored: boolean;
  /** The scene's sun, or `null` when it authors no `DirectionalLight`. */
  readonly sun: { readonly index: number; readonly generation: number } | null;
}

/** Which camera the human's viewport is on, and — while it is the flown view camera — where that
 *  camera stands (`Plan/editor-stage-camera.md` decision 6). A persona's own `render.screenshot`
 *  answers through the GAME camera whatever this says; `pose` is how it knows there is a "here" to
 *  ask about with `camera: "view"`, and is `null` on `game`, where there is no override at all. */
export interface ContextCamera {
  readonly mode: 'view' | 'game';
  readonly pose: {
    readonly translation: readonly [number, number, number];
    /** Euler XYZ degrees, the engine's `Transform.rotation` convention. */
    readonly rotation: readonly [number, number, number];
  } | null;
}

export interface EditorContext {
  readonly view: string;
  readonly scenePath: string | null;
  readonly selection: ContextSelection | null;
  readonly pointer: ContextPointer | null;
  /** Present only in the Voxel Artist view; `null` everywhere else, so a persona reading a stale
   *  file can tell "no model on a stage" from "a model I should still be looking at". */
  readonly voxel: ContextVoxel | null;
  /** Present only in the Art Director view; `null` everywhere else — and also `null` there until the
   *  tab has resolved a target, which an empty world never yields. */
  readonly artDirector: ContextArtDirector | null;
  /** Present only in the Lighter view; `null` everywhere else. */
  readonly lighter: ContextLighter | null;
  /** The viewport's camera, or `null` when no viewport is reporting one (a view with no frame
   *  stream at all). */
  readonly camera: ContextCamera | null;
}

/** `pointer`, `voxel`, `artDirector`, `lighter` and `camera` are optional on input (defaulting to
 *  `null`) so a caller that tracks none of them keeps compiling; the written context always carries
 *  all five fields. */
export function buildEditorContext(
  input: Omit<EditorContext, 'pointer' | 'voxel' | 'artDirector' | 'lighter' | 'camera'> & {
    readonly pointer?: ContextPointer | null;
    readonly voxel?: ContextVoxel | null;
    readonly artDirector?: ContextArtDirector | null;
    readonly lighter?: ContextLighter | null;
    readonly camera?: ContextCamera | null;
  },
): EditorContext {
  return {
    view: input.view,
    scenePath: input.scenePath === '' ? null : input.scenePath,
    selection: input.selection,
    pointer: input.pointer ?? null,
    voxel: input.voxel ?? null,
    artDirector: input.artDirector ?? null,
    lighter: input.lighter ?? null,
    camera: input.camera ?? null,
  };
}

/** The statusline sink path for a terminal, under the project's git+lore-ignored `.ludion/` dir.
 *  Kept next to the context-file path convention so both live in one place. */
export function statusSinkPath(projectDir: string, termId: string): string {
  return `${projectDir}/.ludion/claude-status-${termId}.json`;
}
