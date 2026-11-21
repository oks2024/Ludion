//! Render layers at extract time: names become bits, an entity's mask folds down `Parent`, and the
//! main camera's and each view's layer lists become masks. `ludion docs render-views` is the guide.

use std::collections::BTreeMap;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::World;
use ludion_renderer::{DEFAULT_LAYER_BIT, InstanceKey, LayerMasks, RenderScene};

use crate::components::{Camera, DEFAULT_RENDER_LAYER, RenderLayers, RenderView};
use crate::hierarchy::InheritMemo;

/// Layer names other than `default` that one frame can use: bits 1..=31.
const MAX_NAMED_LAYERS: usize = 31;

/// The entity behind each item of a scene list, as extract collected them; each is parallel to its
/// list, or a prefix of it when the list grows after (appended projectiles and bullets are ownerless).
pub(crate) struct Owners<'a> {
    pub(crate) meshes: &'a [EntityId],
    pub(crate) blended: &'a [EntityId],
    pub(crate) sprites: &'a [Option<EntityId>],
    pub(crate) ground_markers: &'a [EntityId],
    pub(crate) drop_shadows: &'a [EntityId],
    pub(crate) decals: &'a [EntityId],
    pub(crate) particles: &'a [InstanceKey],
    pub(crate) texts: &'a [Option<EntityId>],
}

/// The layer table of one frame, present only when the scene authors a layer anywhere.
pub(crate) struct FrameLayers {
    bits: BTreeMap<String, u32>,
    main: u32,
    memo: InheritMemo<Option<u32>>,
    last: Option<(EntityId, u32)>,
}

impl FrameLayers {
    /// The frame's layer table, or `None` when no `RenderLayers`, `Camera.layers` or `RenderView.layers`
    /// is authored (the frame then takes no layer path). `editor_view` is a `render.view` override, which
    /// draws every layer in the main frame.
    ///
    /// # Errors
    /// `L2375` when the scene uses more than 31 layer names besides `default`.
    pub(crate) fn of(
        world: &World,
        ids: &[EntityId],
        editor_view: bool,
    ) -> Result<Option<FrameLayers>, Diagnostic> {
        let on_entities = world.component_count::<RenderLayers>() > 0;
        let mut names: Vec<&str> = Vec::new();
        let mut authored = on_entities;
        let mut main_camera: Option<&Camera> = None;
        let single = crate::extract::single_camera(world, ids).ok().flatten();
        for &id in ids {
            if on_entities && let Some(layers) = world.get::<RenderLayers>(id).ok().flatten() {
                names.extend(layers.layers.iter().map(String::as_str));
            }
            let Some(camera) = world.get::<Camera>(id).ok().flatten() else {
                continue;
            };
            if Some(id) == single {
                main_camera = Some(camera);
            }
            if let Some(list) = &camera.layers {
                authored = true;
                names.extend(list.iter().map(String::as_str));
            }
            if let Some(view) = world.get::<RenderView>(id).ok().flatten()
                && let Some(list) = &view.layers
            {
                authored = true;
                names.extend(list.iter().map(String::as_str));
            }
        }
        if !authored {
            return Ok(None);
        }
        let mut sorted: Vec<&str> = names
            .into_iter()
            .filter(|name| *name != DEFAULT_RENDER_LAYER)
            .collect();
        sorted.sort_unstable();
        sorted.dedup();
        if sorted.len() > MAX_NAMED_LAYERS {
            return Err(Diagnostic::new(
                code::RENDER_LAYERS_INVALID,
                format!(
                    "the scene uses {} render layer names besides `default`, but a frame has bits for {MAX_NAMED_LAYERS}",
                    sorted.len()
                ),
            )
            .with_help("reuse layer names or merge layers; `ludion explain L2375`"));
        }
        let mut bits: BTreeMap<String, u32> = BTreeMap::new();
        bits.insert(DEFAULT_RENDER_LAYER.to_string(), DEFAULT_LAYER_BIT);
        for (i, name) in sorted.into_iter().enumerate() {
            bits.insert(name.to_string(), 1 << (i + 1));
        }
        let mut layers = FrameLayers {
            bits,
            main: u32::MAX,
            memo: InheritMemo::new(),
            last: None,
        };
        if !editor_view && let Some(list) = main_camera.and_then(|c| c.layers.as_ref()) {
            layers.main = layers.mask_of(list);
        }
        Ok(Some(layers))
    }

    /// The layers `view` draws: its own list, else `default`.
    pub(crate) fn view_mask(&self, view: &RenderView) -> u32 {
        view.layers
            .as_deref()
            .map_or(DEFAULT_LAYER_BIT, |list| self.mask_of(list))
    }

    /// The mask of `id`: its own `RenderLayers`, else its nearest ancestor's, else `default`.
    pub(crate) fn entity_mask(&mut self, world: &World, id: EntityId) -> u32 {
        if let Some((entity, mask)) = self.last
            && entity == id
        {
            return mask;
        }
        let bits = &self.bits;
        let own = |entity: EntityId| {
            world
                .get::<RenderLayers>(entity)
                .ok()
                .flatten()
                .map(|l| mask_in(bits, &l.layers))
        };
        let mask = self
            .memo
            .resolve(world, id, own, |parent, own| own.or(parent), None)
            .unwrap_or(DEFAULT_LAYER_BIT);
        self.last = Some((id, mask));
        mask
    }

    /// The mask of every item of `scene`'s filtered lists, from the entity behind each. An item past its
    /// owner list (a projectile, a volley bullet) or with no owner is `default`; a viewmodel mesh or
    /// particle is in every layer.
    pub(crate) fn masks(
        &mut self,
        world: &World,
        scene: &RenderScene,
        owners: &Owners<'_>,
    ) -> LayerMasks {
        let main = self.main;
        let mut of = |len: usize, owner: &dyn Fn(usize) -> Option<EntityId>| -> Vec<u32> {
            (0..len)
                .map(|i| owner(i).map_or(DEFAULT_LAYER_BIT, |id| self.entity_mask(world, id)))
                .collect()
        };
        let meshes = of(scene.meshes.len(), &|i| owners.meshes.get(i).copied());
        let particles = of(scene.particles.len(), &|i| {
            owners.particles.get(i).map(|key| key.owner)
        });
        LayerMasks {
            main,
            meshes: meshes
                .into_iter()
                .zip(&scene.meshes)
                .map(|(mask, mesh)| if mesh.viewmodel { u32::MAX } else { mask })
                .collect(),
            blended: of(scene.blended.len(), &|i| owners.blended.get(i).copied()),
            sprites: of(scene.sprites.len(), &|i| {
                owners.sprites.get(i).copied().flatten()
            }),
            ground_markers: of(scene.ground_markers.len(), &|i| {
                owners.ground_markers.get(i).copied()
            }),
            drop_shadows: of(scene.drop_shadows.len(), &|i| {
                owners.drop_shadows.get(i).copied()
            }),
            decals: of(scene.decals.len(), &|i| owners.decals.get(i).copied()),
            particles: particles
                .into_iter()
                .zip(&scene.particles)
                .map(|(mask, p)| if p.viewmodel { u32::MAX } else { mask })
                .collect(),
            texts: of(scene.texts.len(), &|i| {
                owners.texts.get(i).copied().flatten()
            }),
        }
    }

    fn mask_of(&self, list: &[String]) -> u32 {
        mask_in(&self.bits, list)
    }
}

fn mask_in(bits: &BTreeMap<String, u32>, list: &[String]) -> u32 {
    list.iter()
        .filter_map(|name| bits.get(name))
        .fold(0, |mask, bit| mask | bit)
}
