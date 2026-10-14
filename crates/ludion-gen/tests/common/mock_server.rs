//! A mock ComfyUI server: the six endpoints plus `/ws`, served from the committed fixtures on a
//! loopback port. Shared by `ludion-gen`'s mock tests and `ludion-cli`'s `gen` tests through
//! `#[path]`, so it is std plus `serde_json` and `tungstenite` only.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const OBJECT_INFO: &[u8] = include_bytes!("../fixtures/object_info.json");
const CANDIDATE_PNG: &[u8] = include_bytes!("../fixtures/candidate.png");

/// How the mock behaves for one test.
#[derive(Clone, Copy)]
pub struct MockConfig {
    /// `false` answers `/ws` with a 404, so `follow` falls back to polling `/history`.
    pub websocket: bool,
    /// `true` accepts the WebSocket and then says nothing, and no job ever runs — no frames, no
    /// history. What a wedged or very slow server looks like from here.
    pub stall: bool,
    /// `false` routes no job frame to any socket — what a socket registered after the jobs
    /// finished sees. Only `/history` can observe them.
    pub frames: bool,
    /// How many `/history` polls a job takes to appear. `0` is "immediately".
    pub history_after: usize,
    /// Submit numbers (`job-N`) that fail exactly like a graph titled `Explode`.
    pub fail_jobs: &'static [u32],
    /// `true` accepts a job and then never queues, runs or records it — what a ComfyUI restart
    /// does to the jobs it held.
    pub forget: bool,
    /// How many files the image saver reports per job, as a `batch_size` above 1 does.
    pub batch: usize,
    /// `true` adds a `PreviewImage`'s `temp` file under node `8`, which a run must never download.
    pub preview: bool,
    /// Submit numbers answered with a 400 before the number is taken, so every later submit is
    /// refused too.
    pub refuse_jobs: &'static [u32],
    /// Submit numbers (`job-N`) that stay queued forever, as `stall` does for every job.
    pub stall_jobs: &'static [u32],
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig {
            websocket: true,
            stall: false,
            frames: true,
            history_after: 0,
            fail_jobs: &[],
            forget: false,
            batch: 1,
            preview: false,
            refuse_jobs: &[],
            stall_jobs: &[],
        }
    }
}

impl MockConfig {
    /// Whether the job submitted as `number` with `graph` fails on the server.
    fn fails(&self, graph: &Value, number: usize) -> bool {
        has_title(graph, "Explode") || self.fail_jobs.iter().any(|job| *job as usize == number)
    }

    /// Whether the job submitted as `number` never runs.
    fn stalls(&self, number: usize) -> bool {
        self.stall || self.stall_jobs.iter().any(|job| *job as usize == number)
    }
}

/// The submit number inside a prompt id (`job-3` is 3).
fn submit_number(prompt_id: &str) -> usize {
    prompt_id
        .strip_prefix("job-")
        .and_then(|number| number.parse().ok())
        .expect("the mock names its prompts job-<number>")
}

pub struct State {
    pub config: MockConfig,
    /// Every submitted `(prompt_id, graph)`, in submit order.
    pub prompts: Mutex<Vec<(String, Value)>>,
    /// The `client_id` each submit carried.
    pub client_ids: Mutex<Vec<String>>,
    /// The live WebSocket per `clientId`; the latest registration wins, as ComfyUI's `sockets[sid]`.
    pub sockets: Mutex<BTreeMap<String, mpsc::Sender<Value>>>,
    /// Prompt ids whose frames found no socket under their submit's `client_id`: ComfyUI drops
    /// them and never replays them on connect.
    pub dropped: Mutex<Vec<String>>,
    /// Per-job `/history` poll counts, for `history_after`.
    pub polls: Mutex<BTreeMap<String, usize>>,
    /// Every uploaded filename, in upload order.
    pub uploads: Mutex<Vec<String>>,
    /// The server's input directory: stored name to bytes.
    pub files: Mutex<BTreeMap<String, Vec<u8>>>,
    pub next_id: AtomicU32,
}

impl State {
    pub fn prompt_index(&self, prompt_id: &str) -> Option<usize> {
        self.prompts
            .lock()
            .unwrap()
            .iter()
            .position(|(id, _)| id == prompt_id)
    }
}

