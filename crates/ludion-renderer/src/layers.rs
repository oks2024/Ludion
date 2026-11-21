//! Render layers on the CPU: per-item layer masks, the filter that keeps the items a mask meets, and the
//! split of a full extracted scene into the main frame and the source its secondary views draw from.
//! No GPU here; `ludion docs render-views` is the guide.

use crate::scene::{DecalReceivers, RenderScene, StaticRange};

/// The bit of the `default` layer: the layer an item is in when it names none.
pub const DEFAULT_LAYER_BIT: u32 = 1;

/// One layer mask per list of a [`RenderScene`], each parallel to its list, plus the main camera's mask.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LayerMasks {
    /// The layers the main camera draws; `u32::MAX` draws every layer.
    pub main: u32,
    /// Parallel to [`RenderScene::meshes`].
    pub meshes: Vec<u32>,
    /// Parallel to [`RenderScene::blended`].
    pub blended: Vec<u32>,
    /// Parallel to [`RenderScene::sprites`].
    pub sprites: Vec<u32>,
    /// Parallel to [`RenderScene::ground_markers`].
    pub ground_markers: Vec<u32>,
    /// Parallel to [`RenderScene::drop_shadows`].
    pub drop_shadows: Vec<u32>,
    /// Parallel to [`RenderScene::decals`].
    pub decals: Vec<u32>,
    /// Parallel to [`RenderScene::particles`].
    pub particles: Vec<u32>,
    /// Parallel to [`RenderScene::texts`].
    pub texts: Vec<u32>,
}

/// The full scene before the main camera's filter, and every item's mask. The main [`RenderScene`] of a
/// layered frame carries it so each secondary view can filter from the full set.
#[derive(Clone, PartialEq, Debug)]
pub struct LayeredSource {
    /// The unfiltered scene (its own `layered` is `None`).
    pub scene: RenderScene,
    /// The masks of `scene`'s lists.
    pub masks: LayerMasks,
}

/// Which items of each list [`RenderScene::retain`] keeps, parallel to the scene's lists.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Keep {
    /// Parallel to [`RenderScene::meshes`].
    pub meshes: Vec<bool>,
    /// Parallel to [`RenderScene::blended`].
    pub blended: Vec<bool>,
    /// Parallel to [`RenderScene::sprites`].
    pub sprites: Vec<bool>,
    /// Parallel to [`RenderScene::ground_markers`].
    pub ground_markers: Vec<bool>,
    /// Parallel to [`RenderScene::drop_shadows`].
    pub drop_shadows: Vec<bool>,
    /// Parallel to [`RenderScene::decals`].
    pub decals: Vec<bool>,
    /// Parallel to [`RenderScene::particles`].
    pub particles: Vec<bool>,
    /// Parallel to [`RenderScene::texts`].
    pub texts: Vec<bool>,
}

/// Where each item of a list went after [`RenderScene::retain`]: its new index, or `None` if dropped.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Remap {
    /// For [`RenderScene::meshes`].
    pub meshes: Vec<Option<u32>>,
    /// For [`RenderScene::blended`].
    pub blended: Vec<Option<u32>>,
    /// For [`RenderScene::sprites`].
    pub sprites: Vec<Option<u32>>,
    /// For [`RenderScene::ground_markers`].
    pub ground_markers: Vec<Option<u32>>,
    /// For [`RenderScene::drop_shadows`].
    pub drop_shadows: Vec<Option<u32>>,
    /// For [`RenderScene::decals`].
    pub decals: Vec<Option<u32>>,
    /// For [`RenderScene::particles`].
    pub particles: Vec<Option<u32>>,
    /// For [`RenderScene::texts`].
    pub texts: Vec<Option<u32>>,
}

impl Keep {
    /// Keeps every item of `scene`.
    pub fn all(scene: &RenderScene) -> Keep {
        Keep {
            meshes: vec![true; scene.meshes.len()],
            blended: vec![true; scene.blended.len()],
            sprites: vec![true; scene.sprites.len()],
            ground_markers: vec![true; scene.ground_markers.len()],
            drop_shadows: vec![true; scene.drop_shadows.len()],
            decals: vec![true; scene.decals.len()],
            particles: vec![true; scene.particles.len()],
            texts: vec![true; scene.texts.len()],
        }
    }
}

