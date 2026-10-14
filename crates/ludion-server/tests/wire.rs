//! Integration tests: the engine, over the wire, versus the same engine in-process.
//!
//! Every test binds port 0 (an OS-chosen free port) and lets the [`Server`] drop at scope end, which
//! must shut down cleanly — no leaked threads, no hangs. The typed [`Client`] drives the happy paths;
//! a couple of tests open a raw `tungstenite` socket to send bytes the typed client could never
//! produce (malformed JSON, an unknown method, a wrong version).

use ludion_client::Client;
use ludion_engine::{Engine, dispatch};
use ludion_protocol::command::{EntitySpawnParams, QueryEntitiesParams};
use ludion_protocol::{Command, ResponseBody};
use serde_json::json;
use std::collections::BTreeMap;
use tungstenite::Message;

/// The empty, seeded base scene both the wire and in-process engines start from.
const BASE_SCENE: &str = "format: 1\nseed: 7\nentities: []\n";

/// Builds an engine with the base scene loaded (world reset to frame 0).
fn base_engine() -> Engine {
    let mut engine = Engine::new();
    engine.load_inline_scene(BASE_SCENE).unwrap();
    engine
}

/// Starts a server on a free localhost port and returns it.
fn serve() -> ludion_server::Server {
    ludion_server::Server::bind(base_engine(), "127.0.0.1:0").expect("server binds")
}

/// A Transform component value (author shape: translation as a 3-array).
fn transform(x: f64, y: f64, z: f64) -> serde_json::Value {
    json!({ "translation": [x, y, z] })
}

/// A spawn command carrying one Transform component.
fn spawn_transform(x: f64, y: f64, z: f64) -> Command {
    let mut components = BTreeMap::new();
    components.insert("Transform".to_string(), transform(x, y, z));
    Command::EntitySpawn(EntitySpawnParams {
        name: None,
        components,
    })
}

/// **Handshake.** Connecting succeeds and the server's schema hash matches the client's build.
#[test]
fn connect_handshake_matches_schema() {
    let server = serve();
    let client = Client::connect(&server.url()).expect("handshake succeeds");
    assert_eq!(
        client.schema_hash(),
        ludion_protocol::schema::schema_hash(),
        "server announces this build's schema hash"
    );
    assert_eq!(client.server_hello().protocol_version, 1);
}

/// **Wire parity.** spawn → sim.step 10 → sim.hash over the wire equals the same sequence run on an
/// in-process engine — the byte-identical `WorldHash` exit-criterion for the command path.
#[test]
fn wire_command_sequence_matches_in_process() {
    // Over the wire.
    let server = serve();
    let mut client = Client::connect(&server.url()).unwrap();
    client.call(spawn_transform(1.0, 2.0, 3.0)).unwrap();
    client.sim_step(10, false).unwrap();
    let wire_hash = client.sim_hash().unwrap();

    // In-process: the same commands through dispatch on a fresh identical engine.
    let mut engine = base_engine();
    dispatch(&mut engine, spawn_transform(1.0, 2.0, 3.0)).unwrap();
    dispatch(
        &mut engine,
        Command::SimStep(ludion_protocol::command::SimStepParams {
            ticks: 10,
            hash: false,
        }),
    )
    .unwrap();
    let in_process_hash = engine.world_hash().unwrap().as_u64();

    assert_eq!(
        wire_hash.as_u64(),
        in_process_hash,
        "the world reached over the wire must hash identically to the in-process world"
    );
}

/// **One queue, many clients.** Two clients on one server both get correct, id-matched replies, and
/// their interleaved mutations land safely through the single queue.
#[test]
fn two_clients_share_one_queue() {
    let server = serve();
    let mut a = Client::connect(&server.url()).unwrap();
    let mut b = Client::connect(&server.url()).unwrap();

    // Interleave a spawn from each client.
    let spawned_a = a.call(spawn_transform(1.0, 0.0, 0.0)).unwrap();
    let spawned_b = b.call(spawn_transform(2.0, 0.0, 0.0)).unwrap();
    assert!(matches!(spawned_a.body, ResponseBody::Spawned { .. }));
    assert!(matches!(spawned_b.body, ResponseBody::Spawned { .. }));

    // Both mutations are visible: a query from either client sees two entities.
    let result = a
        .call(Command::QueryEntities(QueryEntitiesParams {
            filter: vec!["Transform".to_string()],
            include: vec![],
            world: false,
            entities: None,
        }))
        .unwrap();
    match result.body {
        ResponseBody::QueryResult { entities } => {
            assert_eq!(
                entities.len(),
                2,
                "both clients' spawns landed in the one world"
            )
        }
        other => panic!("expected a query result, got {other:?}"),
    }

    // Both hash the same world identically (proving they see one shared state).
    assert_eq!(a.sim_hash().unwrap(), b.sim_hash().unwrap());
}

