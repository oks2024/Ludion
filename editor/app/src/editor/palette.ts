// The curated entity palette:
// the archetypes a designer spawns from the panel.
//
// Each archetype is a label plus a ready-to-send `entity.spawn` (name + default components). Two rules
// shape the defaults:
//   1. Every archetype carries a `Transform` so it has a place in the world and the gizmos have
//      something to grab.
//   2. Defaults must spawn AND render without any external asset. `Sprite.texture`/`Mesh` need no path
//      (a textureless Sprite is a plain white quad, per components.rs), so no archetype references a
//      file that could be missing and wedge the render loop at "waiting for frames…" (L1101). `Text`
//      REQUIRES a non-empty `text` field (no serde default on it), so it ships with a visible literal.
//
// Values are lifted from the component defaults in `crates/ludion-engine/src/components.rs` and the
// realistic framings in `examples/gallery/*.yaml`. The entity `name:` is authored through the spawn
// `name` param, never a `Name` component key — passing both is rejected (L3014).
//
// A second `camera` in a scene that already has one surfaces the engine's multi-camera diagnostic
// (L2101) in the shared banner. That's expected and deliberately NOT special-cased here.

import type { Command } from '@ludion/protocol';

/** One palette entry: a button label and the default components its spawn writes. */
export interface Archetype {
  /** Stable id for React keys and tests (not shown to the human). */
  readonly id: string;
  /** Button label. */
  readonly label: string;
  /** The entity's `name:` (becomes the reserved `Name` component engine-side). */
  readonly name: string;
  /** Default components, keyed by registry name — always includes a `Transform`, never a `Name`. */
  readonly components: Record<string, unknown>;
}

/** Perspective projection value in wire form (`Projection` is `#[serde(tag = "type")]`). */
const PERSPECTIVE = { type: 'perspective', fov_y_deg: 60 };

/** The archetypes, in palette order (Plan/phase-6-editing.md's curated five, plus the
 *  "HUD panel" and "Sun"). The palette stays hand-maintained, not
 *  registry-driven (Plan/phase-14-ui.md decision 8) — a new component gets a palette entry only when
 *  a plan decides it should. */
export const ARCHETYPES: readonly Archetype[] = [
  {
    id: 'sprite',
    label: 'Sprite',
    name: 'Sprite',
    components: {
      // No `texture` → a plain white quad (renders with no asset).
      Transform: { translation: [0, 0, 0] },
      Sprite: { size: [1, 1], color: [1, 1, 1, 1], billboard: false },
    },
  },
  {
    id: 'mesh',
    label: 'Mesh',
    name: 'Mesh',
    components: {
      Transform: { translation: [0, 0, 0] },
      Mesh: { primitive: 'cube', color: [0.8, 0.8, 0.82, 1] },
    },
  },
  {
    id: 'light',
    label: 'Light',
    name: 'Light',
    components: {
      // Lifted off the origin so it actually lights nearby surfaces.
      Transform: { translation: [0, 3, 3] },
      PointLight: { color: [1, 1, 1], intensity: 3, range: 10 },
    },
  },
  {
    // A cone light aimed straight down from above the origin; direction and the half-angles are
    // plain fields in the generic inspector (no aiming widget).
    id: 'spot',
    label: 'Spot light',
    name: 'Spot light',
    components: {
      Transform: { translation: [0, 4, 0] },
      SpotLight: { color: [1, 1, 1], intensity: 3, range: 12, direction: [0, -1, 0], inner_angle: 20, outer_angle: 30 },
    },
  },
  {
    // The single directional "sun". Its `direction` is an explicit vector
    // (the default angled overhead sun); its `Transform.translation` anchors the shadow frustum
    // center. It lights the scene and drives the shadow map; the palette archetype lets a
    // designer drop a sun in and tune it through the generic inspector (no custom widget).
    id: 'sun',
    label: 'Sun',
    name: 'Sun',
    components: {
      Transform: { translation: [0, 0, 0] },
      DirectionalLight: { direction: [-0.3, -1, -0.2], color: [1, 1, 1], intensity: 1 },
    },
  },
  {
    id: 'camera',
    label: 'Camera',
    name: 'Camera',
    components: {
      Transform: { translation: [0, 0, 6] },
      Camera: { projection: PERSPECTIVE, near: 0.1, far: 1000 },
    },
  },
  {
    id: 'text',
    label: 'Text',
    name: 'Text',
    components: {
      // `text` is required on the wire (no serde default); a visible literal so the spawn shows up.
      Transform: { translation: [0, 0, 0] },
      Text: { text: 'TEXT', size: 1, color: [1, 1, 1, 1] },
    },
  },
  {
    // Plan/phase-14-ui.md decision 8: the one HUD archetype — everything else about screen-space
    // UI authoring stays hand-composed via the inspector, per the widget-toolkit deferral.
    id: 'hud-panel',
    label: 'HUD panel',
    name: 'HUD panel',
    components: {
      // Ignored by the UI path (screen space has no world placement) but kept for palette-entry
      // consistency — every archetype carries a Transform.
      Transform: { translation: [0, 0, 0] },
      UiRect: { anchor: 'top_left', offset: [24, 24], size: [220, 48], color: [0.1, 0.1, 0.12, 0.85] },
    },
  },
  {
    // ui-widgets: the one menu archetype — the widget-toolkit deferral stands, so this is the single
    // curated entry point for a selectable list, not a family of list variants.
    id: 'ui-list',
    label: 'Menu list',
    name: 'Menu list',
    components: {
      // Ignored by the UI path (screen space has no world placement) but kept for palette-entry
      // consistency — every archetype carries a Transform.
      Transform: { translation: [0, 0, 0] },
      // Non-empty items with one disabled so the spawn is visible and shows both item colors.
      UiList: {
        anchor: 'center',
        title: 'MENU',
        items: [
          { text: 'RESUME', enabled: true },
          { text: 'QUIT', enabled: false },
        ],
        background: [0.1, 0.1, 0.12, 0.85],
      },
    },
  },
  {
    // particles: an endless emitter (`duration: 0`) in steady state from its first frame, so the
    // placement is visible the moment it lands and needs no asset — the engine defaults are already
    // a white plume rising up a 30-degree cone. Tuning belongs to the Effects view's emitter tab
    // over the same component; this is only the ambient-source entry point for a level.
    id: 'particles',
    label: 'Particles',
    name: 'Particles',
    components: {
      Transform: { translation: [0, 0.5, 0] },
      ParticleEmitter: {},
    },
  },
  {
    // reflection-probe: a box at room scale a little above the floor, so the volume reaches below it.
    // Resize `extent` in the inspector; the engine captures the probe on spawn, no bake step.
    id: 'reflection-probe',
    label: 'Reflection probe',
    name: 'Reflection probe',
    components: {
      Transform: { translation: [0, 2, 0] },
      ReflectionProbe: { shape: { box: { extent: [4, 2, 4] } }, blend: 0.5, priority: 0 },
    },
  },
];

/** Builds the `entity.spawn` command for an archetype (name param + default components). */
export function spawnCommand(archetype: Archetype): Command {
  return { method: 'entity.spawn', params: { name: archetype.name, components: archetype.components } };
}