impl LayerMasks {
    /// The items whose mask shares a bit with `mask`.
    ///
    /// ```
    /// use ludion_renderer::{DEFAULT_LAYER_BIT, LayerMasks};
    ///
    /// let masks = LayerMasks { meshes: vec![DEFAULT_LAYER_BIT, 2], ..LayerMasks::default() };
    /// assert_eq!(masks.keep(DEFAULT_LAYER_BIT).meshes, [true, false]);
    /// ```
    pub fn keep(&self, mask: u32) -> Keep {
        let of = |masks: &[u32]| masks.iter().map(|m| m & mask != 0).collect();
        Keep {
            meshes: of(&self.meshes),
            blended: of(&self.blended),
            sprites: of(&self.sprites),
            ground_markers: of(&self.ground_markers),
            drop_shadows: of(&self.drop_shadows),
            decals: of(&self.decals),
            particles: of(&self.particles),
            texts: of(&self.texts),
        }
    }
}

/// Retains the items of `list` marked in `keep`, in order, and returns each old index's new one.
fn retain_by<T>(list: &mut Vec<T>, keep: &[bool]) -> Vec<Option<u32>> {
    assert_eq!(
        list.len(),
        keep.len(),
        "a layer filter is parallel to its list"
    );
    let mut next = 0u32;
    let remap = keep
        .iter()
        .map(|&kept| {
            kept.then(|| {
                next += 1;
                next - 1
            })
        })
        .collect();
    let mut flags = keep.iter();
    list.retain(|_| flags.next().copied().unwrap_or(true));
    remap
}

fn mapped(remap: &[Option<u32>], index: u32) -> Option<u32> {
    remap.get(index as usize).copied().flatten()
}

fn remap_indices(indices: &mut Vec<u32>, remap: &[Option<u32>]) {
    *indices = indices.iter().filter_map(|&i| mapped(remap, i)).collect();
}

impl RenderScene {
    /// Drops every item `keep` marks false from its list, in order, and rewrites every index that points
    /// into a filtered list: silhouette meshes (and the styles nothing uses), decal receivers, drop-shadow
    /// excludes, and the viewmodel's locals and particles. The static prefix is recomputed only when a
    /// static mesh is dropped. `mesh_assets`, `textures`, `ground_marker_profiles` and `bone_palette`
    /// are never touched. Returns each item's new index.
    ///
    /// # Panics
    /// If a `keep` list is not as long as its scene list.
    pub fn retain(&mut self, keep: &Keep) -> Remap {
        let statics = self.statics;
        let static_dropped = keep.meshes.iter().take(statics.count).any(|&kept| !kept);
        let remap = Remap {
            meshes: retain_by(&mut self.meshes, &keep.meshes),
            blended: retain_by(&mut self.blended, &keep.blended),
            sprites: retain_by(&mut self.sprites, &keep.sprites),
            ground_markers: retain_by(&mut self.ground_markers, &keep.ground_markers),
            drop_shadows: retain_by(&mut self.drop_shadows, &keep.drop_shadows),
            decals: retain_by(&mut self.decals, &keep.decals),
            particles: retain_by(&mut self.particles, &keep.particles),
            texts: retain_by(&mut self.texts, &keep.texts),
        };
        if static_dropped {
            let kept = keep.meshes[..statics.count].iter().filter(|&&k| k).count();
            self.statics =
                StaticRange::of(&self.meshes[..kept], &self.mesh_assets[..statics.assets]);
        }
        self.remap_references(&remap);
        remap
    }

