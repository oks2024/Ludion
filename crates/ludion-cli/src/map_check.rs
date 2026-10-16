//! Implementation of `ludion map-check <map.yaml>... [--anchor <name>] [--radius <r>] [--at x,z]
//! [--traverse [--speed <u/s>]]`.
//!
//! The map-authoring loop's mid-edit verifier (the token-efficiency lever of `Plan/map-authoring.md`
//! and the ergonomics lever of `Plan/map-tooling-ergonomics.md`): it validates a map source AND
//! prints a compact, deterministic ASCII report computed from the **generated** entities — so an
//! agent iterates edit-map → `generate-map` → `map-check` on a few hundred tokens of text, never
//! reading the generated YAML.
//!
//! Pipeline: resolve + parse the map/kit/fragments exactly as `generate-map` does, run the pure
//! `ludion_scene::generate` in memory, then report:
//! 1. **Occupancy** — expand prefab instances through the engine's own expander (an expansion error
//!    is a failure: the scene would not load), strip the result to its builtin
//!    `Transform`/`Parent`/`Collider` entities (so the game components never hit L1013) with `Parent`
//!    kept so a parented collider sits at its world pose, instantiate, `spatial::rebuild` once, and
//!    probe each lattice sample with a static-collider overlap query. 3D and rigid colliders are
//!    the physics solver's, absent from the 2D index: the summary counts them as not probed. The
//!    probe runs on a sub-cell **lattice** (`Lattice`), so a street bending through a gap narrower
//!    than a cell stays walkable; a cell is blocked when every one of its samples is. The probe
//!    world is kept for the marker lints and the `--at` query. The walker resolves per map —
//!    `--radius`/`--speed` → the kit's `probe:` block (the content's own numbers) → the built-in
//!    defaults — and the header names each source.
//! 2. **Reachability** — a 4-connected BFS over free lattice samples from the free sample nearest the
//!    `--anchor` (default: the first marker, else the grid centre); a cell is reached when any of its
//!    samples is, and walkable cells it never reaches are sealed pockets, except the void beyond the
//!    walls (blank, unblocked, unreached cells joined to the grid edge), reported as *outside*. The
//!    same BFS, run from each `Gate` marker, gives the gate-to-gate distances `--traverse` reports
//!    and the bisection `L1031` names.
//! 3. **A ruler-annotated grid, a marker/anchor/gate table, and a one-line walkability-delta count** —
//!    the report speaks both coordinate systems (world `(x, z)` and cell `(col, row)`) so the agent
//!    never hand-computes the conversion.
//! 4. **Placement lints (warnings)** — `L1028` a `Gate` the anchor cannot reach; `L1029` a marker on
//!    a cell blocked by *another* prop or in a sealed pocket (its own collider excluded); `L1031` two
//!    gates with no walkable path between them (a bisected map); `L1032` a `Residue`-marked marker
//!    with no explicit `name:` (its identity is index-derived, so it re-binds on the next edit).
//!    Heuristic (except `L1032`, a structural check), so warnings: they surface the bug in-loop
//!    without failing the command.
//! 5. **Cross-map gate links (L1027)** — every generated `Gate` component's `to`+`anchor` resolved
//!    against the sibling map (generated in memory) or a committed sibling scene. The only **error**
//!    check (it is provable), and the only one that flips the exit code.
//!
//! The report prints regardless of outcome (it is the tool's value); the command exits non-zero when
//! any diagnostic — a load/parse/generate error or an L1027 dangling gate — was raised. Placement
//! warnings never change the exit code. Multiple map paths are processed in turn, aggregating the
//! exit code (non-zero if any input fails).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_core::time::{FrameIndex, TickRate};
use ludion_ecs::{SystemCtx, World};
use ludion_scene::{InstantiateOptions, MapGrid, MapMarker, Scene, instantiate_expanded};
use ludion_spatial::{Body, Collider, Shape};
use serde_json::Value;

use crate::{diagnostic, generate_map, placement};

/// The walker speed `--speed` falls back to when neither the flag nor the kit names one, in world
/// units per second. The figure the gabarit bands of a private plan are quoted at.
pub const DEFAULT_SPEED: f64 = 6.0;

/// The occupancy-probe radius `--radius` falls back to when neither the flag nor the kit names one.
pub const DEFAULT_RADIUS: f64 = 0.5;

/// Where a resolved probe number came from, printed in the report header so a reader knows whether
/// the gabarit was measured with the game's own walker or with a generic default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Flag,
    Kit,
    Default,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Flag => "flag",
            Source::Kit => "kit",
            Source::Default => "default",
        }
    }
}

/// Resolution order for a probe number: an explicit flag beats the kit's `probe:` block, which beats
/// the built-in default.
fn resolve(flag: Option<f64>, kit: Option<f64>, default: f64) -> (f64, Source) {
    match (flag, kit) {
        (Some(value), _) => (value, Source::Flag),
        (None, Some(value)) => (value, Source::Kit),
        (None, None) => (default, Source::Default),
    }
}

/// Runs `ludion map-check <maps>... [--anchor] [--radius] [--at] [--traverse] [--speed]`, one report
/// per map, aggregating the exit code (non-zero if any map failed).
pub fn run(
    maps: &[PathBuf],
    anchor: Option<&str>,
    radius: Option<f64>,
    at: Option<&str>,
    traverse: bool,
    speed: Option<f64>,
) -> ExitCode {
    // `--at` parses once, up front: a bad value is a usage error for the whole invocation.
    let at = match at.map(parse_at).transpose() {
        Ok(at) => at,
        Err(message) => {
            return diagnostic::usage_error(message);
        }
    };
    // The flags are validated up front (a usage error for the whole invocation); the kit's own
    // values were validated at parse time (`L1022`), so a resolved number is always positive.
    if let Some(speed) = speed
        && !(speed.is_finite() && speed > 0.0)
    {
        return diagnostic::usage_error(format_args!(
            "--speed must be a positive number of world units per second, got {speed}"
        ));
    }
    if let Some(radius) = radius
        && !(radius.is_finite() && radius > 0.0)
    {
        return diagnostic::usage_error(format_args!(
            "--radius must be a positive number of world units, got {radius}"
        ));
    }

    let mut failed = false;
    for (i, map) in maps.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let options = Options {
            anchor,
            radius,
            at,
            traverse,
            speed,
        };
        if run_one(map, &options) == ExitCode::FAILURE {
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Parses the `--at` value: a single `x,z` token (one comma-joined argument, so it needs no shell
/// arg-splitting). Returns a readable message on any malformed input.
fn parse_at(value: &str) -> Result<[f64; 2], String> {
    let (x, z) = value.split_once(',').ok_or_else(|| {
        format!("--at expects `x,z` (a single comma-joined token), got `{value}`")
    })?;
    let x = x
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("--at x is not a number: `{}`", x.trim()))?;
    let z = z
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("--at z is not a number: `{}`", z.trim()))?;
    Ok([x, z])
}

/// The per-invocation switches, carried whole so one report's inputs travel as one value.
struct Options<'a> {
    anchor: Option<&'a str>,
    /// The `--radius` / `--speed` flags as given: `None` defers to the kit, then to the default.
    radius: Option<f64>,
    at: Option<[f64; 2]>,
    traverse: bool,
    speed: Option<f64>,
}

/// Renders `expand_scene`'s errors where their source lives. A pointer into the generated scene's
/// fragment entities (copied first, verbatim, in `fragments:` order) becomes a fragment-local pointer
/// reported against that fragment's file; anything else stays on the map.
fn report_expand_errors(
    map_path: &Path,
    loaded: &generate_map::LoadedMap,
    diagnostics: Vec<Diagnostic>,
) -> ExitCode {
    let dir = map_path.parent().unwrap_or_else(|| Path::new("."));
    let mut groups: Vec<(PathBuf, Vec<Diagnostic>)> = Vec::new();
    for mut diagnostic in diagnostics {
        let mut source = map_path.to_path_buf();
        if let Some((path, local, rest)) = fragment_origin(loaded, diagnostic.path.as_deref()) {
            diagnostic.path = Some(format!("/entities/{local}{rest}"));
            source = dir.join(path);
        }
        match groups.iter_mut().find(|(path, _)| *path == source) {
            Some((_, found)) => found.push(diagnostic),
            None => groups.push((source, vec![diagnostic])),
        }
    }
    for (source, found) in &groups {
        diagnostic::fail_on_errors(source, found);
    }
    ExitCode::FAILURE
}

/// The fragment (as written in `fragments:`), local entity index and pointer tail a generated-scene
/// pointer `/entities/{n}{rest}` falls in, when `n` is a fragment entity.
fn fragment_origin<'a>(
    loaded: &'a generate_map::LoadedMap,
    pointer: Option<&str>,
) -> Option<(&'a str, usize, String)> {
    let (entity, rest) = generate_map::entity_pointer(pointer)?;
    let (path, local) = generate_map::fragment_entity(loaded, entity)?;
    Some((path, local, rest.to_owned()))
}

