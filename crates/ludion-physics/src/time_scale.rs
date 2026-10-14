//! Slowing or freezing the physics step alone: [`scale_time`](PhysicsWorld::scale_time),
//! [`freeze`](PhysicsWorld::freeze) and [`last_step_dt`](PhysicsWorld::last_step_dt). See the
//! crate's `# Time scale` docs for the contract.

use rapier3d::prelude::{ImpulseJointHandle, Real, SpringCoefficients};

use crate::world::{PhysicsError, PhysicsWorld};

/// The smallest scale [`scale_time`](PhysicsWorld::scale_time) accepts: below it the solver's
/// inverse timestep overflows and a step stops meaning anything. Use
/// [`freeze`](PhysicsWorld::freeze) to stop the step.
pub const MIN_TIME_SCALE: f32 = 1e-3;

impl PhysicsWorld {
    /// Runs this frame's physics step at `scale` of the fixed timestep, `scale` in
    /// `[MIN_TIME_SCALE, 1]`; call it every frame the slowdown should last, since the scale resets
    /// to `1.0` after the step.
    ///
    /// Several calls in one frame compose by minimum, in any order. The step runs with
    /// `base_dt × scale` and the base timestep is restored after it, so nothing about a tick after
    /// the slowed one changes. Only the physics step is scaled: the sim clock, the tick count and
    /// every game system keep their rate. Velocities are unchanged, so a body covers a scaled
    /// step's distance in proportion; resting contact impulses and joint break thresholds are
    /// compared per step, so they shrink with `dt`; an impact's impulse still lands on its `Started` (summed over one base
    /// tick of simulated time, so the event arrives up to `1 / scale` ticks late), and a joint that
    /// breaks under sustained load at full speed may hold in slow motion; a body's sleep timer
    /// counts simulated seconds, so it needs `1 / scale` times the ticks. A joint corrects its drift
    /// at the full-speed rate per simulated second (not amplified by `1 / scale`), so a ragdoll
    /// landing into a slowdown settles as it would unscaled.
    ///
    /// Kinematic bodies are driven from the ECS pose and are not scaled: one that reaches its full
    /// per-tick target inside a scaled step is inferred to move at `1 / scale` times its speed and
    /// pushes rigid bodies that hard. Scale your own kinematic motion by the same factor.
    ///
    /// A `scale` outside the range or non-finite is [`PhysicsError::QueryInput`] and nothing
    /// changes — use [`freeze`](PhysicsWorld::freeze) to stop the step.
    ///
    /// A scale is not part of the snapshot: [`to_bytes`](PhysicsWorld::to_bytes) refuses a world
    /// holding one below `1.0`, so call this from a system that runs before the physics write
    /// system (under a scene `physics:` block every game system does), and derive it from state
    /// your own hashed components carry.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::{PhysicsError, PhysicsWorld};
    ///
    /// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.scale_time(0.25).is_ok());
    /// assert!(matches!(world.scale_time(0.0), Err(PhysicsError::QueryInput(_))));
    /// assert!(matches!(world.scale_time(1.5), Err(PhysicsError::QueryInput(_))));
    /// assert!(matches!(world.scale_time(1e-4), Err(PhysicsError::QueryInput(_))));
    /// ```
    ///
    /// A hit-stop and a slow-motion blast, driven by a timer component the game hashes: the
    /// system re-derives the freeze or the scale from it every frame, so a replay or a restore
    /// reproduces it. The blast lands on the first frozen frame and changes the ball's velocity,
    /// not its pose; the ball then crawls at a quarter of the step until the timer runs out.
    /// ```
    /// use ludion_core::time::{FrameIndex, TickRate};
    /// use ludion_ecs::{Schedule, SystemCtx, World};
    /// use ludion_physics::{
    ///     Blast, Falloff, PhysicsPose, PhysicsSettings, PhysicsSpace, PhysicsWorld, QueryFilter,
    ///     install,
    /// };
    /// use ludion_spatial::{Body, Collider, RigidBody, Shape};
    /// use schemars::JsonSchema;
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Transform {
    ///     translation: [f32; 3],
    ///     rotation: [f32; 4],
    /// }
    /// impl PhysicsPose for Transform {
    ///     fn translation(&self) -> [f32; 3] { self.translation }
    ///     fn set_translation(&mut self, t: [f32; 3]) { self.translation = t; }
    ///     fn rotation_quat(&self) -> [f32; 4] { self.rotation }
    ///     fn set_rotation_quat(&mut self, r: [f32; 4]) { self.rotation = r; }
    /// }
    ///
    /// /// Frames of hit-stop still to run, then frames of slow motion.
    /// #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy)]
    /// struct Beats {
    ///     hit_stop: u32,
    ///     slow_mo: u32,
    /// }
    ///
    /// fn feel(world: &mut World, ctx: &SystemCtx) {
    ///     if world.resource::<PhysicsWorld>().is_none() {
    ///         return; // the resource appears on the first tick that claims an entity
    ///     }
    ///     let Some((beats,)) = world.query::<(&mut Beats,)>().next() else { return };
    ///     let (freeze, slow) = if beats.hit_stop > 0 {
    ///         beats.hit_stop -= 1;
    ///         (true, false)
    ///     } else if beats.slow_mo > 0 {
    ///         beats.slow_mo -= 1;
    ///         (false, true)
    ///     } else {
    ///         (false, false)
    ///     };
    ///     let physics = world.resource_mut::<PhysicsWorld>().unwrap();
    ///     if ctx.frame.as_u64() == 1 {
    ///         let blast = Blast {
    ///             center: [0.0, -1.0, 0.0],
    ///             radius: 3.0,
    ///             impulse: 2.0,
    ///             falloff: Falloff::Constant,
    ///             filter: QueryFilter::new(),
    ///         };
    ///         physics.radial_impulse(blast).unwrap();
    ///     }
    ///     if freeze {
    ///         physics.freeze();
    ///     } else if slow {
    ///         physics.scale_time(0.25).unwrap();
    ///     }
    /// }
    ///
    /// let mut world = World::new();
    /// let at_origin = Transform { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] };
    /// let ball = world.spawn();
    /// world.insert(ball, at_origin).unwrap();
    /// let body = Body::Rigid(RigidBody { mass: 1.0, ..RigidBody::default() });
    /// let collider = Collider { shape: Shape::Sphere { radius: 0.5 }, body, layer: None };
    /// world.insert(ball, collider).unwrap();
    /// let director = world.spawn();
    /// world.insert(director, Beats { hit_stop: 3, slow_mo: 4 }).unwrap();
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_system("feel", feel);
    /// let gravityless = PhysicsSettings {
    ///     gravity: [0.0; 3],
    ///     ..PhysicsSettings::new(PhysicsSpace::Free)
    /// };
    /// install::<Transform>(&mut schedule, gravityless);
    /// let mut frame = 0;
    /// let mut run = |world: &mut World, frames: u64| {
    ///     for _ in 0..frames {
    ///         let ctx = SystemCtx::new(FrameIndex::new(frame), TickRate::DEFAULT);
    ///         schedule.run(world, &ctx);
    ///         frame += 1;
    ///     }
    ///     let y = world.get::<Transform>(ball).unwrap().unwrap().translation[1];
    ///     (y, world.resource::<PhysicsWorld>().unwrap().last_step_dt())
    /// };
    ///
    /// // Frame 0 runs untouched; frames 1..=3 are the hit-stop.
    /// let (y, dt) = run(&mut world, 4);
    /// assert_eq!((y, dt), (0.0, 0.0));
    /// // Frames 4..=7 step at a quarter of the timestep.
    /// let (y, dt) = run(&mut world, 4);
    /// let base = TickRate::DEFAULT.seconds_per_tick() as f32;
    /// assert_eq!(dt, base * 0.25);
    /// assert!((y - 2.0 * base).abs() < 1e-4, "2 m/s for four quarter steps");
    /// // The timer is spent: the step runs at the full timestep again.
    /// let (y, dt) = run(&mut world, 1);
    /// assert_eq!(dt, base);
    /// assert!((y - 4.0 * base).abs() < 1e-4);
    /// ```
    pub fn scale_time(&mut self, scale: f32) -> Result<(), PhysicsError> {
        if !(scale.is_finite() && (MIN_TIME_SCALE..=1.0).contains(&scale)) {
            return Err(PhysicsError::QueryInput(format!(
                "scale_time: scale must be finite and in [{MIN_TIME_SCALE}, 1], got {scale}; use \
                 freeze() to stop the step"
            )));
        }
        self.time_scale = self.time_scale.min(scale);
        Ok(())
    }

    /// Skips this frame's physics step outright — the solver is never stepped with a zero
    /// timestep. Call it every frame the freeze should last, like
    /// [`scale_time`](PhysicsWorld::scale_time); a freeze wins over any scale.
    ///
    /// The rest of the physics triple still runs on a frozen frame: the write system applies
    /// queued impulses and motors (so a body's velocity changes but its pose does not) and the
    /// mirror copies the result back. The step's products are empty —
    /// [`contact_events`](PhysicsWorld::contact_events) and
    /// [`joint_events`](PhysicsWorld::joint_events) — while [`blast_hits`](PhysicsWorld::blast_hits)
    /// keeps the impulses that really applied. Contacts that start on the frozen frame's pose are
    /// reported by the first step that runs. Like a scale, a pending freeze makes
    /// [`to_bytes`](PhysicsWorld::to_bytes) refuse.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let mut world = PhysicsWorld::new(TickRate::DEFAULT);
    /// world.freeze();
    /// assert!(world.to_bytes().is_err()); // the freeze has not been stepped yet
    /// ```
    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    /// The timestep the last step slot ran with, in seconds: `base_dt × scale`, or `0.0` when the
    /// slot was frozen. A world that has not stepped reports the base timestep.
    ///
    /// Informational. It rides the snapshot; under a scene `physics:` block a game system reads
    /// the previous frame's value. Gate gameplay on your own hashed slow-motion state, as the
    /// scale itself is, not on this.
    ///
    /// # Example
    /// ```
    /// use ludion_core::time::TickRate;
    /// use ludion_physics::PhysicsWorld;
    ///
    /// let world = PhysicsWorld::new(TickRate::DEFAULT);
    /// assert!(world.last_step_dt() > 0.0);
    /// ```
    pub fn last_step_dt(&self) -> f32 {
        self.last_step_dt
    }
}

impl PhysicsWorld {
    /// Retunes every attached joint's softness so a step at `scale` of the full timestep applies the
    /// same lock-row gain as the full step, and returns the coefficients to restore after it.
    ///
    /// Rapier turns a locked axis's error into a bias velocity `err × erp_inv_dt` with no clamp, and
    /// the joint default makes `erp_inv_dt` about `1 / substep`, so an impact's joint stretch would
    /// become `1 / scale` times the full-step velocity and throw the bodies. Holding `erp_inv_dt` at
    /// its full-step value means solving the natural frequency for the scaled substep. The substep
    /// is `base_dt / num_solver_iterations` since Ludion sets no per-body extra solver groups.
    pub(crate) fn scale_joint_softness(
        &mut self,
        base_dt: f32,
        scale: f32,
    ) -> Vec<(ImpulseJointHandle, SpringCoefficients<Real>)> {
        let substep = base_dt
            / self
                .rapier
                .integration_parameters
                .num_solver_iterations
                .max(1) as f32;
        let mut saved = Vec::new();
        for handle in self.joints.iter().filter_map(|entry| entry.handle) {
            let Some(joint) = self.rapier.impulse_joints.get_mut(handle, false) else {
                continue;
            };
            let original = joint.data.softness;
            let gain = original.erp_inv_dt(substep);
            let angular = 2.0 * original.damping_ratio * gain / (1.0 - substep * scale * gain);
            joint.data.softness.natural_frequency = angular / core::f32::consts::TAU;
            saved.push((handle, original));
        }
        saved
    }

    /// Puts back what [`scale_joint_softness`](PhysicsWorld::scale_joint_softness) replaced, by
    /// handle.
    pub(crate) fn restore_joint_softness(
        &mut self,
        saved: Vec<(ImpulseJointHandle, SpringCoefficients<Real>)>,
    ) {
        for (handle, softness) in saved {
            if let Some(joint) = self.rapier.impulse_joints.get_mut(handle, false) {
                joint.data.softness = softness;
            }
        }
    }
}
