//! Animate a served scene over the wire — a peer client mutating the world while a viewer watches.
//!
//! This is the "one command core, many clients" pillar as a demo: `ludion serve` hosts a scene, the
//! Tauri/browser viewer watches it through a `frames` subscription, and this example — just another
//! protocol client on the same queue — drives motion with plain `component.set` commands. Nothing
//! here is privileged: every mutation is a command the viewer's inspector sees land live.
//!
//! Built for the `lights` gallery scene (three coloured point lights around a sphere and two cubes):
//! it orbits each light around the scene centre and bobs the sphere. On any other scene it degrades
//! gracefully — every entity with both `Transform` and `PointLight` orbits, and the first entity
//! with a `Mesh` whose name isn't ground-like bobs.
//!
//! Run (against the default `ludion serve` address):
//!
//! ```text
//! cargo run --release -p ludion-cli -- serve --scene examples/gallery/lights.yaml
//! cargo run --release -p ludion-client --example animate            # ws://127.0.0.1:9411, 30 s
//! cargo run --release -p ludion-client --example animate -- ws://127.0.0.1:9411 60
//! ```

use std::time::{Duration, Instant};

use ludion_client::Client;
use ludion_core::ids::EntityId;
use ludion_protocol::command::{ComponentSetParams, QueryEntitiesParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::{Value, json};

/// How often to push a `component.set` per animated entity. 30 Hz reads as smooth motion in the
/// viewer and keeps well under the wire's capacity on loopback.
const UPDATE_RATE: Duration = Duration::from_millis(33);

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "ws://127.0.0.1:9411".into());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    if let Err(diagnostic) = run(&url, seconds) {
        eprintln!("{diagnostic}");
        std::process::exit(1);
    }
}

/// One animated entity: its id, the `Transform` value it started with (rotation/scale are
/// preserved — only translation is animated), and its base translation.
struct Target {
    entity: EntityId,
    base_transform: Value,
    base_translation: [f64; 3],
    kind: Kind,
}

enum Kind {
    Orbit,
    Bob,
}

fn run(url: &str, seconds: u64) -> Result<(), ludion_core::diagnostics::Diagnostic> {
    let mut client = Client::connect(url)?;
    println!("connected to {url}");

    // Discover the scene: every entity with a Transform, with the components we animate against.
    let query = Command::QueryEntities(QueryEntitiesParams {
        filter: vec!["Transform".into()],
        include: vec![
            "Transform".into(),
            "PointLight".into(),
            "Mesh".into(),
            "Camera".into(),
        ],
        world: false,
        entities: None,
    });
    let ResponseBody::QueryResult { entities } = client.call(query)?.body else {
        unreachable!("query.entities always answers with a QueryResult");
    };

    let mut targets = Vec::new();
    let mut bob_taken = false;
    for m in entities {
        let Some(transform) = m.components.get("Transform") else {
            continue;
        };
        let kind = if m.components.contains_key("PointLight") {
            Kind::Orbit
        } else if m.components.contains_key("Mesh")
            && !m.components.contains_key("Camera")
            && !bob_taken
            // The ground plane is a Mesh too; skip anything scaled wide and flat like one.
            && !is_ground_like(transform)
        {
            bob_taken = true;
            Kind::Bob
        } else {
            continue;
        };
        targets.push(Target {
            entity: m.entity,
            base_translation: translation_of(transform),
            base_transform: transform.clone(),
            kind,
        });
    }
    if targets.is_empty() {
        println!("nothing to animate: no entity with Transform + PointLight/Mesh found");
        return Ok(());
    }
    println!(
        "animating {} entities for {seconds} s — watch the viewer",
        targets.len()
    );

    let start = Instant::now();
    let deadline = start + Duration::from_secs(seconds);
    let mut next = start;
    while Instant::now() < deadline {
        let t = start.elapsed().as_secs_f64();
        for target in &targets {
            let [x, y, z] = target.base_translation;
            let translation = match target.kind {
                // Orbit in the ground plane around the scene centre. Radius and start angle come
                // from each light's own base position, so the three lights keep their spacing.
                Kind::Orbit => {
                    let radius = (x * x + z * z).sqrt().max(1.0);
                    let angle = z.atan2(x) + 0.9 * t;
                    [radius * angle.cos(), y, radius * angle.sin()]
                }
                // Bob vertically around the base height.
                Kind::Bob => [x, y + 0.6 * (1.7 * t).sin(), z],
            };
            let mut value = target.base_transform.clone();
            value["translation"] = json!(translation);
            client.call(Command::ComponentSet(ComponentSetParams {
                entity: target.entity,
                component: "Transform".into(),
                value,
            }))?;
        }
        next += UPDATE_RATE;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }

    // Put everything back where it started, so the scene ends as it began.
    for target in &targets {
        client.call(Command::ComponentSet(ComponentSetParams {
            entity: target.entity,
            component: "Transform".into(),
            value: target.base_transform.clone(),
        }))?;
    }
    println!("done — scene restored to its starting pose");
    Ok(())
}

/// The `translation` of a `Transform` value, defaulting to the origin when absent (the component's
/// serde default).
fn translation_of(transform: &Value) -> [f64; 3] {
    let mut out = [0.0; 3];
    if let Some(list) = transform.get("translation").and_then(Value::as_array) {
        for (slot, v) in out.iter_mut().zip(list) {
            *slot = v.as_f64().unwrap_or(0.0);
        }
    }
    out
}

/// Whether a `Transform` looks like a ground plane: much wider than tall.
fn is_ground_like(transform: &Value) -> bool {
    let Some(scale) = transform.get("scale").and_then(Value::as_array) else {
        return false;
    };
    let s = |i: usize| scale.get(i).and_then(Value::as_f64).unwrap_or(1.0);
    s(0) > 4.0 * s(1) && s(2) > 4.0 * s(1)
}