/// Checks one map and prints its report.
fn run_one(map_path: &Path, options: &Options<'_>) -> ExitCode {
    let loaded = match generate_map::load(map_path) {
        Ok(loaded) => loaded,
        Err(diagnostics) => return diagnostic::report(map_path, &diagnostics),
    };
    let scene = match generate_map::generate_checked(&loaded, &ludion_engine::builtin_registry()) {
        Ok(scene) => scene,
        Err(faults) => return generate_map::report_faults(map_path, faults),
    };

    // The walker resolves per map: the kit's `probe:` block is the content's own radius and speed, so
    // a report is quoted at the numbers the game actually walks with unless a flag overrides them.
    let (radius, radius_source) = resolve(options.radius, loaded.kit.probe.radius, DEFAULT_RADIUS);
    let (speed, speed_source) = resolve(options.speed, loaded.kit.probe.speed, DEFAULT_SPEED);

    let grid = MapGrid::new(&loaded.doc);
    let root = map_path.parent().unwrap_or_else(|| Path::new("."));
    let probe = match Probe::build(root, &scene, &grid, radius as f32) {
        Ok(probe) => probe,
        Err(diagnostics) => return report_expand_errors(map_path, &loaded, diagnostics),
    };
    let markers = marker_cells(&loaded.doc, &grid);
    let anchor_cell = resolve_anchor(&scene, options.anchor, &loaded.doc, &grid);
    let (sample_reach, anchor_start) = anchor_reach(&grid, &probe.lattice, &anchor_cell);
    let reach = cell_reach(&grid, &probe.lattice, &sample_reach, &anchor_cell);
    let seal = seal_check(
        &loaded.doc,
        &grid,
        &probe,
        &sample_reach,
        &anchor_cell,
        anchor_start,
    );
    let outside = outside_cells(&loaded.doc, &grid, &probe.blocked, &reach);
    let delta_count = walkability_delta_count(&loaded.doc, &loaded.kit, &grid, &probe.blocked);

    // The gate-to-gate routes are measured unconditionally: `L1031` is a correctness lint, not a
    // report, and one BFS per gate over the lattice costs far less than the occupancy probe that
    // already ran. `--traverse` only decides whether the table prints.
    let traverse = Traverse::measure(&loaded.doc, &grid, &probe.lattice, &sample_reach, speed);

    // Heuristic placement lints (warnings — they never flip the exit code).
    let mut warnings = placement_lints(
        &loaded.doc,
        &grid,
        &probe,
        &reach,
        &sample_reach,
        &anchor_cell,
    );
    warnings.extend(traverse.lints());
    warnings.extend(seal.lint());
    warnings.extend(name_lints(&loaded.doc));
    // The cross-map gate check is the only report step that raises errors (L1027) and the only thing
    // that makes the command exit non-zero. The report still prints below it.
    let gate_diagnostics = check_gate_links(map_path, &scene);

    print_report(&ReportInputs {
        map_path,
        scene: &scene,
        loaded: &loaded,
        grid: &grid,
        probe: &probe,
        markers: &markers,
        anchor: &anchor_cell,
        reached: &reach,
        seal: &seal,
        outside: &outside,
        delta_count,
        gate_diagnostics: &gate_diagnostics,
        warnings: &warnings,
        walker: Walker {
            radius,
            radius_source,
            speed,
            speed_source,
        },
        at: options
            .at
            .map(|xz| at_query(xz, &grid, &probe, &loaded.doc)),
        traverse: options.traverse.then_some(&traverse),
    });

    if gate_diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn idx(grid: &MapGrid, row: usize, col: usize) -> usize {
    row * grid.width() + col
}

/// Lattice samples per walker radius: the 4-connected flood can miss a gap up to about `step·√2`
/// wider than the walker (it errs toward sealed), and a step of `radius / K` bounds that slack to a
/// fraction of the radius. The report's reachability is therefore a heuristic; the seal check
/// (`SEAL_STEPS_PER_RADIUS`) is the sound one.
const LATTICE_STEPS_PER_RADIUS: f64 = 2.0;

/// The most samples per cell axis, bounding the lattice for a tiny `--radius`.
const LATTICE_MAX_PER_AXIS: usize = 33;

/// The walker's sub-cell sampling lattice: every cell is `n × n` samples, `step = cell_size / n`
/// apart, each at the centre of its `step` square. `n` is odd, so a cell's centre is itself a sample.
struct Lattice {
    n: usize,
    step: f64,
    origin: [f64; 2],
    /// Lattice extent, in samples.
    width: usize,
    blocked: Vec<bool>,
}

impl Lattice {
    /// An all-free lattice over `grid` fine enough for a walker of `radius`.
    fn new(grid: &MapGrid, radius: f64) -> Self {
        let wanted = (grid.cell_size() * LATTICE_STEPS_PER_RADIUS / radius).ceil();
        let n = (wanted.min(LATTICE_MAX_PER_AXIS as f64) as usize).max(1) | 1;
        Lattice {
            n,
            step: grid.cell_size() / n as f64,
            origin: grid.origin(),
            width: grid.width() * n,
            blocked: vec![false; grid.width() * n * grid.height() * n],
        }
    }

    /// World XZ of lattice sample `sample`.
    fn point(&self, sample: usize) -> [f64; 2] {
        [
            self.origin[0] + ((sample % self.width) as f64 + 0.5) * self.step,
            self.origin[1] + ((sample / self.width) as f64 + 0.5) * self.step,
        ]
    }

    /// The lattice indices of grid cell `(row, col)`'s samples, row by row.
    fn cell_samples(&self, row: usize, col: usize) -> impl Iterator<Item = usize> + '_ {
        let n = self.n;
        (row * n..(row + 1) * n)
            .flat_map(move |r| (col * n..(col + 1) * n).map(move |c| r * self.width + c))
    }

    /// The sample whose square holds `point`, or `None` outside the lattice.
    fn sample_at(&self, point: [f64; 2]) -> Option<usize> {
        let col = ((point[0] - self.origin[0]) / self.step).floor();
        let row = ((point[1] - self.origin[1]) / self.step).floor();
        (col >= 0.0
            && row >= 0.0
            && (col as usize) < self.width
            && (row as usize) < self.blocked.len() / self.width)
            .then(|| row as usize * self.width + col as usize)
    }

    /// The grid cell a lattice sample belongs to.
    fn cell_of(&self, sample: usize) -> (usize, usize) {
        (sample / self.width / self.n, sample % self.width / self.n)
    }

    /// The free sample of `cells` nearest `point` that also satisfies `keep`; ties go to the lowest
    /// lattice index.
    fn nearest_free(
        &self,
        cells: &[(usize, usize)],
        point: [f64; 2],
        keep: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let mut best: Option<(f64, usize)> = None;
        for &(row, col) in cells {
            for sample in self.cell_samples(row, col) {
                if self.blocked[sample] || !keep(sample) {
                    continue;
                }
                let [x, z] = self.point(sample);
                let (dx, dz) = (x - point[0], z - point[1]);
                let d = dx * dx + dz * dz;
                if best.is_none_or(|(bd, bi)| d < bd || (d == bd && sample < bi)) {
                    best = Some((d, sample));
                }
            }
        }
        best.map(|(_, sample)| sample)
    }
}

// --- Occupancy ---------------------------------------------------------------------------------

/// The static-collider occupancy of the generated scene as the game loads it: prefab instances
/// expanded, `Parent` kept so a parented collider sits at its world pose. The probe world is kept
/// for the marker lints and the `--at` query.
struct Probe {
    world: World,
    /// Entity name by slot index; `EntityId::index()` addresses it directly.
    names: Vec<Option<String>>,
    /// The walker's footprint over the sub-cell lattice.
    lattice: Lattice,
    /// Per cell: every lattice sample of it is blocked.
    blocked: Vec<bool>,
    radius: f32,
    /// Colliders of the expanded scene.
    colliders: usize,
    /// Depth-0 prefab instances and the parts they expanded into.
    instances: usize,
    instance_parts: usize,
    /// Colliders the 2D index never holds (3D or rigid — the physics solver's).
    unprobed: usize,
}

impl Probe {
    /// Expands `scene` against `root` (the map's own directory, as `check-scene` does); an expansion
    /// error is provable — the generated scene would not load.
    fn build(
        root: &Path,
        scene: &Scene,
        grid: &MapGrid,
        radius: f32,
    ) -> Result<Self, Vec<Diagnostic>> {
        let registry = ludion_engine::builtin_registry();
        let expanded = ludion_engine::expand_scene(root, &registry, scene)?;

        let entities = &expanded.scene.entities;
        let colliders: Vec<&Value> = entities
            .iter()
            .filter_map(|e| e.components.iter().find(|(n, _)| n == "Collider"))
            .map(|(_, v)| v)
            .collect();
        let unprobed = colliders
            .iter()
            .filter(|v| {
                serde_json::from_value::<Collider>((**v).clone())
                    .map_or(true, |c| c.is_solver_owned())
            })
            .count();
        let top_level = || expanded.instances.iter().filter(|i| i.depth == 0);
        let mut probe = Probe {
            world: World::new(),
            names: Vec::new(),
            lattice: Lattice::new(grid, f64::from(radius)),
            blocked: vec![false; grid.cells()],
            radius,
            colliders: colliders.len(),
            instances: top_level().count(),
            instance_parts: top_level().map(|i| i.parts.len()).sum(),
            unprobed,
        };

        let stripped = placement::stripped(&expanded);
        let ids = instantiate_expanded(
            &mut probe.world,
            &stripped,
            &registry,
            &InstantiateOptions {
                placement: None,
                name_prefix: None,
                hooks: &ludion_engine::EngineHooks,
            },
        )?;
        for (id, entity) in ids.iter().zip(&stripped.scene.entities) {
            let slot = id.index() as usize;
            if probe.names.len() <= slot {
                probe.names.resize(slot + 1, None);
            }
            probe.names[slot] = entity.name.clone();
        }
        let ctx = SystemCtx::new(FrameIndex::ZERO, TickRate::DEFAULT);
        ludion_spatial::spatial::rebuild::<ludion_engine::Transform>(&mut probe.world, &ctx);

        probe.fill_lattice(grid);
        Ok(probe)
    }

    /// Probes every lattice sample, then reduces to the per-cell verdict. One query at the cell
    /// centre with the radius grown by the cell's half-diagonal clears a whole open cell at once.
    fn fill_lattice(&mut self, grid: &MapGrid) {
        let reach = f64::from(self.radius) + grid.cell_size() * std::f64::consts::SQRT_2 / 2.0;
        for row in 0..grid.height() {
            for col in 0..grid.width() {
                let [x, z] = grid.cell_center(row, col);
                if !self.overlaps_any([x as f32, z as f32], reach as f32) {
                    continue;
                }
                let mut all = true;
                let samples: Vec<usize> = self.lattice.cell_samples(row, col).collect();
                for sample in samples {
                    let [x, z] = self.lattice.point(sample);
                    let hit = self.overlaps_any([x as f32, z as f32], self.radius);
                    self.lattice.blocked[sample] = hit;
                    all &= hit;
                }
                self.blocked[idx(grid, row, col)] = all;
            }
        }
    }