pub struct Mock {
    pub url: String,
    pub state: Arc<State>,
}

impl Mock {
    pub fn start(config: MockConfig) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").expect("the mock binds a loopback port");
        let addr: SocketAddr = listener.local_addr().expect("the mock has an address");
        let state = Arc::new(State {
            config,
            prompts: Mutex::new(Vec::new()),
            client_ids: Mutex::new(Vec::new()),
            sockets: Mutex::new(BTreeMap::new()),
            dropped: Mutex::new(Vec::new()),
            polls: Mutex::new(BTreeMap::new()),
            uploads: Mutex::new(Vec::new()),
            files: Mutex::new(BTreeMap::new()),
            next_id: AtomicU32::new(0),
        });
        let served = Arc::clone(&state);
        // Detached: the test binary exits when its main thread does, whatever this one is waiting on.
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&served);
                std::thread::spawn(move || serve(stream, state));
            }
        });
        Mock {
            url: format!("http://{addr}"),
            state,
        }
    }

    /// A ComfyUI restart: the queue and the history are in memory, so every job is forgotten.
    pub fn restart(&self) {
        self.state.prompts.lock().unwrap().clear();
        self.state.polls.lock().unwrap().clear();
    }
}

fn serve(stream: TcpStream, state: Arc<State>) {
    let Some(request_line) = peek_request_line(&stream) else {
        return;
    };
    if request_line.starts_with("GET /ws") {
        serve_websocket(stream, &state, &request_line);
        return;
    }
    let Some((method, target, body)) = read_request(&stream) else {
        return;
    };
    let (path, query) = split_query(&target);
    match (method.as_str(), path) {
        ("GET", "/system_stats") => respond_json(
            &stream,
            200,
            &json!({
                "system": {
                    "os": "posix",
                    "comfyui_version": "0.3.99",
                    "python_version": "3.12.7",
                    "ram_total": 68_719_476_736u64
                },
                "devices": [{ "name": "cuda:0", "type": "cuda", "vram_total": 25_769_803_776u64 }]
            }),
        ),
        ("GET", "/object_info") => {
            respond(&stream, 200, "application/json", OBJECT_INFO);
        }
        ("GET", "/queue") => serve_queue(&stream, &state),
        ("POST", "/upload/image") => serve_upload(&stream, &state, &body),
        ("POST", "/prompt") => serve_prompt(&stream, &state, &body),
        ("GET", path) if path.starts_with("/history/") => {
            serve_history(&stream, &state, &path["/history/".len()..]);
        }
        ("GET", "/view") => serve_view(&stream, &query),
        _ => respond(&stream, 404, "text/plain", b"not found"),
    }
}

/// Where `needle` first occurs in `haystack`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A multipart body's parts as `(header block, bytes)`, split byte-exactly: the boundary is the
/// first line, and each part ends at `\r\n--{boundary}`.
fn multipart_parts(body: &[u8]) -> Vec<(String, Vec<u8>)> {
    let line_end = find_bytes(body, b"\r\n").expect("a multipart body opens with its boundary");
    let delimiter = [b"\r\n".as_slice(), &body[..line_end]].concat();
    let mut parts = Vec::new();
    let mut position = line_end + 2;
    while position < body.len() {
        let rest = &body[position..];
        let header_end = find_bytes(rest, b"\r\n\r\n").expect("a part has headers");
        let headers = String::from_utf8_lossy(&rest[..header_end]).to_string();
        let data = &rest[header_end + 4..];
        let end = find_bytes(data, &delimiter).expect("a part ends at the boundary");
        parts.push((headers, data[..end].to_vec()));
        position += header_end + 4 + end + delimiter.len();
        if body[position..].starts_with(b"--") {
            break;
        }
        position += 2;
    }
    parts
}