    fn remap_references(&mut self, remap: &Remap) {
        let before = self.silhouettes.meshes.len();
        self.silhouettes
            .meshes
            .retain_mut(|m| match mapped(&remap.meshes, m.mesh) {
                Some(new) => {
                    m.mesh = new;
                    true
                }
                None => false,
            });
        if self.silhouettes.meshes.len() != before {
            self.drop_unused_silhouette_styles();
        }
        for decal in &mut self.decals {
            match &mut decal.receivers {
                DecalReceivers::Unskinned => {}
                DecalReceivers::Exclude(list) | DecalReceivers::Only(list) => {
                    remap_indices(list, &remap.meshes);
                }
            }
        }
        for shadow in &mut self.drop_shadows {
            remap_indices(&mut shadow.exclude, &remap.meshes);
        }
        if let Some(viewmodel) = &mut self.viewmodel {
            viewmodel
                .locals
                .retain_mut(|l| match mapped(&remap.meshes, l.mesh) {
                    Some(new) => {
                        l.mesh = new;
                        true
                    }
                    None => false,
                });
            viewmodel
                .particles
                .retain_mut(|p| match mapped(&remap.particles, p.particle) {
                    Some(new) => {
                        p.particle = new;
                        true
                    }
                    None => false,
                });
        }
    }

    /// Removes the styles no silhouette mesh names, keeping first-encounter order and renumbering the
    /// 1-based `style` of the meshes that remain.
    fn drop_unused_silhouette_styles(&mut self) {
        let silhouettes = &mut self.silhouettes;
        let mut used = vec![false; silhouettes.styles.len()];
        for m in &silhouettes.meshes {
            if let Some(slot) = used.get_mut(usize::from(m.style).wrapping_sub(1)) {
                *slot = true;
            }
        }
        let mut new_index = vec![0u8; used.len()];
        let mut next = 0u8;
        for (old, &is_used) in used.iter().enumerate() {
            if is_used {
                next += 1;
                new_index[old] = next;
            }
        }
        let mut flags = used.iter();
        silhouettes
            .styles
            .retain(|_| flags.next().copied().unwrap_or(true));
        for m in &mut silhouettes.meshes {
            m.style = new_index
                .get(usize::from(m.style).wrapping_sub(1))
                .copied()
                .unwrap_or(0);
        }
    }
}