    /// Whether a static collider overlaps a circle of `radius` at world `center`.
    fn overlaps_any(&self, center: [f32; 2], radius: f32) -> bool {
        ludion_spatial::spatial::overlaps_any(
            &self.world,
            Shape::Circle { radius },
            center,
            Some(Body::new_static()),
        )
    }

    /// The entity ids whose static colliders overlap a `--radius` circle at world `center`.
    fn overlap_ids(&self, center: [f32; 2]) -> Vec<EntityId> {
        ludion_spatial::spatial::overlaps(
            &self.world,
            Shape::Circle {
                radius: self.radius,
            },
            center,
            Some(Body::new_static()),
        )
    }

    /// The names of the entities overlapping a `--radius` circle at world `center`.
    fn overlap_names(&self, center: [f32; 2]) -> Vec<&str> {
        self.overlap_ids(center)
            .into_iter()
            .filter_map(|id| {
                self.names
                    .get(id.index() as usize)
                    .and_then(|n| n.as_deref())
            })
            .collect()
    }
}

/// The cells outside the map: void (a `' '` glyph, or past the authored rows), unblocked, unreached,
/// and joined to the grid edge through such cells. A void pocket walled in on all sides is not
/// outside — it is a real sealed pocket, and stays `?`.
fn outside_cells(
    doc: &ludion_scene::MapDoc,
    grid: &MapGrid,
    blocked: &[bool],
    reached: &[bool],
) -> Vec<bool> {
    let candidate = |row: usize, col: usize| {
        let i = idx(grid, row, col);
        let glyph = doc
            .rows
            .get(row)
            .and_then(|r| r.chars().nth(col))
            .unwrap_or(' ');
        glyph == ' ' && !blocked[i] && !reached[i]
    };
    let mut outside = vec![false; grid.cells()];
    let mut queue = VecDeque::new();
    for row in 0..grid.height() {
        for col in 0..grid.width() {
            let i = idx(grid, row, col);
            if grid.is_edge(row, col) && candidate(row, col) && !outside[i] {
                outside[i] = true;
                queue.push_back((row, col));
            }
        }
    }
    while let Some((row, col)) = queue.pop_front() {
        for (nr, nc) in neighbors(grid, row, col) {
            let i = idx(grid, nr, nc);
            if !outside[i] && candidate(nr, nc) {
                outside[i] = true;
                queue.push_back((nr, nc));
            }
        }
    }
    outside
}

// --- Reachability --------------------------------------------------------------------------------

/// 4-connected BFS over free lattice samples from the free sample `start`, giving every reached
/// sample its distance in lattice steps. `Some(_)` is exactly "reachable"; the distance is what
/// `--traverse` turns into world units and seconds.
fn walk_distances(lattice: &Lattice, start: Option<usize>) -> Vec<Option<u32>> {
    let mut distance = vec![None; lattice.blocked.len()];
    let Some(start) = start else {
        return distance;
    };
    let width = lattice.width;
    let mut queue = VecDeque::new();
    distance[start] = Some(0);
    queue.push_back(start);
    while let Some(sample) = queue.pop_front() {
        let next = distance[sample].expect("dequeued samples carry a distance") + 1;
        let (row, col) = (sample / width, sample % width);
        let around = [
            (row > 0).then(|| sample - width),
            (sample + width < distance.len()).then(|| sample + width),
            (col > 0).then(|| sample - 1),
            (col + 1 < width).then(|| sample + 1),
        ];
        for n in around.into_iter().flatten() {
            if distance[n].is_none() && !lattice.blocked[n] {
                distance[n] = Some(next);
                queue.push_back(n);
            }
        }
    }
    distance
}

fn neighbors(grid: &MapGrid, row: usize, col: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(4);
    if row > 0 {
        out.push((row - 1, col));
    }
    if row + 1 < grid.height() {
        out.push((row + 1, col));
    }
    if col > 0 {
        out.push((row, col - 1));
    }
    if col + 1 < grid.width() {
        out.push((row, col + 1));
    }
    out
}

// --- Seal check (L1040) --------------------------------------------------------------------------

/// Samples per walker radius on the seal lattice, finer than the reachability lattice: the slack of
/// the flood is bounded by the sample's half diagonal, a quarter radius at most.
const SEAL_STEPS_PER_RADIUS: f64 = 4.0;

/// The most seal samples per cell axis, bounding the lattice for a tiny `--radius`.
const SEAL_MAX_PER_AXIS: usize = 64;

/// The seal flood's lattice: `n × n` samples to a cell, each at the centre of its step square. Unlike
/// `Lattice` it stores nothing — the flood probes a sample only when it reaches it.
struct SealLattice {
    n: usize,
    step: f64,
    origin: [f64; 2],
    /// Lattice extent, in samples.
    width: usize,
    height: usize,
}

impl SealLattice {
    fn new(grid: &MapGrid, radius: f64) -> Self {
        let wanted = (grid.cell_size() * SEAL_STEPS_PER_RADIUS / radius).ceil();
        let n = (wanted.min(SEAL_MAX_PER_AXIS as f64) as usize).max(1);
        SealLattice {
            n,
            step: grid.cell_size() / n as f64,
            origin: grid.origin(),
            width: grid.width() * n,
            height: grid.height() * n,
        }
    }

    /// The largest distance from a point to the centre of the sample square holding it.
    fn half_diagonal(&self) -> f64 {
        self.step / std::f64::consts::SQRT_2
    }

    fn point(&self, sample: usize) -> [f64; 2] {
        [
            self.origin[0] + ((sample % self.width) as f64 + 0.5) * self.step,
            self.origin[1] + ((sample / self.width) as f64 + 0.5) * self.step,
        ]
    }

    /// The sample whose square holds `point`, or `None` outside the lattice.
    fn sample_at(&self, point: [f64; 2]) -> Option<usize> {
        let col = ((point[0] - self.origin[0]) / self.step).floor();
        let row = ((point[1] - self.origin[1]) / self.step).floor();
        (col >= 0.0 && row >= 0.0 && (col as usize) < self.width && (row as usize) < self.height)
            .then(|| row as usize * self.width + col as usize)
    }
}

enum Seal {
    Sealed,
    /// A walk from the seed to the lattice's outer ring, seed first.
    Leak {
        escape: [f64; 2],
        path: Vec<[f64; 2]>,
    },
    Unchecked(&'static str),
}

/// Whether a walker of radius `r` can go from `seed` to the lattice's frame. `free(p)` says no static
/// collider lies within `r' = r − h` of `p`, where `h = lat.half_diagonal()`; `cell_open(row, col)`
/// says the whole cell is free, so none of its samples needs probing.
///
/// **Sound:** a leak the walker fits is never reported sealed. A sample is free when no collider is
/// within `r'` (1), and the map escapes when a visited sample sits on the outer ring (2). Take any
/// continuous path with clearance ≥ `r`. Each of its points has a nearest sample within `h`, which is
/// free under (1). Consecutive nearest samples are 4-adjacent; at a corner crossing all four samples
/// are within `h` of the corner point. So the flood follows the path to the ring. False positives are
/// limited to gaps between `2r'` and `2r` wide.
///
/// The walk starts at the sample holding `seed`, else the nearest free one among its 8 neighbours. At
/// the shrunk radius the holding sample is always free (the seed is free at `r`); the fallback serves
/// the full-radius re-flood that labels a leak.
///
/// A gate's trigger never stops the walk, because a game may let a player pass a trigger without
/// crossing it (a hold gate, a key held through an arrival): a frame opening is closed by a collider
/// behind its gate.
fn seal_flood(
    lat: &SealLattice,
    seed: [f64; 2],
    cell_open: impl Fn(usize, usize) -> bool,
    free: impl Fn([f64; 2]) -> bool,
) -> Seal {
    const UNKNOWN: u8 = 0;
    const BLOCKED: u8 = 1;
    const OPEN: u8 = 2;
    const UNSEEN: u32 = u32::MAX;

    let Some(start) = lat.sample_at(seed) else {
        return Seal::Unchecked("the start lies outside the grid");
    };
    let total = lat.width * lat.height;
    let mut state = vec![UNKNOWN; total];
    let mut parent = vec![UNSEEN; total];
    // `cell_open` is a large-radius query: ask it once per cell (0 unasked, 1 mixed, 2 open).
    let cells_wide = lat.width / lat.n;
    let mut cells = vec![0u8; cells_wide * (lat.height / lat.n)];
    let probe = |sample: usize, state: &mut Vec<u8>, cells: &mut Vec<u8>| -> u8 {
        if state[sample] == UNKNOWN {
            let (row, col) = (sample / lat.width, sample % lat.width);
            let point = lat.point(sample);
            let cell = &mut cells[row / lat.n * cells_wide + col / lat.n];
            if *cell == 0 {
                *cell = if cell_open(row / lat.n, col / lat.n) {
                    2
                } else {
                    1
                };
            }
            state[sample] = if *cell == 2 {
                OPEN
            } else if !free(point) {
                BLOCKED
            } else {
                OPEN
            };
        }
        state[sample]
    };
    let on_ring = |sample: usize| {
        let (row, col) = (sample / lat.width, sample % lat.width);
        row == 0 || col == 0 || row + 1 == lat.height || col + 1 == lat.width
    };
    let leak = |escape: usize, parent: &[u32]| {
        let mut path = vec![lat.point(escape)];
        let mut at = escape;
        while parent[at] as usize != at {
            at = parent[at] as usize;
            path.push(lat.point(at));
        }
        path.reverse();
        Seal::Leak {
            escape: lat.point(escape),
            path,
        }
    };

    let start = if probe(start, &mut state, &mut cells) == OPEN {
        start
    } else {
        let (row, col) = ((start / lat.width) as isize, (start % lat.width) as isize);
        let mut best: Option<(f64, usize)> = None;
        for (dr, dc) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (-1, 1),
            (1, -1),
            (1, 1),
        ] {
            let (r, c) = (row + dr, col + dc);
            if r < 0 || c < 0 || r as usize >= lat.height || c as usize >= lat.width {
                continue;
            }
            let near = r as usize * lat.width + c as usize;
            if probe(near, &mut state, &mut cells) != OPEN {
                continue;
            }
            let [x, z] = lat.point(near);
            let (dx, dz) = (x - seed[0], z - seed[1]);
            let d = dx * dx + dz * dz;
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, near));
            }
        }
        match best {
            Some((_, near)) => near,
            None => return Seal::Unchecked("the start sample and its neighbours are blocked"),
        }
    };
    parent[start] = start as u32;
    if on_ring(start) {
        return leak(start, &parent);
    }
    let mut queue = VecDeque::from([start]);
    while let Some(sample) = queue.pop_front() {
        let (row, col) = (sample / lat.width, sample % lat.width);
        let around = [
            (row > 0).then(|| sample - lat.width),
            (row + 1 < lat.height).then(|| sample + lat.width),
            (col > 0).then(|| sample - 1),
            (col + 1 < lat.width).then(|| sample + 1),
        ];
        for next in around.into_iter().flatten() {
            if parent[next] != UNSEEN {
                continue;
            }
            match probe(next, &mut state, &mut cells) {
                BLOCKED => {}
                _ => {
                    parent[next] = sample as u32;
                    if on_ring(next) {
                        return leak(next, &parent);
                    }
                    queue.push_back(next);
                }
            }
        }
    }
    Seal::Sealed
}