/// ComfyUI's `/upload/image`: `overwrite` replaces the file; otherwise identical bytes reuse the
/// name and different bytes take the next free `stem (i).ext`. The reply carries the stored name.
fn serve_upload(stream: &TcpStream, state: &State, body: &[u8]) {
    let parts = multipart_parts(body);
    let part = |name: &str| {
        parts
            .iter()
            .find(|(headers, _)| headers.contains(&format!("name=\"{name}\"")))
    };
    let (headers, bytes) = part("image").expect("the upload body has an `image` part");
    let filename = headers
        .split("filename=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("upload.png");
    let overwrite = part("overwrite").is_some_and(|(_, value)| value == b"true");

    let mut files = state.files.lock().unwrap();
    let (stem, extension) = match filename.rsplit_once('.') {
        Some((stem, extension)) => (stem, format!(".{extension}")),
        None => (filename, String::new()),
    };
    let mut name = filename.to_string();
    let mut copy = 0;
    while !overwrite && files.get(&name).is_some_and(|stored| stored != bytes) {
        copy += 1;
        name = format!("{stem} ({copy}){extension}");
    }
    files.insert(name.clone(), bytes.clone());
    state.uploads.lock().unwrap().push(name.clone());
    respond_json(
        stream,
        200,
        &json!({ "name": name, "subfolder": "", "type": "input" }),
    );
}

fn serve_prompt(stream: &TcpStream, state: &State, body: &[u8]) {
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        respond_json(stream, 400, &json!({ "error": { "message": "not JSON" } }));
        return;
    };
    let graph = payload
        .get("prompt")
        .cloned()
        .expect("a submit carries its graph under `prompt`");
    if let Some(client_id) = payload.get("client_id").and_then(Value::as_str) {
        state.client_ids.lock().unwrap().push(client_id.to_string());
    }

    // The one class the mock always refuses, so the structured 400 has a reproducible trigger.
    if let Some((id, node)) = graph
        .as_object()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|(_, node)| node.get("class_type").and_then(Value::as_str) == Some("FailMe"))
        })
        .map(|(id, node)| (id.clone(), node.clone()))
    {
        respond_json(
            stream,
            400,
            &json!({
                "error": {
                    "type": "prompt_outputs_failed_validation",
                    "message": "Prompt outputs failed validation"
                },
                "node_errors": {
                    id: {
                        "class_type": node.get("class_type").cloned().unwrap_or(Value::Null),
                        "errors": [{
                            "type": "value_not_in_list",
                            "message": "Value not in list",
                            "details": "filename_prefix: 'ComfyUI' not in []",
                            "extra_info": { "input_name": "filename_prefix" }
                        }]
                    }
                }
            }),
        );
        return;
    }

    if state
        .config
        .refuse_jobs
        .contains(&state.next_id.load(Ordering::SeqCst))
    {
        respond_json(
            stream,
            400,
            &json!({
                "error": { "type": "server_busy", "message": "the server refuses this job" },
                "node_errors": {}
            }),
        );
        return;
    }

    let number = state.next_id.fetch_add(1, Ordering::SeqCst);
    let prompt_id = format!("job-{number}");
    let frames = job_frames(&prompt_id, number as usize, &graph, &state.config);
    state
        .prompts
        .lock()
        .unwrap()
        .push((prompt_id.clone(), graph));
    if !state.config.stalls(number as usize) && !state.config.forget {
        // The job finishes at submit (a cache hit); its frames go to the socket registered under
        // the submit's client id right now, or nowhere.
        let client_id = payload
            .get("client_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let sender = state.sockets.lock().unwrap().get(client_id).cloned();
        match sender {
            Some(sender) if state.config.frames => {
                for frame in frames {
                    let _ = sender.send(frame);
                }
            }
            _ => state.dropped.lock().unwrap().push(prompt_id.clone()),
        }
    }
    respond_json(
        stream,
        200,
        &json!({ "prompt_id": prompt_id, "number": number, "node_errors": {} }),
    );
}

/// Every job `/history` has not yet recorded, as ComfyUI lists them: the first running, the rest
/// pending, each `[number, prompt_id, prompt, extra_data, outputs]`.
fn serve_queue(stream: &TcpStream, state: &State) {
    let mut unfinished = Vec::new();
    if !state.config.forget {
        let polls = state.polls.lock().unwrap();
        for (id, _) in state.prompts.lock().unwrap().iter() {
            let number = submit_number(id);
            let seen = polls.get(id).copied().unwrap_or(0);
            if state.config.stalls(number) || seen < state.config.history_after {
                unfinished.push(json!([number, id, {}, {}, []]));
            }
        }
    }
    let pending = unfinished.split_off(unfinished.len().min(1));
    respond_json(
        stream,
        200,
        &json!({ "queue_running": unfinished, "queue_pending": pending }),
    );
}

