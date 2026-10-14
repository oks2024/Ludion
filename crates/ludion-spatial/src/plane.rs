//! [`Plane`]: which two world axes a game's 2D queries live in.

use ludion_ecs::World;

/// The plane every 2D query lives in — collision, and any engine feature that turns a 2D angle into
/// a world direction.
///
/// A game chooses it once through the engine builder's `plane`, which inserts it as a world
/// resource; nothing reads it per frame off a hashed component, and nothing overrides it at
/// runtime. **An absent resource means [`Plane::Xz`]**, so a world built without one is a
/// ground-plane game.
///
/// [`Xz`](Plane::Xz) is the ground plane a top-down or 3D-eyed game moves on; [`Xy`](Plane::Xy) is
/// the plane sprites and tilemaps already draw on, so a side-scrolling sprite game collides where
/// its content actually sits instead of stacking every collider at one depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Plane {
    /// World `x` and `z` — the ground plane.
    #[default]
    Xz,
    /// World `x` and `y` — the plane sprites and tilemaps draw on.
    Xy,
}

impl Plane {
    /// The `[x, y, z]` indices this plane's two in-plane axes occupy: `[0, 2]` for [`Plane::Xz`],
    /// `[0, 1]` for [`Plane::Xy`]. Indexing a `[f32; 3]` translation with these yields the pair
    /// every 2D query speaks in.
    pub fn axes(self) -> [usize; 2] {
        match self {
            Plane::Xz => [0, 2],
            Plane::Xy => [0, 1],
        }
    }

    /// The world direction of the first in-plane axis — `[1, 0, 0]` on both planes.
    pub fn right(self) -> [f32; 3] {
        [1.0, 0.0, 0.0]
    }

    /// The world direction of the second in-plane axis: the way a heading of 90° points —
    /// `[0, 0, 1]` on [`Plane::Xz`], `[0, 1, 0]` on [`Plane::Xy`].
    pub fn forward(self) -> [f32; 3] {
        match self {
            Plane::Xz => [0.0, 0.0, 1.0],
            Plane::Xy => [0.0, 1.0, 0.0],
        }
    }

    /// The plane's normal — the one world axis 2D queries ignore: `[0, 1, 0]` on [`Plane::Xz`],
    /// `[0, 0, 1]` on [`Plane::Xy`].
    pub fn normal(self) -> [f32; 3] {
        match self {
            Plane::Xz => [0.0, 1.0, 0.0],
            Plane::Xy => [0.0, 0.0, 1.0],
        }
    }

    /// The plane `world` runs its 2D queries in: its [`Plane`] resource, or [`Plane::Xz`] when the
    /// resource is absent. The one read every consumer goes through.
    pub fn of(world: &World) -> Plane {
        world.resource::<Plane>().copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_index_the_in_plane_translation_components() {
        let translation = [1.0_f32, 2.0, 3.0];
        let xz = Plane::Xz.axes();
        assert_eq!([translation[xz[0]], translation[xz[1]]], [1.0, 3.0]);
        let xy = Plane::Xy.axes();
        assert_eq!([translation[xy[0]], translation[xy[1]]], [1.0, 2.0]);
    }

    #[test]
    fn basis_vectors_are_orthogonal_per_plane() {
        for plane in [Plane::Xz, Plane::Xy] {
            let (right, forward, normal) = (plane.right(), plane.forward(), plane.normal());
            for pair in [(right, forward), (forward, normal), (normal, right)] {
                let dot = pair.0[0] * pair.1[0] + pair.0[1] * pair.1[1] + pair.0[2] * pair.1[2];
                assert_eq!(dot, 0.0, "{plane:?}");
            }
        }
    }

    #[test]
    fn an_absent_resource_reads_as_the_ground_plane() {
        let mut world = World::new();
        assert_eq!(Plane::of(&world), Plane::Xz);
        world.insert_resource(Plane::Xy);
        assert_eq!(Plane::of(&world), Plane::Xy);
    }
}