/// The seal check's verdict with what the report line and the diagnostic print.
struct SealReport {
    outcome: Seal,
    /// Where the walk started, e.g. ``first marker `spawn` ``.
    label: String,
    /// The shrunk radius the flood tested (`r'`).
    radius: f64,
    /// The walker's full radius plus half a lattice diagonal: the full-radius flood can miss a gap
    /// up to twice this wide.
    walker: f64,
    step: f64,
    /// Where the leak slips through a gap the occupancy probe reads as closed, when it does.
    via: Option<[f64; 2]>,
    /// The leak survives at the full walker radius, not only at the shrunk one.
    wide: bool,
}

impl SealReport {
    fn unchecked(reason: &'static str) -> Self {
        SealReport {
            outcome: Seal::Unchecked(reason),
            label: String::new(),
            radius: 0.0,
            walker: 0.0,
            step: 0.0,
            via: None,
            wide: false,
        }
    }

    /// The `seal:` line of the report.
    fn line(&self) -> String {
        match &self.outcome {
            Seal::Sealed => format!(
                "seal: sealed from {} — no walker-wide way reaches the map's edge (sound \
                 flood: radius {:.2}, step {:.2})",
                self.label, self.radius, self.step
            ),
            Seal::Leak { .. } => "seal: LEAKS — warning L1040".to_string(),
            Seal::Unchecked(reason) => format!("seal: not checked — {reason}"),
        }
    }

    /// `L1040`, one per leaking map.
    fn lint(&self) -> Option<Diagnostic> {
        let Seal::Leak { escape, .. } = &self.outcome else {
            return None;
        };
        let near = self
            .via
            .map(|[x, z]| (fmt_num(round_tenth(x)), fmt_num(round_tenth(z))));
        let help = if self.wide {
            let leaves = near.as_ref().map_or(String::new(), |(x, z)| {
                format!(": it leaves the report's walkable area near ({x}, {z})")
            });
            format!(
                "a walker-wide way out{leaves}: close the opening with a collider behind its \
                 gate — a gate's trigger never seals it"
            )
        } else {
            let squeeze = fmt_num((2.0 * self.radius * 100.0).round() / 100.0);
            let at = near
                .as_ref()
                .map_or(String::new(), |(x, z)| format!(" near ({x}, {z})"));
            let probe_at = near.as_ref().map_or(String::new(), |(x, z)| {
                format!(" (`map-check --at {x},{z}`)")
            });
            format!(
                "it squeezes through a gap{at} between {squeeze} and about {} wide, which the \
                 walker may fit: plug it with a collider{probe_at}",
                fmt_num((2.0 * self.walker * 100.0).round() / 100.0),
            )
        };
        Some(
            Diagnostic::new(
                code::MAP_EDGE_LEAK,
                format!(
                    "the walker can leave the map: from {} a walk reaches the map's edge at \
                     ({}, {})",
                    self.label,
                    fmt_num(round_tenth(escape[0])),
                    fmt_num(round_tenth(escape[1])),
                ),
            )
            .with_help(help),
        )
    }
}

/// Where the seal walk starts: the reachability anchor's start, else the first marker (source order)
/// with free ground. Matches how the game spawns a player.
fn seal_seed(
    doc: &ludion_scene::MapDoc,
    grid: &MapGrid,
    probe: &Probe,
    anchor: &Anchor,
    anchor_start: Option<usize>,
) -> Option<([f64; 2], String)> {
    if let Some(start) = anchor_start {
        return Some((probe.lattice.point(start), anchor.label.clone()));
    }
    doc.markers.iter().enumerate().find_map(|(index, marker)| {
        let point = [marker.x, marker.z];
        let (row, col) = grid.cell_of(marker.x, marker.z)?;
        let cells: Vec<(usize, usize)> = std::iter::once((row, col))
            .chain(neighbors(grid, row, col))
            .collect();
        let sample = probe.lattice.nearest_free(&cells, point, |_| true)?;
        Some((
            probe.lattice.point(sample),
            format!("marker `{}`", marker.derived_name(index)),
        ))
    })
}

/// Runs the seal flood over the generated scene's static colliders.
fn seal_check(
    doc: &ludion_scene::MapDoc,
    grid: &MapGrid,
    probe: &Probe,
    sample_reach: &[bool],
    anchor: &Anchor,
    anchor_start: Option<usize>,
) -> SealReport {
    if grid.cells() == 0 {
        return SealReport::unchecked("the grid is empty");
    }
    let radius = f64::from(probe.radius);
    let lat = SealLattice::new(grid, radius);
    let h = lat.half_diagonal();
    let shrunk = radius - h;
    if shrunk <= 0.0 {
        return SealReport::unchecked("the walker is too small for the seal lattice");
    }
    let Some((seed, label)) = seal_seed(doc, grid, probe, anchor, anchor_start) else {
        return SealReport::unchecked("no walkable start");
    };

    let half_diagonal = grid.cell_size() * std::f64::consts::SQRT_2 / 2.0;
    let flood = |clearance: f64| {
        let cell_open = |row: usize, col: usize| {
            let centre = grid.cell_center(row, col);
            !probe.overlaps_any(
                [centre[0] as f32, centre[1] as f32],
                (clearance + half_diagonal) as f32,
            )
        };
        let free = |p: [f64; 2]| !probe.overlaps_any([p[0] as f32, p[1] as f32], clearance as f32);
        seal_flood(&lat, seed, cell_open, free)
    };
    let outcome = flood(shrunk);
    // A leak that survives at the full radius is a way out the walker fits (to within half a lattice
    // step); otherwise it squeezes through a gap between `2r'` and `2(r + h)` wide.
    let wide = matches!(outcome, Seal::Leak { .. }) && matches!(flood(radius), Seal::Leak { .. });

    // The maximal path suffix ending at the escape that the player-radius walk never reaches; its
    // first point is where the leak slips through. An escape the r-walk reaches has no via.
    let via = match &outcome {
        Seal::Leak { path, .. } => {
            let reached =
                |p: &[f64; 2]| probe.lattice.sample_at(*p).is_some_and(|s| sample_reach[s]);
            let unreached = path.iter().rev().take_while(|p| !reached(p)).count();
            (unreached > 0).then(|| path[path.len() - unreached])
        }
        _ => None,
    };
    SealReport {
        outcome,
        label,
        radius: shrunk,
        walker: radius + h,
        step: lat.step,
        via,
        wide,
    }
}

// --- Gate-to-gate traversal (--traverse / --speed, L1031) --------------------------------------

/// A `Gate` marker reduced to what a traversal measurement needs.
struct GateNode {
    /// Index into `doc.markers`, for the diagnostic's JSON Pointer.
    index: usize,
    name: String,
    /// A `Gate` with an empty `to` — authored on a built-dark map, inert at runtime. Timed like any
    /// other gate and labelled in the table.
    inert: bool,
    /// The lattice sample a player stands on to use this gate: the free sample nearest the gate in
    /// its own cell, else in the first neighbouring cell that has one. `None` when the gate is walled
    /// in on all four sides — that bug is `L1028`'s, and reporting it again as a missing path would
    /// be noise.
    approach: Option<usize>,
}

/// One gate pair's measured route.
enum Route {
    /// The shortest walkable path, in lattice steps.
    Steps(u32),
    /// Both gates stand somewhere walkable, but nothing joins the two regions — the map is bisected
    /// (`L1031`).
    Disconnected,
    /// At least one of the two gates has no walkable cell at all (already `L1028`).
    Sealed,
}

/// The gate-to-gate measurement: every unordered pair's shortest walkable route, plus the speed the
/// seconds are quoted at. Pairs are ordered by gate name — a stable key that does not move when a
/// marker is reordered in the source.
struct Traverse {
    speed: f64,
    cell_size: f64,
    /// World units per lattice step.
    step: f64,
    gates: Vec<GateNode>,
    /// `(from index, to index, route)` over `gates`, `from < to`, in ascending pair order.
    routes: Vec<(usize, usize, Route)>,
}