fn serve_history(stream: &TcpStream, state: &State, prompt_id: &str) {
    let Some(index) = state.prompt_index(prompt_id) else {
        respond_json(stream, 200, &json!({}));
        return;
    };
    let number = submit_number(prompt_id);
    let seen = {
        let mut polls = state.polls.lock().unwrap();
        let entry = polls.entry(prompt_id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    };
    if state.config.stalls(number) || state.config.forget || seen <= state.config.history_after {
        respond_json(stream, 200, &json!({}));
        return;
    }
    let graph = state.prompts.lock().unwrap()[index].1.clone();
    if state.config.fails(&graph, number) {
        respond_json(
            stream,
            200,
            &json!({
                prompt_id: {
                    "outputs": {},
                    "status": {
                        "status_str": "error",
                        "completed": false,
                        "messages": [
                            ["execution_start", { "prompt_id": prompt_id, "timestamp": 1 }],
                            ["execution_error", {
                                "prompt_id": prompt_id,
                                "node_id": "7",
                                "node_type": "SaveImage",
                                "exception_type": "RuntimeError",
                                "exception_message": "CUDA out of memory",
                                "traceback": []
                            }]
                        ]
                    }
                }
            }),
        );
        return;
    }
    let outputs = if writes_mesh(&graph) {
        // A `SaveGLB` graph's entry: the file under `3d`, plus a mesh-info node's `text` — a
        // non-file entry the download must ignore rather than count as an output.
        json!({
            "12": { "3d": [{
                "filename": format!("ludion_{number:05}_.glb"),
                "subfolder": "3d",
                "type": "output"
            }] },
            "13": { "text": ["4962 faces, 9 components"] }
        })
    } else {
        let images: Vec<Value> = (0..state.config.batch)
            .map(|slot| {
                json!({
                    "filename": format!("ComfyUI_{number:05}_{slot}_.png"),
                    "subfolder": "",
                    "type": "output"
                })
            })
            .collect();
        let mut outputs = json!({ "7": { "images": images } });
        if state.config.preview {
            outputs["8"] = json!({ "images": [{
                "filename": format!("ComfyUI_temp_{number:05}_.png"),
                "subfolder": "",
                "type": "temp"
            }] });
        }
        outputs
    };
    respond_json(
        stream,
        200,
        &json!({
            prompt_id: {
                "outputs": outputs,
                "status": { "status_str": "success", "completed": true, "messages": [] }
            }
        }),
    );
}

fn serve_view(stream: &TcpStream, query: &BTreeMap<String, String>) {
    let filename = query
        .get("filename")
        .expect("a view request names its file");
    if filename == "truncated.png" {
        // A promise of 1000 bytes, ten delivered, and the connection closed: a dropped download.
        let mut out = stream;
        let _ = out.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 1000\r\n\
              Connection: close\r\n\r\n0123456789",
        );
        let _ = out.flush();
        return;
    }
    if filename.ends_with(".glb") {
        // The download path never parses a mesh, so a body is all the test needs; a real `.glb` is
        // the accept path's fixture, not this one's.
        respond(stream, 200, "model/gltf-binary", MESH_BODY);
        return;
    }
    respond(stream, 200, "image/png", CANDIDATE_PNG);
}

/// What `/view` serves for a `.glb`.
pub const MESH_BODY: &[u8] = b"not a mesh: the run pipeline downloads bytes and never reads them";