/// **Paused engine keeps answering.** Pause over the wire, then a query still gets a response (the
/// paused engine keeps draining the queue; it just does not tick).
#[test]
fn paused_engine_still_answers_queries() {
    let server = serve();
    let mut client = Client::connect(&server.url()).unwrap();

    let ack = client.call(Command::SimPause).unwrap();
    assert!(matches!(ack.body, ResponseBody::Ack));

    // A query after pause must still return — the engine is paused, not deaf.
    let result = client
        .call(Command::QueryEntities(QueryEntitiesParams::default()))
        .unwrap();
    assert!(matches!(result.body, ResponseBody::QueryResult { .. }));

    // And the hash primitive still answers.
    client.sim_hash().unwrap();
}

/// **Every success carries the gameplay frame.** A game that holds frames 10..=39 through its
/// `GameClock` reports a `game_frame` that lags `frame` by the held frames, and the lag survives the
/// end of the hold. The oracle is arithmetic on the hold window, never the clock.
#[test]
fn success_replies_carry_the_gameplay_frame_of_a_held_session() {
    const HOLD_FROM: u64 = 10;
    const HOLD_TO: u64 = 39;
    let expected = |frame: u64| frame - (HOLD_FROM..=HOLD_TO).filter(|f| *f < frame).count() as u64;

    let mut engine = Engine::builder()
        .system("hold_window", |world, ctx| {
            if (HOLD_FROM..=HOLD_TO).contains(&ctx.frame.as_u64()) {
                world
                    .resource_mut::<ludion_core::time::GameClock>()
                    .expect("the engine installs a GameClock")
                    .hold();
            }
        })
        .build()
        .expect("the builder validates");
    engine.load_inline_scene(BASE_SCENE).unwrap();
    // Paused before serving, so only the client's steps advance the frame.
    dispatch(&mut engine, Command::SimPause).unwrap();
    let server = ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds");
    let mut client = Client::connect(&server.url()).unwrap();

    let mut at = 0;
    for boundary in [5, 30, 40, 41, 50] {
        let step = Command::SimStep(ludion_protocol::command::SimStepParams {
            ticks: boundary - at,
            hash: false,
        });
        at = boundary;
        let ok = client.call(step).unwrap();
        assert_eq!(ok.frame.map(|f| f.as_u64()), Some(boundary));
        assert_eq!(
            ok.game_frame.map(|f| f.as_u64()),
            Some(expected(boundary)),
            "game_frame at sim frame {boundary}"
        );
    }
    // A plain read at the same boundary: the editor stamps from this.
    let read = client.call(Command::SimHash).unwrap();
    assert_eq!(read.game_frame.map(|f| f.as_u64()), Some(expected(50)));
    assert_eq!(expected(50), 20);
}

/// **The GI rebake lever over the wire.** One verb sets, triggers and queries: empty params read the
/// session's current lever, a `mode` moves it and it *stays* moved across calls (host state, not a
/// per-request flag), and a `rebake` request is accepted at either setting. No GPU is involved — the
/// lever is pure session state, and a scene with no `GiVolume` is never stale.
#[test]
fn gi_rebake_lever_reads_and_persists_over_the_wire() {
    use ludion_protocol::command::{GiRebakeMode, RenderGiRebakeParams};

    let server = serve();
    let mut client = Client::connect(&server.url()).unwrap();
    let gi_rebake = |client: &mut Client, params: RenderGiRebakeParams| match client
        .call(Command::RenderGiRebake(params))
        .unwrap()
        .body
    {
        ResponseBody::GiRebake { mode, stale } => (mode, stale),
        other => panic!("{other:?}"),
    };

    // The query form: a fresh host is at the default, and shows nothing held.
    assert_eq!(
        gi_rebake(&mut client, RenderGiRebakeParams::default()),
        (GiRebakeMode::Auto, false)
    );

    assert_eq!(
        gi_rebake(
            &mut client,
            RenderGiRebakeParams {
                mode: Some(GiRebakeMode::Hold),
                rebake: false,
            }
        )
        .0,
        GiRebakeMode::Hold
    );
    // Read it back on a later request: the lever is the session's, not the request's.
    assert_eq!(
        gi_rebake(&mut client, RenderGiRebakeParams::default()).0,
        GiRebakeMode::Hold
    );

    // A rebake request answers with the lever unchanged (it suppresses the hold for one frame, it
    // does not release it), and going back to auto is the same verb again.
    assert_eq!(
        gi_rebake(
            &mut client,
            RenderGiRebakeParams {
                mode: None,
                rebake: true,
            }
        )
        .0,
        GiRebakeMode::Hold
    );
    assert_eq!(
        gi_rebake(
            &mut client,
            RenderGiRebakeParams {
                mode: Some(GiRebakeMode::Auto),
                rebake: true,
            }
        ),
        (GiRebakeMode::Auto, false)
    );
}