impl Traverse {
    fn measure(
        doc: &ludion_scene::MapDoc,
        grid: &MapGrid,
        lattice: &Lattice,
        reached: &[bool],
        speed: f64,
    ) -> Self {
        let mut gates: Vec<GateNode> = doc
            .markers
            .iter()
            .enumerate()
            .filter(|(_, m)| marker_gate_target(m).is_some() || marker_is_inert_gate(m))
            .map(|(index, m)| GateNode {
                index,
                name: m.derived_name(index),
                inert: marker_gate_target(m).is_none(),
                approach: grid.cell_of(m.x, m.z).and_then(|(row, col)| {
                    approach_sample(grid, lattice, reached, (row, col), [m.x, m.z])
                }),
            })
            .collect();
        gates.sort_by(|a, b| a.name.cmp(&b.name));

        let mut routes = Vec::new();
        for (i, from) in gates.iter().enumerate() {
            // One BFS per gate answers every pair it starts; the walkable graph is undirected, so
            // measuring each pair once from its lower-indexed end is the whole matrix.
            let distance = from
                .approach
                .map(|start| walk_distances(lattice, Some(start)));
            for (j, to) in gates.iter().enumerate().skip(i + 1) {
                let route = match (&distance, to.approach) {
                    (Some(distance), Some(sample)) => match distance[sample] {
                        Some(steps) => Route::Steps(steps),
                        None => Route::Disconnected,
                    },
                    _ => Route::Sealed,
                };
                routes.push((i, j, route));
            }
        }
        Traverse {
            speed,
            cell_size: grid.cell_size(),
            step: lattice.step,
            gates,
            routes,
        }
    }

    /// `L1031` for every gate pair nothing walkable joins. A **warning**, not an error, for the same
    /// reason as `L1028`: the search runs over the heuristic occupancy probe, where a doorway
    /// narrower than the walker already reads as blocked, and a gap only just wider than it can
    /// fall between lattice samples.
    fn lints(&self) -> Vec<Diagnostic> {
        self.routes
            .iter()
            .filter(|(_, _, route)| matches!(route, Route::Disconnected))
            .map(|(i, j, _)| {
                let from = &self.gates[*i];
                let to = &self.gates[*j];
                Diagnostic::new(
                    code::MAP_GATES_DISCONNECTED,
                    format!(
                        "gates `{}` and `{}` have no walkable path between them — the map is \
                         bisected, so a player arriving at one can never reach the other",
                        from.name, to.name
                    ),
                )
                .with_path(format!("/markers/{}", from.index))
                .with_help(
                    "open the wall between the two regions, or move the gate into the region the \
                     rest of the map reaches (`--traverse` prints every gate-to-gate distance)",
                )
            })
            .collect()
    }
}

/// The lattice sample a gate at `cell` is entered from: the free sample nearest the gate `point` in
/// its own cell, else in the first orthogonal neighbour (in `neighbors` order, deterministic) that has
/// one — preferring samples the anchor reaches, so a gate standing in a capped opening is not measured
/// from the outside neighbour.
fn approach_sample(
    grid: &MapGrid,
    lattice: &Lattice,
    reached: &[bool],
    cell: (usize, usize),
    point: [f64; 2],
) -> Option<usize> {
    let cells: Vec<(usize, usize)> = std::iter::once(cell)
        .chain(neighbors(grid, cell.0, cell.1))
        .collect();
    let nearest = |keep: &dyn Fn(usize) -> bool| {
        cells
            .iter()
            .find_map(|c| lattice.nearest_free(std::slice::from_ref(c), point, keep))
    };
    nearest(&|sample| reached[sample]).or_else(|| nearest(&|_| true))
}

/// The gabarit a crossing time falls in (a private plan, decision 1: transition ~30 s,
/// standard 60–90 s, signature 120–180 s). The buckets split at the midpoints between neighbouring
/// bands — 45 s and 105 s — with 15 s (half the transition reference) and 180 s (the design ceiling
/// for a sector crossing) as the out-of-range edges. Each label carries its band, so a value
/// inside a bucket but outside its band still reads at a glance.
fn gabarit(seconds: f64) -> &'static str {
    if seconds < 15.0 {
        "below transition (<15 s)"
    } else if seconds < 45.0 {
        "transition (~30 s)"
    } else if seconds < 105.0 {
        "standard (60-90 s)"
    } else if seconds <= 180.0 {
        "signature (120-180 s)"
    } else {
        "above signature (>180 s)"
    }
}

// --- Intended-vs-actual walkability delta ------------------------------------------------------

/// The count of cells a glyph declares walkable (no collider in its kit primitives) but that the
/// collider probe found blocked — i.e. blocked by an overhanging marker prop. Collapsed to a single
/// number (the per-cell enumeration was mostly false positives at district scale — see
/// `Plan/map-tooling-ergonomics.md`); the marker table and `--at` answer *which* cell.
fn walkability_delta_count(
    doc: &ludion_scene::MapDoc,
    kit: &ludion_scene::MapKit,
    grid: &MapGrid,
    blocked: &[bool],
) -> usize {
    let base_has_collider = doc
        .base
        .as_deref()
        .map(|g| glyph_has_collider(kit, g))
        .unwrap_or(false);

    let mut count = 0;
    for row in 0..grid.height() {
        for col in 0..grid.width() {
            if !blocked[idx(grid, row, col)] {
                continue;
            }
            // Only an authored, non-void glyph declares a walkable cell; a `bounds:` map's cells and
            // void cells declare nothing.
            let Some(glyph) = doc
                .rows
                .get(row)
                .and_then(|r| r.chars().nth(col))
                .filter(|&g| g != ' ')
            else {
                continue;
            };
            // "intended walkable" means neither the glyph's own primitives nor the base underlay
            // contributes a collider.
            if !(glyph_has_collider(kit, &glyph.to_string()) || base_has_collider) {
                count += 1;
            }
        }
    }
    count
}

/// Whether any of the kit's primitives for `glyph` carries a collider.
fn glyph_has_collider(kit: &ludion_scene::MapKit, glyph: &str) -> bool {
    kit.tiles
        .iter()
        .find(|(g, _)| g == glyph)
        .map(|(_, prims)| prims.iter().any(|p| p.collider.is_some()))
        .unwrap_or(false)
}

// --- Markers + anchor --------------------------------------------------------------------------

/// The grid cell each marker falls in, or `None` for an off-grid marker (positionally aligned with
/// `doc.markers`).
fn marker_cells(doc: &ludion_scene::MapDoc, grid: &MapGrid) -> Vec<Option<(usize, usize)>> {
    doc.markers.iter().map(|m| grid.cell_of(m.x, m.z)).collect()
}

/// The reachability anchor: an explicit `--anchor` entity name, else the first marker, else the grid
/// centre. Carries how it was chosen, for the report line.
struct Anchor {
    cell: Option<(usize, usize)>,
    /// The anchor's world position: the flood starts at the free sample nearest it.
    point: [f64; 2],
    label: String,
}

fn resolve_anchor(
    scene: &Scene,
    requested: Option<&str>,
    doc: &ludion_scene::MapDoc,
    grid: &MapGrid,
) -> Anchor {
    if let Some(name) = requested {
        if let Some(xz) = entity_xz(scene, name) {
            if let Some(cell) = grid.cell_of(xz[0], xz[1]) {
                return Anchor {
                    cell: Some(cell),
                    point: xz,
                    label: format!("`{name}`"),
                };
            }
            return grid_centre_anchor(
                grid,
                format!("`{name}` is outside the grid — using the grid centre"),
            );
        }
        return grid_centre_anchor(
            grid,
            format!("`{name}` names no entity — using the grid centre"),
        );
    }
    if let Some(marker) = doc.markers.first()
        && let Some(cell) = grid.cell_of(marker.x, marker.z)
    {
        let name = marker.derived_name(0);
        return Anchor {
            cell: Some(cell),
            point: [marker.x, marker.z],
            label: format!("first marker `{name}`"),
        };
    }
    grid_centre_anchor(grid, "grid centre".to_string())
}

fn grid_centre_anchor(grid: &MapGrid, label: String) -> Anchor {
    let cell = (grid.cells() > 0).then_some((grid.height() / 2, grid.width() / 2));
    Anchor {
        cell,
        point: cell.map_or([0.0, 0.0], |(row, col)| grid.cell_center(row, col)),
        label,
    }
}

/// Which lattice samples the anchor's walk reaches. The flood starts at the free sample nearest the
/// anchor point among the anchor's cell and its four neighbours — an anchor standing inside a prop's
/// footprint still reaches the walkable ground around it; none free means nothing is reached.
/// Also returns that start sample.
fn anchor_reach(grid: &MapGrid, lattice: &Lattice, anchor: &Anchor) -> (Vec<bool>, Option<usize>) {
    let start = anchor.cell.and_then(|(row, col)| {
        let cells: Vec<(usize, usize)> = std::iter::once((row, col))
            .chain(neighbors(grid, row, col))
            .collect();
        lattice.nearest_free(&cells, anchor.point, |_| true)
    });
    let reach = walk_distances(lattice, start)
        .iter()
        .map(Option::is_some)
        .collect();
    (reach, start)
}

/// A cell is reached when any of its samples is; the anchor's own cell always is.
fn cell_reach(
    grid: &MapGrid,
    lattice: &Lattice,
    sample_reach: &[bool],
    anchor: &Anchor,
) -> Vec<bool> {
    let mut reached = vec![false; grid.cells()];
    for sample in (0..sample_reach.len()).filter(|&i| sample_reach[i]) {
        let (row, col) = lattice.cell_of(sample);
        reached[idx(grid, row, col)] = true;
    }
    if let Some((row, col)) = anchor.cell {
        reached[idx(grid, row, col)] = true;
    }
    reached
}