fn serve_websocket(stream: TcpStream, state: &State, request_line: &str) {
    if !state.config.websocket {
        respond(&stream, 404, "text/plain", b"no websocket here");
        return;
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/ws");
    let client_id = split_query(target)
        .1
        .get("clientId")
        .cloned()
        .unwrap_or_default();
    // Registered before the handshake completes: ComfyUI registers inside the handshake's own
    // event-loop tick, so no submit falls between the client's connect and the registration.
    let (sender, receiver) = mpsc::channel::<Value>();
    state
        .sockets
        .lock()
        .unwrap()
        .insert(client_id.clone(), sender);
    let Ok(mut socket) = tungstenite::accept(stream) else {
        return;
    };
    let greeting = json!({
        "type": "status",
        "data": { "status": { "exec_info": { "queue_remaining": 0 } }, "sid": client_id }
    });
    let _ = socket.send(tungstenite::Message::text(greeting.to_string()));
    // Frames arrive only from `serve_prompt`; a stalled mock sends none.
    while let Ok(frame) = receiver.recv_timeout(Duration::from_secs(30)) {
        let _ = socket.send(tungstenite::Message::text(frame.to_string()));
    }
}

/// The frames ComfyUI emits for one job, in order.
fn job_frames(prompt_id: &str, number: usize, graph: &Value, config: &MockConfig) -> Vec<Value> {
    let mut frames = vec![
        json!({
            "type": "executing",
            "data": { "node": "5", "display_node": "5", "prompt_id": prompt_id }
        }),
        json!({
            "type": "progress",
            "data": { "value": 10, "max": 20, "node": "5", "prompt_id": prompt_id }
        }),
    ];
    if config.fails(graph, number) {
        frames.push(json!({
            "type": "execution_error",
            "data": {
                "prompt_id": prompt_id,
                "node_id": "7",
                "node_type": "SaveImage",
                "exception_type": "RuntimeError",
                "exception_message": "CUDA out of memory",
                "traceback": []
            }
        }));
        return frames;
    }
    frames.push(json!({
        "type": "executed",
        "data": {
            "node": "7",
            "display_node": "7",
            "prompt_id": prompt_id,
            "output": { "images": [{
                "filename": format!("ComfyUI_{number:05}_.png"),
                "subfolder": "",
                "type": "output"
            }] }
        }
    }));
    frames.push(json!({ "type": "executing", "data": { "node": null, "prompt_id": prompt_id } }));
    frames.push(json!({
        "type": "execution_success",
        "data": { "prompt_id": prompt_id, "timestamp": 1 }
    }));
    frames
}

/// Whether the submitted graph saves a mesh, which is what decides the shape of its `/history`
/// entry — the server reports `3d` files under a different key from `images`.
fn writes_mesh(graph: &Value) -> bool {
    graph.as_object().is_some_and(|nodes| {
        nodes.values().any(|node| {
            node.get("class_type")
                .and_then(Value::as_str)
                .is_some_and(|class| class.contains("SaveGLB"))
        })
    })
}

fn has_title(graph: &Value, title: &str) -> bool {
    graph.as_object().is_some_and(|nodes| {
        nodes.values().any(|node| {
            node.get("_meta")
                .and_then(|meta| meta.get("title"))
                .and_then(Value::as_str)
                == Some(title)
        })
    })
}

// --- HTTP plumbing --------------------------------------------------------------------------

/// Looks at the request line without consuming it, so a `/ws` upgrade can be handed to
/// `tungstenite::accept` with its handshake intact.
fn peek_request_line(stream: &TcpStream) -> Option<String> {
    let mut buffer = [0u8; 1024];
    for _ in 0..200 {
        let read = stream.peek(&mut buffer).ok()?;
        if let Some(end) = buffer[..read].windows(2).position(|pair| pair == b"\r\n") {
            return Some(String::from_utf8_lossy(&buffer[..end]).to_string());
        }
        if read == buffer.len() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

fn read_request(stream: &TcpStream) -> Option<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some(value) = header
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(str::trim)
            .and_then(|value| value.parse::<usize>().ok())
        {
            length = value;
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some((method, target, body))
}

fn split_query(target: &str) -> (&str, BTreeMap<String, String>) {
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let mut parsed = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        parsed.insert(key.to_string(), percent_decode(value));
    }
    (path, parsed)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn respond_json(stream: &TcpStream, status: u16, body: &Value) {
    respond(
        stream,
        status,
        "application/json",
        body.to_string().as_bytes(),
    );
}

fn respond(stream: &TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let mut out = stream;
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = out.write_all(header.as_bytes());
    let _ = out.write_all(body);
    let _ = out.flush();
}