/// **Structured errors over the wire.** A command that fails at dispatch comes back as an error with
/// its diagnostic code intact (here: reading a component the entity does not carry → `L3006`).
#[test]
fn dispatch_error_flows_back_structured() {
    let server = serve();
    let mut client = Client::connect(&server.url()).unwrap();

    // component.get on a fresh entity that has only Transform, asking for Sprite → L3006.
    let spawned = client.call(spawn_transform(0.0, 0.0, 0.0)).unwrap();
    let entity = match spawned.body {
        ResponseBody::Spawned { entity } => entity,
        other => panic!("{other:?}"),
    };
    let err = client
        .call(Command::ComponentGet(
            ludion_protocol::command::ComponentGetParams {
                entity,
                component: "Sprite".to_string(),
            },
        ))
        .unwrap_err();
    assert_eq!(err.code.as_str(), "L3006");
}

/// **A request above tungstenite's default 16 MiB limit is read.** The engine answers with its own
/// refusal of the malformed snapshot; the connection is not dropped (`L3009`).
#[test]
fn a_request_over_the_default_message_limit_reaches_the_engine() {
    let server = serve();
    let mut client = Client::connect(&server.url()).unwrap();
    let err = client
        .call(Command::EngineRestore(
            ludion_protocol::command::EngineRestoreParams {
                snapshot: json!({ "padding": "x".repeat(17 << 20) }),
            },
        ))
        .unwrap_err();
    assert_ne!(err.code.as_str(), "L3009", "{err}");
}

// --- Raw-socket tests (bytes the typed client cannot produce) ------------------------------------

/// Opens a raw WebSocket, performs the hello handshake, and returns the socket ready for requests.
fn raw_connect(
    url: &str,
) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
    let (mut socket, _resp) = tungstenite::connect(url).unwrap();
    let hello = json!({ "protocol_version": 1, "client": "raw-test", "features": [] });
    socket.send(Message::text(hello.to_string())).unwrap();
    // Read (and discard) the ServerHello.
    let reply = read_text(&mut socket);
    assert!(
        reply.contains("schema_hash"),
        "expected a ServerHello, got {reply}"
    );
    socket
}

/// Reads the next text message from a raw socket.
fn read_text(
    socket: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> String {
    loop {
        match socket.read().unwrap() {
            Message::Text(t) => return t.as_str().to_string(),
            Message::Close(_) => return String::new(),
            _ => continue,
        }
    }
}

/// Sends `text` over a raw socket and returns the response's error code (or panics if it was `ok`).
fn error_code_for(url: &str, text: &str) -> String {
    let mut socket = raw_connect(url);
    socket.send(Message::text(text.to_string())).unwrap();
    let reply = read_text(&mut socket);
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    value["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error response, got {reply}"))
        .to_string()
}