/// The ground-plane XZ of the named entity's `Transform`, if it has one.
fn entity_xz(scene: &Scene, name: &str) -> Option<[f64; 2]> {
    let entity = scene
        .entities
        .iter()
        .find(|e| e.name.as_deref() == Some(name))?;
    let transform = entity
        .components
        .iter()
        .find(|(n, _)| n == "Transform")
        .map(|(_, v)| v)?;
    let translation = transform.get("translation")?.as_array()?;
    let x = translation.first()?.as_f64()?;
    let z = translation.get(2)?.as_f64()?;
    Some([x, z])
}

// --- Placement lints (L1028 / L1029) -----------------------------------------------------------

/// The heuristic placement warnings: `L1028` a `Gate` the anchor cannot reach, `L1029` a marker on a
/// cell blocked by *another* prop or in a sealed pocket. Both read the occupancy + reachability the
/// report already built; `L1029` excludes the marker's own collider.
fn placement_lints(
    doc: &ludion_scene::MapDoc,
    grid: &MapGrid,
    probe: &Probe,
    reached: &[bool],
    sample_reach: &[bool],
    anchor: &Anchor,
) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    for (index, marker) in doc.markers.iter().enumerate() {
        let Some((row, col)) = grid.cell_of(marker.x, marker.z) else {
            continue; // an out-of-bounds marker already failed `generate` as L1023.
        };
        let name = marker.derived_name(index);
        let cell = idx(grid, row, col);

        // L1028 — a Gate whose own cell and 4 neighbours the anchor's walk never reaches.
        if marker_gate_target(marker).is_some()
            && !reached[cell]
            && !neighbors(grid, row, col)
                .into_iter()
                .any(|(r, c)| reached[idx(grid, r, c)])
        {
            warnings.push(
                Diagnostic::new(
                    code::MAP_GATE_SEALED,
                    format!(
                        "gate `{name}` at cell (c{col},r{row}) cannot be reached from {} — \
                         neither its cell nor a cell beside it is on a walkable path from the anchor",
                        anchor.label
                    ),
                )
                .with_path(format!("/markers/{index}"))
                .with_help(
                    "open the wall between the gate and the playable area, or move the gate onto a \
                     reachable cell (`--anchor <name>` chooses where the walk starts)",
                ),
            );
        }

        // L1029a — a *foreign* collider lies within the probe radius of the marker's spawn point. Probed at
        // `(marker.x, marker.z)`, not the cell centre: a marker parked at a cell corner (a torch by
        // a fountous prop) is clear even when its cell centre clips the prop, and only the real spawn
        // point is the real bug. The marker's own collider is excluded (a passthrough footprint).
        if blocked_by_other(probe, [marker.x, marker.z], &name) {
            warnings.push(
                Diagnostic::new(
                    code::MAP_MARKER_BLOCKED,
                    format!(
                        "marker `{name}` at ({}, {}) / cell (c{col},r{row}) has another prop's \
                         collider within the {} probe radius of its point — it would spawn blocked",
                        fmt_num(marker.x),
                        fmt_num(marker.z),
                        probe.radius,
                    ),
                )
                .with_path(format!("/markers/{index}"))
                .with_help(
                    "move the marker off the prop (its own collider is already excluded — this is a \
                     foreign collider)",
                ),
            );
        }
        // L1029b — the marker's cell is walkable but sealed off from the reachability anchor (an
        // interior pocket nothing can walk to). Judged at the free lattice sample nearest the marker,
        // not the cell: a thin wall can seal one half of a cell the anchor reaches the other half of.
        // A marker standing inside its own collider is a footprint, not a spawn, and is skipped.
        else if !probe.blocked[cell]
            && !probe.overlaps_any([marker.x as f32, marker.z as f32], probe.radius)
            && probe
                .lattice
                .nearest_free(&[(row, col)], [marker.x, marker.z], |_| true)
                .is_some_and(|sample| !sample_reach[sample])
        {
            warnings.push(
                Diagnostic::new(
                    code::MAP_MARKER_BLOCKED,
                    format!(
                        "marker `{name}` at cell (c{col},r{row}) sits in a sealed pocket the \
                         reachability anchor never reaches"
                    ),
                )
                .with_path(format!("/markers/{index}"))
                .with_help("open the pocket, or move the marker onto a reachable cell"),
            );
        }
    }
    warnings
}

/// `L1032` — a marker whose `components:` bag authors `Residue` but has no explicit `name:`, so its
/// `Name` is index-derived and shifts if a marker is inserted or reordered above it. A pure structural
/// check (no occupancy/reachability input needed), unlike its `L1028`/`L1029` neighbours.
fn name_lints(doc: &ludion_scene::MapDoc) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    for (index, marker) in doc.markers.iter().enumerate() {
        if marker.name.is_some() {
            continue;
        }
        if !marker.components.iter().any(|(name, _)| name == "Residue") {
            continue;
        }
        let derived = marker.derived_name(index);
        warnings.push(
            Diagnostic::new(
                code::MAP_RESIDUE_MARKER_UNNAMED,
                format!(
                    "a `Residue`-marked marker has no `name:`, so its identity is the index-derived \
                     `{derived}`: residue captured under that name re-binds to whichever marker \
                     holds the index after the next edit"
                ),
            )
            .with_path(format!("/markers/{index}"))
            .with_help(
                "give the marker an explicit `name:` — residue matches entities by `Name`, and a \
                 stable name keeps a sector's memory attached to the same entity across map edits",
            ),
        );
    }
    warnings
}

/// Whether the world `point` is overlapped by a collider that does **not** belong to the marker named
/// `owner` — i.e. blocked by someone else. The marker's own entities are named `owner` or `owner-p<n>`
/// (its per-primitive suffix), so a passthrough building blocking its own cell is excluded.
fn blocked_by_other(probe: &Probe, point: [f64; 2], owner: &str) -> bool {
    let own_suffix = format!("{owner}-p");
    probe
        .overlap_names([point[0] as f32, point[1] as f32])
        .into_iter()
        .any(|name| name != owner && !name.starts_with(&own_suffix))
}

/// The `Gate` component's `to` target, if the marker carries one (a non-empty string).
fn marker_gate_target(marker: &MapMarker) -> Option<&str> {
    let gate = marker.components.iter().find(|(n, _)| n == "Gate")?;
    let to = gate.1.get("to").and_then(Value::as_str)?;
    (!to.is_empty()).then_some(to)
}

/// Whether the marker carries a `Gate` whose `to` is the empty string — an unbuilt target.
fn marker_is_inert_gate(marker: &MapMarker) -> bool {
    marker
        .components
        .iter()
        .find(|(n, _)| n == "Gate")
        .and_then(|(_, gate)| gate.get("to").and_then(Value::as_str))
        .is_some_and(str::is_empty)
}

// --- `--at` query ------------------------------------------------------------------------------

/// The resolved facts of an `--at x,z` query: the cell it falls in, its walkable/blocked verdict, and
/// the nearest marker (name + Euclidean distance).
struct AtQuery {
    xz: [f64; 2],
    cell: Option<(usize, usize)>,
    blocked: Option<bool>,
    nearest: Option<(String, f64)>,
}

/// The exact point is probed, not its cell: the walker standing at `--at`.
fn at_query(xz: [f64; 2], grid: &MapGrid, probe: &Probe, doc: &ludion_scene::MapDoc) -> AtQuery {
    let cell = grid.cell_of(xz[0], xz[1]);
    let blocked = cell.map(|_| probe.overlaps_any([xz[0] as f32, xz[1] as f32], probe.radius));
    let nearest = doc
        .markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (dx, dz) = (m.x - xz[0], m.z - xz[1]);
            let d = (dx * dx + dz * dz).sqrt();
            (m.derived_name(i), d)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    AtQuery {
        xz,
        cell,
        blocked,
        nearest,
    }
}

// --- Cross-map gate link check (L1027) ---------------------------------------------------------

/// Validates every generated `Gate` component's `to`+`anchor` against the target district. `to`
/// resolves as a sibling of the source map: first the generated `<stem>.map.yaml`, else a committed
/// `<stem>.yaml` scene. A missing target file, or a target with no entity named `anchor`, is L1027.
/// An empty `to` is skipped silently.
fn check_gate_links(map_path: &Path, scene: &Scene) -> Vec<Diagnostic> {
    let dir = map_path.parent().unwrap_or_else(|| Path::new("."));
    let mut diagnostics = Vec::new();
    for entity in &scene.entities {
        let Some(gate) = entity
            .components
            .iter()
            .find(|(n, _)| n == "Gate")
            .map(|(_, v)| v)
        else {
            continue;
        };
        let to = gate.get("to").and_then(Value::as_str).unwrap_or("");
        if to.is_empty() {
            continue;
        }
        let anchor = gate.get("anchor").and_then(Value::as_str).unwrap_or("");
        let gate_name = entity.name.as_deref().unwrap_or("<unnamed gate>");

        match resolve_target_names(dir, to) {
            None => diagnostics.push(
                Diagnostic::new(
                    code::MAP_DANGLING_GATE_ANCHOR,
                    format!(
                        "gate `{gate_name}` targets `{to}`, but no sibling map (`<name>.map.yaml`) \
                         or scene (`<name>.yaml`) for it exists next to this map"
                    ),
                )
                .with_help(
                    "author the target district's map or scene beside this one, or fix the gate's \
                     `to`",
                ),
            ),
            Some(names) if !names.iter().any(|n| n == anchor) => diagnostics.push(
                Diagnostic::new(
                    code::MAP_DANGLING_GATE_ANCHOR,
                    format!(
                        "gate `{gate_name}` targets `{to}` anchor `{anchor}`, which names no entity \
                         in that district (it would fail only at runtime, as L3019)"
                    ),
                )
                .with_help(format!(
                    "add a `{anchor}`-named entity (an inbound anchor marker) to the target \
                     district, or correct the gate's `anchor`"
                )),
            ),
            Some(_) => {}
        }
    }
    diagnostics
}