/// Splits a full extracted scene by the main camera's mask. Returns the main scene (the items whose mask
/// meets `masks.main`, with the full scene and `masks` attached as [`RenderScene::layered`]) and the
/// remap from `full`'s list indices to the main scene's.
pub fn split_layers(full: RenderScene, masks: LayerMasks) -> (RenderScene, Remap) {
    let mut main = full.clone();
    let remap = main.retain(&masks.keep(masks.main));
    main.layered = Some(Box::new(LayeredSource { scene: full, masks }));
    (main, remap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::view_scene;
    use crate::scene::{
        DecalBlend, DecalInstance, DecalMaps, DropShadowInstance, GroundMarkerShape, MeshInstance,
        MeshShape, RenderViewData, SilhouetteMesh, SilhouetteStyle,
    };
    use crate::viewmodel::{ViewmodelData, ViewmodelLocal, ViewmodelParticle};
    use ludion_core::ids::EntityId;
    use ludion_core::math::glam::{Mat3, Mat4, Vec3};

    const RADAR: u32 = 2;

    fn mesh(x: f32) -> MeshInstance {
        MeshInstance {
            medium: 1.0,
            shape: MeshShape::Cube,
            model: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            normal_matrix: Mat3::IDENTITY,
            color: [1.0; 4],
            material: [1.0, 0.0, 1.0],
            use_vertex_material: false,
            fade: 1.0,
            fade_height_min: -f32::MAX,
            flash: 0.0,
            flash_color: [1.0; 3],
            wrap: 0.0,
            scatter_color: [1.0; 3],
            bone_offset: 0,
            cast_shadows: true,
            viewmodel: false,
            rim: [0.0; 4],
        }
    }

    fn decal(receivers: DecalReceivers) -> DecalInstance {
        DecalInstance {
            box_to_world: Mat4::IDENTITY,
            world_to_box: Mat4::IDENTITY,
            blend: DecalBlend::Multiply,
            color: [1.0; 4],
            opacity: 1.0,
            roughness: None,
            metallic: 0.0,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            angle_fade: [60.0, 80.0],
            layer: 0,
            maps: DecalMaps::default(),
            receivers,
        }
    }

    fn style(a: f32) -> SilhouetteStyle {
        SilhouetteStyle {
            color: [a; 4],
            outline_px: 0.0,
            outline_alpha: 0.0,
            bias: 0.0,
            always: false,
        }
    }

    /// Meshes at x = 0..4 with masks `[D, R, D, R]`, the first two static.
    fn scene_with_four_meshes() -> (RenderScene, LayerMasks) {
        let mut scene = RenderScene::empty((64, 64));
        scene.meshes = (0..4).map(|i| mesh(i as f32)).collect();
        scene.statics = StaticRange::of(&scene.meshes[..2], &[]);
        let masks = LayerMasks {
            main: DEFAULT_LAYER_BIT,
            meshes: vec![DEFAULT_LAYER_BIT, RADAR, DEFAULT_LAYER_BIT, RADAR],
            ..LayerMasks::default()
        };
        (scene, masks)
    }

    #[test]
    fn retain_remaps_silhouettes_decals_shadows_and_the_viewmodel() {
        let (mut scene, masks) = scene_with_four_meshes();
        scene.silhouettes.styles = vec![style(0.1), style(0.2)];
        scene.silhouettes.meshes = vec![
            SilhouetteMesh { mesh: 1, style: 1 },
            SilhouetteMesh { mesh: 2, style: 2 },
            SilhouetteMesh { mesh: 3, style: 1 },
        ];
        scene.decals = vec![
            decal(DecalReceivers::Only(vec![0, 1, 3])),
            decal(DecalReceivers::Exclude(vec![1, 2])),
            decal(DecalReceivers::Unskinned),
        ];
        scene.drop_shadows = vec![DropShadowInstance {
            shape: GroundMarkerShape::Disc { radius: 1.0 },
            anchor: Vec3::ZERO,
            yaw: 0.0,
            opacity: 1.0,
            softness: 0.0,
            max_distance: 5.0,
            fade_distance: 1.0,
            exclude: vec![1, 2, 3],
        }];
        let local = |mesh: u32| ViewmodelLocal {
            mesh,
            owner: EntityId::new(mesh, 0),
            model: Mat4::IDENTITY,
            normal_matrix: Mat3::IDENTITY,
        };
        scene.viewmodel = Some(ViewmodelData {
            fov_y_deg: 60.0,
            near: 0.01,
            depth_slice: 0.1,
            projection: Mat4::IDENTITY,
            locals: vec![local(1), local(2)],
            particles: vec![ViewmodelParticle {
                particle: 0,
                center: Vec3::ZERO,
                right: Vec3::X,
                up: Vec3::Y,
            }],
        });
        scene.drop_shadows.push(scene.drop_shadows[0].clone());
        let mut keep = Keep::all(&scene);
        keep.meshes = masks.keep(DEFAULT_LAYER_BIT).meshes;
        keep.drop_shadows = vec![true, false];
        let remap = scene.retain(&keep);
        assert_eq!(remap.meshes, [Some(0), None, Some(1), None]);
        assert_eq!(scene.meshes.len(), 2);
        // Only mesh 2 (now 1) stays a silhouette, and style 2 becomes style 1.
        assert_eq!(
            scene.silhouettes.meshes,
            [SilhouetteMesh { mesh: 1, style: 1 }]
        );
        assert_eq!(scene.silhouettes.styles.len(), 1);
        assert_eq!(scene.silhouettes.styles[0].color, [0.2; 4]);
        assert_eq!(scene.decals[0].receivers, DecalReceivers::Only(vec![0]));
        assert_eq!(scene.decals[1].receivers, DecalReceivers::Exclude(vec![1]));
        assert_eq!(scene.decals[2].receivers, DecalReceivers::Unskinned);
        assert_eq!(scene.drop_shadows.len(), 1);
        assert_eq!(scene.drop_shadows[0].exclude, [1]);
        let viewmodel = scene.viewmodel.as_ref().unwrap();
        assert_eq!(viewmodel.locals.len(), 1);
        assert_eq!(viewmodel.locals[0].mesh, 1);
        assert!(viewmodel.particles.is_empty());
    }

    #[test]
    fn statics_are_untouched_unless_one_is_removed() {
        let (mut scene, mut masks) = scene_with_four_meshes();
        let before = scene.statics;
        assert_eq!(before.count, 2);
        // Only a dynamic goes: the static range is the same value.
        masks.meshes = vec![
            DEFAULT_LAYER_BIT,
            DEFAULT_LAYER_BIT,
            DEFAULT_LAYER_BIT,
            RADAR,
        ];
        scene.retain(&masks.keep(DEFAULT_LAYER_BIT));
        assert_eq!(scene.statics, before);
        // A static goes: the prefix shrinks and is re-keyed over the kept statics.
        let (mut scene, masks) = scene_with_four_meshes();
        scene.retain(&masks.keep(DEFAULT_LAYER_BIT));
        assert_eq!(scene.statics.count, 1);
        assert_eq!(scene.statics, StaticRange::of(&scene.meshes[..1], &[]));
        assert_ne!(scene.statics.key, before.key);
    }

    #[test]
    fn split_filters_the_main_scene_and_keeps_the_full_source() {
        let (scene, masks) = scene_with_four_meshes();
        let (main, remap) = split_layers(scene.clone(), masks);
        assert_eq!(main.meshes.len(), 2);
        assert_eq!(remap.meshes, [Some(0), None, Some(1), None]);
        let source = main.layered.as_deref().unwrap();
        assert_eq!(source.scene, scene);
        assert_eq!(source.masks.meshes.len(), 4);
    }

    fn with_view(mut main: RenderScene, layers: u32) -> RenderScene {
        let camera = main.camera;
        main.views.push(RenderViewData {
            name: "v".into(),
            size: (16, 16),
            camera,
            layers,
        });
        main
    }

    #[test]
    fn a_view_keeps_its_own_layers_from_the_full_source() {
        let (scene, masks) = scene_with_four_meshes();
        let (main, _) = split_layers(with_view(scene, RADAR | DEFAULT_LAYER_BIT), masks.clone());
        let all = view_scene(&main, 0);
        assert_eq!(all.meshes.len(), 4);
        assert!(all.layered.is_none());
        let (main, _) = split_layers(with_view(scene_with_four_meshes().0, RADAR), masks);
        let only = view_scene(&main, 0);
        assert_eq!(only.meshes.len(), 2);
        assert_eq!(only.meshes[0].model.w_axis.x, 1.0);
        assert_eq!(only.meshes[1].model.w_axis.x, 3.0);
    }

    #[test]
    fn a_view_demotes_statics_when_its_set_differs_and_keeps_them_when_it_matches() {
        let (scene, masks) = scene_with_four_meshes();
        let (main, _) = split_layers(with_view(scene.clone(), RADAR), masks.clone());
        // Main kept one static (mesh 0), the view keeps the other: the keys differ.
        assert_eq!(view_scene(&main, 0).statics, StaticRange::default());
        // A view with the main camera's layers keeps the same static set.
        let (main, _) = split_layers(with_view(scene, DEFAULT_LAYER_BIT), masks);
        assert_eq!(view_scene(&main, 0).statics, main.statics);
        assert_eq!(main.statics.count, 1);
    }

    /// W1 shifted decal receiver indices when a viewmodel mesh sat before the receiver.
    #[test]
    fn dropping_a_viewmodel_mesh_keeps_a_decal_receiver_right() {
        let mut scene = RenderScene::empty((64, 64));
        let mut gun = mesh(0.0);
        gun.viewmodel = true;
        scene.meshes = vec![gun, mesh(5.0)];
        scene.decals = vec![decal(DecalReceivers::Only(vec![1]))];
        let scene = with_view(scene, u32::MAX);
        let view = view_scene(&scene, 0);
        assert_eq!(view.meshes.len(), 1);
        assert_eq!(view.meshes[0].model.w_axis.x, 5.0);
        assert_eq!(view.decals[0].receivers, DecalReceivers::Only(vec![0]));
    }
}