/// **Malformed / unknown / wrong-version requests** come back as structured errors with the right
/// L-codes: L3001 for non-JSON, L3002 for an unknown method, L3003 for a bad version.
#[test]
fn malformed_requests_get_structured_errors() {
    let server = serve();
    let url = server.url();

    // Not JSON at all → L3001, and (null-id convention) id 0.
    assert_eq!(error_code_for(&url, "this is not json"), "L3001");

    // Unknown method → L3002, echoing the request's id.
    assert_eq!(
        error_code_for(
            &url,
            r#"{"v":1,"id":5,"method":"entity.teleport","params":{}}"#
        ),
        "L3002"
    );

    // Unsupported protocol version → L3003.
    assert_eq!(
        error_code_for(&url, r#"{"v":2,"id":6,"method":"sim.pause"}"#),
        "L3003"
    );
}

/// **The null-id convention.** An unparseable request (no recoverable id) is answered with id 0; a
/// request whose id is present but is otherwise invalid echoes that id.
#[test]
fn error_id_follows_the_null_id_convention() {
    let server = serve();
    let url = server.url();

    let reply_for = |text: &str| -> serde_json::Value {
        let mut socket = raw_connect(&url);
        socket.send(Message::text(text.to_string())).unwrap();
        serde_json::from_str(&read_text(&mut socket)).unwrap()
    };

    // No recoverable id → 0.
    assert_eq!(reply_for("nonsense")["id"], json!(0));
    // Id present, method unknown → echoed.
    assert_eq!(
        reply_for(r#"{"v":1,"id":99,"method":"nope"}"#)["id"],
        json!(99)
    );
}

/// **Wrong first message closes the handshake with an L-code.** A request sent before the hello is
/// rejected with L3001 and the connection is closed.
#[test]
fn wrong_first_message_is_rejected() {
    let server = serve();
    let (mut socket, _resp) = tungstenite::connect(server.url()).unwrap();
    // Send a request instead of a hello.
    socket
        .send(Message::text(
            r#"{"v":1,"id":1,"method":"sim.pause"}"#.to_string(),
        ))
        .unwrap();
    let reply = read_text(&mut socket);
    let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(value["error"]["code"], json!("L3001"));
}

/// Opens a raw WebSocket whose handshake carries the given `Origin` header.
fn connect_with_origin(
    url: &str,
    origin: &str,
) -> Result<
    tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    Box<tungstenite::Error>,
> {
    use tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", origin.parse().unwrap());
    tungstenite::connect(request)
        .map(|(socket, _response)| socket)
        .map_err(Box::new)
}

/// **A foreign browser origin is refused at the handshake.** The upgrade is answered HTTP 403 with the
/// L3039 text, so a web page from another site cannot drive the engine; native clients send no
/// `Origin` and every other test here connects without one.
#[test]
fn a_foreign_browser_origin_is_refused_with_403() {
    let server = serve();
    let url = server.url();
    for origin in ["https://example.com", "null", "http://localhost.evil.com"] {
        match connect_with_origin(&url, origin).map_err(|error| *error) {
            Err(tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), 403, "{origin}");
                let body = String::from_utf8_lossy(response.body().as_deref().unwrap_or_default())
                    .into_owned();
                assert!(body.contains("L3039"), "{origin}: {body}");
            }
            Err(other) => panic!("{origin}: expected an HTTP 403, got {other}"),
            Ok(_) => panic!("{origin}: the handshake was accepted"),
        }
    }
}

/// **The editor's own origins connect.** Tauri serves `tauri://localhost` (macOS) and
/// `http://tauri.localhost` (Windows); the dev server is `http://localhost:1420`.
#[test]
fn the_editors_origins_complete_the_handshake() {
    let server = serve();
    let url = server.url();
    for origin in [
        "tauri://localhost",
        "http://tauri.localhost",
        "http://localhost:1420",
    ] {
        let mut socket =
            connect_with_origin(&url, origin).unwrap_or_else(|error| panic!("{origin}: {error}"));
        let hello = json!({ "protocol_version": 1, "client": "origin-test", "features": [] });
        socket.send(Message::text(hello.to_string())).unwrap();
        let reply = read_text(&mut socket);
        assert!(reply.contains("schema_hash"), "{origin}: {reply}");
    }
}

/// A same-build client connects. The drift paths (a mismatched schema hash or protocol version,
/// `L3003`) live in `ludion-client/tests/handshake_mismatch.rs`.
#[test]
fn matching_schema_connects_cleanly() {
    let server = serve();
    Client::connect(&server.url()).unwrap();
}

/// A recording sink that fails every write and flush once `broken` is set (a full disk).
struct BreakableSink(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl std::io::Write for BreakableSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        Ok(())
    }
}

/// A recording that dies on a full disk surfaces from `shutdown` — the host then exits non-zero —
/// whether a live tick met the failure first (`L3042`) or the final flush did (`L1038`).
#[test]
fn a_recording_that_fails_is_reported_by_shutdown() {
    let broken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut engine = base_engine();
    engine
        .set_recording(BreakableSink(broken.clone()))
        .expect("the recording arms");
    let server = ludion_server::Server::bind(engine, "127.0.0.1:0").expect("server binds");
    broken.store(true, std::sync::atomic::Ordering::SeqCst);
    let fault = server
        .shutdown()
        .expect("a failed recording is reported, not dropped");
    assert!(["L1038", "L3042"].contains(&fault.code.as_str()), "{fault}");
}

#[test]
fn a_socket_that_never_upgrades_does_not_hold_shutdown() {
    let server = serve();
    // Connects, then says nothing: no upgrade request, no `Hello`.
    let _silent = std::net::TcpStream::connect(server.local_addr()).unwrap();
    // Let the accept loop take the connection before teardown starts.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let started = std::time::Instant::now();
    assert!(server.shutdown().is_none());
    // The drain grace is 5 s, so a connection teardown had to wait out would take at least that.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "shutdown took {:?}",
        started.elapsed()
    );
}