/// The entity names of the district `to` points at: the generated `<stem>.map.yaml` if it exists and
/// generates, else a committed `<stem>.yaml` scene. `None` when neither resolves.
fn resolve_target_names(dir: &Path, to: &str) -> Option<Vec<String>> {
    let stem = Path::new(to).file_stem()?.to_string_lossy().into_owned();

    // The generated sibling map, when it exists and generates. If it exists but fails to generate,
    // the chain falls through to a committed scene.
    let map_path = dir.join(format!("{stem}.map.yaml"));
    if map_path.exists()
        && let Ok(loaded) = generate_map::load(&map_path)
        && let Ok(target) = ludion_scene::generate(
            &loaded.doc,
            &loaded.kit,
            &loaded.fragments,
            &ludion_engine::builtin_registry(),
        )
    {
        return Some(entity_names(&target));
    }

    let scene_path = dir.join(format!("{stem}.yaml"));
    if scene_path.exists()
        && let Ok(text) = std::fs::read_to_string(&scene_path)
        && let Ok(scene) = ludion_scene::parse(&text)
    {
        return Some(entity_names(&scene));
    }
    None
}

fn entity_names(scene: &Scene) -> Vec<String> {
    scene
        .entities
        .iter()
        .filter_map(|e| e.name.clone())
        .collect()
}

// --- Report rendering --------------------------------------------------------------------------

struct ReportInputs<'a> {
    map_path: &'a Path,
    scene: &'a Scene,
    loaded: &'a generate_map::LoadedMap,
    grid: &'a MapGrid,
    probe: &'a Probe,
    markers: &'a [Option<(usize, usize)>],
    anchor: &'a Anchor,
    reached: &'a [bool],
    seal: &'a SealReport,
    outside: &'a [bool],
    delta_count: usize,
    gate_diagnostics: &'a [Diagnostic],
    warnings: &'a [Diagnostic],
    walker: Walker,
    at: Option<AtQuery>,
    /// The gate-to-gate table, present only under `--traverse` (it is always measured — see
    /// `run_one` — but only printed on request, so the default report stays the same size).
    traverse: Option<&'a Traverse>,
}

/// The resolved walker the whole report was measured with, plus where each number came from.
#[derive(Clone, Copy)]
struct Walker {
    radius: f64,
    radius_source: Source,
    speed: f64,
    speed_source: Source,
}

fn print_report(r: &ReportInputs<'_>) {
    // Warnings then errors print first, at the top (rustc-style, on stderr), then the report on
    // stdout. Placement lints are warnings (a heuristic never fails the command); gate links are
    // errors that also flip the exit code.
    for warning in r.warnings {
        diagnostic::print(warning);
    }
    for gate in r.gate_diagnostics {
        diagnostic::print(gate);
    }

    let grid = r.grid;
    let mut out = String::new();
    out.push_str(&format!("map-check {}\n\n", r.map_path.display()));
    out.push_str(&format!(
        "probe: radius {} ({}), speed {} u/s ({})\n",
        fmt_num(r.walker.radius),
        r.walker.radius_source.label(),
        fmt_num(r.walker.speed),
        r.walker.speed_source.label(),
    ));

    if grid.cells() == 0 {
        out.push_str("grid: empty (no rows)\n");
        print_summary(&mut out, r, 0, 0);
        print!("{out}");
        return;
    }

    out.push_str(&format!(
        "grid {}x{} ({} cells), cell_size {}\n",
        grid.width(),
        grid.height(),
        grid.cells(),
        fmt_num(grid.cell_size()),
    ));
    render_grid(&mut out, r);
    let outside = r.outside.iter().filter(|&&o| o).count();
    out.push_str("  legend: # blocked  . walkable  M marker  A anchor  ? unreachable");
    if outside > 0 {
        out.push_str("  (blank) outside");
    }
    out.push_str("\n\n");

    let unreachable = (0..grid.cells())
        .filter(|&i| !r.probe.blocked[i] && !r.reached[i] && !r.outside[i])
        .count();
    print_summary(&mut out, r, unreachable, outside);

    out.push_str(&format!(
        "reachability: from {} — {} unreachable walkable cell{}\n",
        r.anchor.label,
        unreachable,
        if unreachable == 1 { "" } else { "s" },
    ));
    out.push_str(&r.seal.line());
    out.push('\n');
    out.push_str(&format!(
        "walkability delta: {} cell{} blocked by an overhanging prop\n",
        r.delta_count,
        if r.delta_count == 1 { "" } else { "s" },
    ));

    let gate_count = r
        .scene
        .entities
        .iter()
        .filter(|e| e.components.iter().any(|(n, _)| n == "Gate"))
        .count();
    out.push_str(&format!(
        "gate links: {} checked, {} dangling\n",
        gate_count,
        r.gate_diagnostics.len(),
    ));

    render_marker_table(&mut out, r);

    if let Some(traverse) = r.traverse {
        render_traverse(&mut out, traverse);
    }

    if let Some(at) = &r.at {
        render_at(&mut out, at);
    }

    print!("{out}");
}

/// The annotated grid with a column ruler (every 5th column) and a row-index prefix (every 5th row).
fn render_grid(out: &mut String, r: &ReportInputs<'_>) {
    let grid = r.grid;
    let label_w = row_label_width(grid.height());
    let gutter = label_w + 2;

    // Column ruler: a left-aligned number every 5 columns, each occupying its 5-column span.
    let mut ruler = String::new();
    for c in (0..grid.width()).step_by(5) {
        ruler.push_str(&format!("{c:<5}"));
    }
    out.push_str(&" ".repeat(gutter));
    out.push_str(ruler.trim_end());
    out.push('\n');

    let marker_set = |row: usize, col: usize| r.markers.contains(&Some((row, col)));
    for row in 0..grid.height() {
        if row % 5 == 0 {
            out.push_str(&format!("{row:>label_w$}  "));
        } else {
            out.push_str(&" ".repeat(gutter));
        }
        for col in 0..grid.width() {
            let i = idx(grid, row, col);
            let glyph = if r.anchor.cell == Some((row, col)) {
                'A'
            } else if marker_set(row, col) {
                'M'
            } else if r.outside[i] {
                ' '
            } else if !r.probe.blocked[i] && !r.reached[i] {
                '?'
            } else if r.probe.blocked[i] {
                '#'
            } else {
                '.'
            };
            out.push(glyph);
        }
        out.push('\n');
    }
}

/// The digit count of the largest labelled row index (the largest multiple of 5 that is a valid row),
/// minimum 1 — sizes the right-aligned row-prefix column.
fn row_label_width(height: usize) -> usize {
    if height == 0 {
        return 1;
    }
    let last_label = ((height - 1) / 5) * 5;
    last_label.to_string().len()
}

/// The marker/anchor/gate table: one line per marker in source order, each showing its kind (or
/// `Gate → <target>`), world `(x, z)`, and cell `(col, row)`.
fn render_marker_table(out: &mut String, r: &ReportInputs<'_>) {
    let markers = &r.loaded.doc.markers;
    out.push_str(&format!("\nmarkers ({}):\n", markers.len()));

    struct Row {
        name: String,
        kind: String,
        x: String,
        z: String,
        cell: String,
    }
    let rows: Vec<Row> = markers
        .iter()
        .enumerate()
        .map(|(i, m)| Row {
            name: m.derived_name(i),
            kind: marker_kind_label(m),
            x: fmt_num(m.x),
            z: fmt_num(m.z),
            cell: match r.markers[i] {
                Some((row, col)) => format!("(c{col},r{row})"),
                None => "(off-grid)".to_string(),
            },
        })
        .collect();

    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let kind_w = rows.iter().map(|r| r.kind.len()).max().unwrap_or(0);
    let x_w = rows.iter().map(|r| r.x.len()).max().unwrap_or(0);
    let z_w = rows.iter().map(|r| r.z.len()).max().unwrap_or(0);

    for row in &rows {
        out.push_str(&format!(
            "  {:<name_w$}  {:<kind_w$}  world ({:>x_w$}, {:>z_w$})  cell {}\n",
            row.name, row.kind, row.x, row.z, row.cell,
        ));
    }
}

/// A marker's kind column: `Gate → <target-stem>` for a gate (doubling as the gate inventory), else
/// `kind=<kind>`, else `-` for a pure-`components` marker.
fn marker_kind_label(marker: &MapMarker) -> String {
    if let Some(to) = marker_gate_target(marker) {
        let stem = Path::new(to)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| to.to_string());
        format!("Gate → {stem}")
    } else if let Some(kind) = &marker.kind {
        format!("kind={kind}")
    } else {
        "-".to_string()
    }
}

