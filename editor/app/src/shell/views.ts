// The view registry: a view is a workspace preset, not a capability wall.
// A view is declared here before it is built, so it slots in without restructuring the switcher;
// `enabled: false` entries render as disabled tabs with an explanatory tooltip. Every declared view
// is built.

export interface ViewDef {
  readonly id: string;
  readonly label: string;
  readonly enabled: boolean;
  /** Tooltip shown on a disabled tab (why it isn't available yet). */
  readonly note?: string;
}

export const VIEWS: readonly ViewDef[] = [
  { id: 'level-designer', label: 'Level Designer', enabled: true },
  { id: 'game-designer', label: 'Game Designer', enabled: true },
  { id: 'voxel-artist', label: 'Voxel Artist', enabled: true },
  { id: 'models', label: 'Models', enabled: true },
  { id: 'generation', label: 'Generation', enabled: true },
  { id: 'effects-artist', label: 'Effects', enabled: true },
  { id: 'lighter', label: 'Lighter', enabled: true },
  { id: 'art-director', label: 'Art Director', enabled: true },
  { id: 'performance', label: 'Performance', enabled: true },
];