/// The `--traverse` block: one line per gate pair — the shortest walkable path as cells → world units
/// → seconds at `--speed`, plus the gabarit the seconds land in.
fn render_traverse(out: &mut String, t: &Traverse) {
    out.push_str(&format!(
        "\ntraverse: speed {} u/s, {} gate{}, {} pair{}\n",
        fmt_num(t.speed),
        t.gates.len(),
        if t.gates.len() == 1 { "" } else { "s" },
        t.routes.len(),
        if t.routes.len() == 1 { "" } else { "s" },
    ));
    out.push_str(
        "  measure: 4-connected path; a walker moving diagonally covers it in up to 1/√2 of these \
         seconds\n",
    );
    if t.routes.is_empty() {
        out.push_str("  (a gate-to-gate crossing needs two or more Gate markers)\n");
        return;
    }

    struct Row {
        from: String,
        to: String,
        /// The measured columns, or `None` for a route with no length to print.
        measure: Option<(String, String, String)>,
        verdict: String,
    }
    let rows: Vec<Row> = t
        .routes
        .iter()
        .map(|(i, j, route)| {
            let (measure, verdict) = match route {
                Route::Steps(steps) => {
                    let units = f64::from(*steps) * t.step;
                    let seconds = units / t.speed;
                    (
                        Some((
                            fmt_num(round_tenth(units / t.cell_size)),
                            fmt_num(round_tenth(units)),
                            format!("{seconds:.1}"),
                        )),
                        gabarit(seconds).to_string(),
                    )
                }
                Route::Disconnected => (None, "no walkable path (L1031)".to_string()),
                Route::Sealed => (None, "a gate has no walkable cell (L1028)".to_string()),
            };
            let (from, to) = (&t.gates[*i], &t.gates[*j]);
            let verdict = if from.inert || to.inert {
                format!("{verdict}  [inert gate: empty `to`]")
            } else {
                verdict
            };
            Row {
                from: t.gates[*i].name.clone(),
                to: t.gates[*j].name.clone(),
                measure,
                verdict,
            }
        })
        .collect();

    let name_w = t.gates.iter().map(|g| g.name.len()).max().unwrap_or(0);
    let widths = |pick: fn(&(String, String, String)) -> &String| {
        rows.iter()
            .filter_map(|r| r.measure.as_ref())
            .map(|m| pick(m).len())
            .max()
            .unwrap_or(0)
    };
    let (cell_w, unit_w, sec_w) = (widths(|m| &m.0), widths(|m| &m.1), widths(|m| &m.2));

    for row in &rows {
        out.push_str(&format!("  {:<name_w$} -> {:<name_w$}  ", row.from, row.to));
        if let Some((cells, units, seconds)) = &row.measure {
            out.push_str(&format!(
                "{cells:>cell_w$} cells  {units:>unit_w$} u  {seconds:>sec_w$} s  ",
            ));
        }
        out.push_str(&row.verdict);
        out.push('\n');
    }
}

/// The `--at x,z` block: the cell, the blocked/walkable verdict, and the nearest marker.
fn render_at(out: &mut String, at: &AtQuery) {
    let cell = match at.cell {
        Some((row, col)) => format!("cell (c{col},r{row})"),
        None => "off-grid".to_string(),
    };
    let verdict = match at.blocked {
        Some(true) => "blocked",
        Some(false) => "walkable",
        None => "off-grid",
    };
    let nearest = match &at.nearest {
        Some((name, d)) => format!("nearest marker `{name}` {d:.1}u away"),
        None => "no markers".to_string(),
    };
    out.push_str(&format!(
        "\nat ({}, {}): {}  {}  {}\n",
        fmt_num(at.xz[0]),
        fmt_num(at.xz[1]),
        cell,
        verdict,
        nearest,
    ));
}

fn print_summary(out: &mut String, r: &ReportInputs<'_>, unreachable: usize, outside: usize) {
    let (scene, loaded, grid, probe) = (r.scene, r.loaded, r.grid, r.probe);
    let blocked_count = probe.blocked.iter().filter(|&&b| b).count();
    let walkable_count = grid.cells() - blocked_count - outside;

    let fragment_count: usize = loaded.fragments.iter().map(|(_, s)| s.entities.len()).sum();
    // Regenerate with the markers stripped to split grid-origin from marker-origin entities exactly
    // (both passes are deterministic, so this is a stable second compile, not a heuristic).
    let no_markers = ludion_scene::MapDoc {
        markers: Vec::new(),
        ..loaded.doc.clone()
    };
    let grid_and_fragments = ludion_scene::generate(
        &no_markers,
        &loaded.kit,
        &loaded.fragments,
        &ludion_engine::builtin_registry(),
    )
    .map(|s| s.entities.len())
    .unwrap_or(fragment_count);
    let grid_count = grid_and_fragments.saturating_sub(fragment_count);
    let marker_count = scene.entities.len().saturating_sub(grid_and_fragments);

    out.push_str(&format!(
        "blocked {blocked_count}  walkable {walkable_count}  unreachable {unreachable}"
    ));
    if outside > 0 {
        out.push_str(&format!("  outside {outside}"));
    }
    out.push('\n');
    out.push_str(&format!(
        "entities {} (fragments {fragment_count}, grid {grid_count}, markers {marker_count}), \
         colliders {}",
        scene.entities.len(),
        probe.colliders,
    ));
    if probe.instances > 0 {
        out.push_str(&format!(
            ", prefab instances {} ({} part{})",
            probe.instances,
            probe.instance_parts,
            if probe.instance_parts == 1 { "" } else { "s" },
        ));
    }
    if probe.unprobed > 0 {
        out.push_str(&format!(
            ", {} not probed (3D or rigid — the physics solver's)",
            probe.unprobed
        ));
    }
    out.push_str("\n\n");
}

/// `value` rounded to one decimal, the precision a distance prints at.
fn round_tenth(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// A number formatted without a trailing `.0` (so `cell_size 2`, not `cell_size 2.0`).
fn fmt_num(value: f64) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: f64 = 4.0;
    const CELLS: usize = 12;
    const RADIUS: f64 = 0.9;
    const ROOM: f64 = 6.0;

    fn origin() -> [f64; 2] {
        [-CELL * CELLS as f64 / 2.0; 2]
    }

    fn seal_lattice() -> SealLattice {
        let n = (CELL * SEAL_STEPS_PER_RADIUS / RADIUS).ceil() as usize;
        SealLattice {
            n,
            step: CELL / n as f64,
            origin: origin(),
            width: CELLS * n,
            height: CELLS * n,
        }
    }

    /// Ground clear of a walker of `clearance`: a disc room at the origin joined to a strip of width
    /// `width` running past the grid edge at `degrees`, its centre line shifted `offset` sideways.
    fn room_and_strip(
        width: f64,
        degrees: f64,
        offset: f64,
        clearance: f64,
    ) -> impl Fn([f64; 2]) -> bool {
        let (sin, cos) = ludion_core::math::f64::sin_cos(degrees.to_radians());
        move |p: [f64; 2]| {
            let room = ROOM - ludion_core::math::f64::hypot(p[0], p[1]);
            let (along, across) = (p[0] * cos + p[1] * sin, -p[0] * sin + p[1] * cos - offset);
            let strip = if along >= 0.0 {
                width / 2.0 - across.abs()
            } else {
                f64::NEG_INFINITY
            };
            room.max(strip) >= clearance
        }
    }

    const ANGLES: [f64; 8] = [0.0, 15.0, 30.0, 45.0, 60.0, 90.0, 135.0, 210.0];

    fn flood(lat: &SealLattice, free: impl Fn([f64; 2]) -> bool) -> Seal {
        seal_flood(lat, [0.0, 0.0], |_, _| false, free)
    }

    /// The reachability lattice, filled from the same ground at the full walker radius.
    fn reaches_the_frame(free: impl Fn([f64; 2]) -> bool) -> bool {
        let n = ((CELL * LATTICE_STEPS_PER_RADIUS / RADIUS).ceil() as usize) | 1;
        let width = CELLS * n;
        let mut lattice = Lattice {
            n,
            step: CELL / n as f64,
            origin: origin(),
            width,
            blocked: vec![false; width * width],
        };
        for sample in 0..width * width {
            lattice.blocked[sample] = !free(lattice.point(sample));
        }
        let start = lattice.sample_at([0.0, 0.0]);
        let distance = walk_distances(&lattice, start);
        (0..width * width).any(|s| {
            let (row, col) = (s / width, s % width);
            let ring = row == 0 || col == 0 || row + 1 == width || col + 1 == width;
            ring && distance[s].is_some()
        })
    }

    /// A strip exactly walker-wide, at any angle and sub-step offset, is a leak the seal flood finds —
    /// including the ones the coarse reachability lattice reads as closed (its old blind spot).
    #[test]
    fn the_seal_flood_finds_every_walker_wide_strip() {
        let lat = seal_lattice();
        let mut missed_by_the_old_flood = 0;
        for degrees in ANGLES {
            for k in 0..8 {
                let offset = lat.step * f64::from(k) / 8.0;
                let wide = 2.0 * RADIUS + 0.001;
                let outcome = flood(
                    &lat,
                    room_and_strip(wide, degrees, offset, RADIUS - lat.half_diagonal()),
                );
                assert!(
                    matches!(outcome, Seal::Leak { .. }),
                    "a {wide} strip at {degrees} degrees, offset {offset}, must leak"
                );
                if !reaches_the_frame(room_and_strip(wide, degrees, offset, RADIUS)) {
                    missed_by_the_old_flood += 1;
                }
            }
        }
        assert!(
            missed_by_the_old_flood > 0,
            "the reachability lattice should miss at least one of these strips"
        );
    }

    /// A strip narrower than the shrunk walker's diameter is never a leak.
    #[test]
    fn the_seal_flood_seals_a_strip_narrower_than_the_shrunk_walker() {
        let lat = seal_lattice();
        let shrunk = RADIUS - lat.half_diagonal();
        for degrees in ANGLES {
            for k in 0..8 {
                let offset = lat.step * f64::from(k) / 8.0;
                let narrow = 2.0 * shrunk - 0.01;
                let outcome = flood(&lat, room_and_strip(narrow, degrees, offset, shrunk));
                assert!(
                    matches!(outcome, Seal::Sealed),
                    "{degrees} degrees, offset {offset}"
                );
            }
        }
    }

    /// The full-radius re-flood that labels a leak starts from the shrunk flood's seed, which is free
    /// at `r` while the sample holding it can sit up to `h` nearer a wall: it must not give up there.
    #[test]
    fn the_full_radius_reflood_starts_on_free_ground() {
        let lat = seal_lattice();
        let free = room_and_strip(2.0 * RADIUS + 0.5, 0.0, 0.0, RADIUS);
        let seed = (0..lat.width * lat.height)
            .map(|s| lat.point(s))
            .find_map(|c| {
                let q = [c[0] + 0.49 * lat.step, c[1]];
                (!free(c) && free(q)).then_some(q)
            })
            .expect("a seed whose holding sample is blocked");
        assert!(matches!(
            seal_flood(&lat, seed, |_, _| false, &free),
            Seal::Leak { .. }
        ));
    }
}
