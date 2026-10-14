//! The ComfyUI HTTP and WebSocket client: six endpoints and one progress stream, all synchronous.
//!
//! The server is a machine property, exactly as `LUDION_THREADS` is: it resolves from `--url` or
//! `LUDION_COMFY_URL`, never from a committed file, and no artifact this crate writes records the
//! address. Because the server may be on another machine, inputs go up through `POST /upload/image`
//! and outputs come back down through `GET /view` — no code path here assumes a shared filesystem.
//!
//! The failure taxonomy is the product: [`code::COMFY_UNREACHABLE`] when nothing answers,
//! [`code::COMFY_REJECTED`] when the server answers and refuses, and
//! [`code::GENERATION_RUN_TIMED_OUT`] when a follow outlives its budget. A job that fails on the
//! server is a verdict, not an error: [`Client::job_states`] returns it as [`JobError`] data, and
//! the run pipeline turns each one into a [`code::GENERATION_RUN_FAILED`].

use std::collections::BTreeSet;
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use serde_json::{Map, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::workflow::OutputKind;

/// The environment variable holding the ComfyUI address on this machine.
pub const COMFY_URL_ENV: &str = "LUDION_COMFY_URL";

/// Where ComfyUI listens when nothing says otherwise — its own default, on the loopback address.
pub const DEFAULT_COMFY_URL: &str = "http://127.0.0.1:8188";

/// How long a single HTTP request may take. Every endpoint here answers immediately (a `/prompt`
/// returns a job id, not a result), so a request that hangs this long is a wedged server.
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a `GET /view` download may take: sized for the 512 MiB cap on a slow link, where
/// [`HTTP_TIMEOUT`] would cut a large mesh off mid-body.
const VIEW_TIMEOUT: Duration = Duration::from_secs(600);

/// A downloaded output may be a multi-megabyte `.glb`, well past `ureq`'s 10 MB default body cap.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

/// The name an image goes up under: `ludion-<content hash>.<ext>`. The name is a function of the
/// bytes, so two sources with one file name stay two files on the server and a graph hash that
/// covers the name covers the bytes. The extension is the source's own when it is plain ASCII
/// alphanumeric, else `png`.
pub fn upload_name(path: &Path, bytes: &[u8]) -> String {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .filter(|extension| {
            !extension.is_empty() && extension.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or_else(|| "png".to_string());
    format!("ludion-{}.{extension}", crate::recipe::content_hash(bytes))
}

/// How often the fallback path asks `/history` when the WebSocket cannot be opened.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often the WebSocket follow also asks `/history` for the jobs still pending, so a job whose
/// frames never reached the socket is still observed.
const HISTORY_SWEEP_INTERVAL: Duration = Duration::from_secs(2);

/// Resolves the server address: `--url` first, then `LUDION_COMFY_URL`, then ComfyUI's own default.
///
/// # Errors
/// [`code::COMFY_UNREACHABLE`] when the resolved address is not a plain `http://` URL — the one
/// thing that can be wrong before anything is tried. An `https://` address is refused by name: this
/// client is built without TLS, and trying it would only report "nothing answered".
pub fn resolve_url(flag: Option<&str>) -> Result<String, Diagnostic> {
    let (raw, source) = match flag.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => (value.to_string(), "the `--url` flag"),
        None => match std::env::var(COMFY_URL_ENV) {
            Ok(value) if !value.trim().is_empty() => (
                value.trim().to_string(),
                "the `LUDION_COMFY_URL` environment variable",
            ),
            _ => (DEFAULT_COMFY_URL.to_string(), "ComfyUI's default address"),
        },
    };
    if raw.starts_with("https://") {
        return Err(Diagnostic::new(
            code::COMFY_UNREACHABLE,
            format!("{source} gives an https:// address, and this client is built without TLS"),
        )
        .with_help(
            "point at ComfyUI's own plain `http://` address, or at a tunnel that exposes plain HTTP",
        ));
    }
    if !raw.starts_with("http://") {
        return Err(Diagnostic::new(
            code::COMFY_UNREACHABLE,
            format!("{source} does not give an http:// address"),
        )
        .with_help(format!(
            "give the whole URL, e.g. `--url http://<host>:8188`, or set `{COMFY_URL_ENV}` to one"
        )));
    }
    Ok(raw.trim_end_matches('/').to_string())
}

/// What a run produced, per output file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputFile {
    /// The node that wrote it.
    pub node: String,
    /// The server-side filename.
    pub filename: String,
    /// The server-side subfolder, empty at the root of its output directory.
    pub subfolder: String,
    /// `output` or `input` — the directory `GET /view` reads it from. `temp` files (a
    /// `PreviewImage`'s) are skipped.
    pub folder_type: String,
    /// The file's position in its node's list for this job — the batch slot the server reported.
    pub batch_index: u32,
    /// Which of the two output families wrote it.
    pub kind: OutputKind,
}

/// One prompt's entry in `GET /history`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    /// The server's own `status_str` (`success`, `error`, …).
    pub status: String,
    /// Whether the server considers this prompt finished.
    pub completed: bool,
    /// The files it wrote, in node-id order.
    pub outputs: Vec<OutputFile>,
    /// Why the server recorded the job as failed; `None` is the only success.
    pub error: Option<JobError>,
}

/// A job the server recorded as failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobError {
    /// The node that raised, when the server names one.
    pub node: Option<String>,
    /// That node's class.
    pub class: Option<String>,
    /// The server's own message.
    pub message: String,
}

/// Where one submitted job stands, as [`Client::job_states`] reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobState {
    /// In `/queue`, running or waiting.
    Queued,
    /// Recorded in `/history`, whether it succeeded or not.
    Finished(HistoryEntry),
    /// In neither: ComfyUI keeps both in memory, so a restart forgot the job for good.
    Lost,
}

/// Which path [`Client::follow`] took. The WebSocket is the normal one; polling is what a server
/// behind a proxy that will not upgrade leaves. On the WebSocket path a job may still be observed
/// by the `/history` sweep rather than by a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowVia {
    /// Frames on `/ws`.
    WebSocket,
    /// Repeated `GET /history/{id}`.
    Polling,
}

/// The outcome of following a submitted run to completion.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Every prompt id with a verdict (finished, failed or lost), in the order it was observed.
    pub finished: Vec<String>,
    /// How the completion was observed.
    pub via: FollowVia,
}

/// What `GET /system_stats` says about the server — the two facts provenance records.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemStats {
    /// The server's own version string.
    pub comfy_version: String,
    /// The whole reply, for the CLI's `--json` and for anything provenance does not name.
    pub raw: Value,
}

/// A `/ws` socket registered under one client id, or none when the upgrade failed (the follow then
/// polls). Made by [`Client::subscribe`], consumed by [`Client::follow`].
pub struct Subscription {
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
}

/// A synchronous ComfyUI client bound to one server address.
pub struct Client {
    agent: ureq::Agent,
    base: String,
}

impl Client {
    /// Binds to `url` (a plain `http://` base, no trailing slash needed; see [`resolve_url`]).
    /// Nothing is contacted until a request is made.
    pub fn new(url: &str) -> Client {
        let config = ureq::Agent::config_builder()
            // The `/prompt` 400 carries the structured `node_errors` map this crate reprints, so a
            // status code is data here, not an error to be swallowed by the transport.
            .http_status_as_error(false)
            .build();
        Client {
            agent: config.into(),
            base: url.trim_end_matches('/').to_string(),
        }
    }

    /// The address this client speaks to.
    pub fn url(&self) -> &str {
        &self.base
    }

    /// `GET /system_stats` — the server's version, which a provenance block records.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn system_stats(&self) -> Result<SystemStats, Diagnostic> {
        let raw = self.get_json("/system_stats")?;
        let comfy_version = raw
            .get("system")
            .and_then(|system| system.get("comfyui_version"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        Ok(SystemStats { comfy_version, raw })
    }

    /// `GET /object_info` — the node schema every offline pass is checked against.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn object_info(&self) -> Result<Value, Diagnostic> {
        self.get_json("/object_info")
    }

    /// `GET /queue` — what is running and what is waiting.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn queue(&self) -> Result<Value, Diagnostic> {
        self.get_json("/queue")
    }

    /// `POST /upload/image` — uploads one image under `name` (see [`upload_name`]) and returns
    /// the server-side name a `LoadImage` input then holds (`subfolder/name` when the server filed
    /// it under one). No `overwrite`: ComfyUI keeps a name whose bytes are identical and renames a
    /// clash, and a content-addressed name never clashes with different bytes.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn upload_image(&self, name: &str, bytes: &[u8]) -> Result<String, Diagnostic> {
        let boundary = format!("----LudionGen{}", crate::recipe::content_hash(bytes));
        let body = multipart_body(&boundary, name, bytes);
        let (status, reply) = self.send(
            "/upload/image",
            Some((format!("multipart/form-data; boundary={boundary}"), body)),
            HTTP_TIMEOUT,
        )?;
        let value = self.decode_json("/upload/image", status, &reply)?;

        let stored = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Diagnostic::new(
                    code::COMFY_REJECTED,
                    "the server's `/upload/image` reply carries no `name`",
                )
                .with_help("check that the address points at a ComfyUI server")
            })?
            .to_string();
        let subfolder = value
            .get("subfolder")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(if subfolder.is_empty() {
            stored
        } else {
            format!("{subfolder}/{stored}")
        })
    }

    /// `POST /prompt` — submits one resolved graph and returns its prompt id.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`], or [`code::COMFY_REJECTED`] with the 400's `node_errors`
    /// reprinted one line per node.
    pub fn submit(&self, graph: &Value, client_id: &str) -> Result<String, Diagnostic> {
        let mut payload = Map::new();
        payload.insert("prompt".to_string(), graph.clone());
        payload.insert("client_id".to_string(), Value::from(client_id));
        let body = Value::Object(payload).to_string();

        let (status, reply) = self.send(
            "/prompt",
            Some(("application/json".to_string(), body.into_bytes())),
            HTTP_TIMEOUT,
        )?;
        if status != 200 {
            return Err(prompt_rejected(status, &reply));
        }
        let value = self.decode_json("/prompt", status, &reply)?;
        value
            .get("prompt_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                Diagnostic::new(
                    code::COMFY_REJECTED,
                    "the server accepted the graph but its `/prompt` reply carries no `prompt_id`",
                )
                .with_help("check that the address points at a ComfyUI server")
            })
    }

    /// `GET /history/{prompt_id}` — `None` while the server has no entry for it yet. An entry that
    /// records a failure is data ([`HistoryEntry::error`]), not an error.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn history(&self, prompt_id: &str) -> Result<Option<HistoryEntry>, Diagnostic> {
        let value = self.get_json(&format!("/history/{}", percent_encode(prompt_id)))?;
        let Some(entry) = value.get(prompt_id) else {
            return Ok(None);
        };
        let status = entry
            .get("status")
            .and_then(|status| status.get("status_str"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let completed = entry
            .get("status")
            .and_then(|status| status.get("completed"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Some(HistoryEntry {
            error: history_error(entry, completed),
            status,
            completed,
            outputs: collect_outputs(entry.get("outputs")),
        }))
    }

    /// Where each of `ids` stands: one `GET /queue`, then `GET /history/{id}` per id. The queue is
    /// read first, so a job absent from it with no history entry was gone before the read — a job
    /// finishing between the two reads shows up in history instead. Returns the raw queue too.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn job_states(&self, ids: &[String]) -> Result<(Vec<JobState>, Value), Diagnostic> {
        let queue = self.queue()?;
        let queued: BTreeSet<&str> = ["queue_running", "queue_pending"]
            .iter()
            .filter_map(|key| queue.get(key).and_then(Value::as_array))
            .flatten()
            .filter_map(|item| item.get(1).and_then(Value::as_str))
            .collect();
        let mut states = Vec::with_capacity(ids.len());
        for id in ids {
            states.push(match self.history(id)? {
                Some(entry) => JobState::Finished(entry),
                None if queued.contains(id.as_str()) => JobState::Queued,
                None => JobState::Lost,
            });
        }
        Ok((states, queue))
    }

    /// `GET /view` — downloads one output file's bytes.
    ///
    /// # Errors
    /// [`code::COMFY_UNREACHABLE`] or [`code::COMFY_REJECTED`].
    pub fn view(
        &self,
        filename: &str,
        subfolder: &str,
        folder_type: &str,
    ) -> Result<Vec<u8>, Diagnostic> {
        let path = format!(
            "/view?filename={}&subfolder={}&type={}",
            percent_encode(filename),
            percent_encode(subfolder),
            percent_encode(folder_type)
        );
        let (status, bytes) = self.send(&path, None, VIEW_TIMEOUT)?;
        if status != 200 {
            return Err(http_rejected("/view", status, &bytes));
        }
        Ok(bytes)
    }

    /// Opens `/ws?clientId=` for [`Client::follow`]. ComfyUI sends a client's frames only to the
    /// socket registered under its `clientId` at the moment they are emitted and never replays
    /// them, and a cached job finishes in milliseconds — so subscribe before the first
    /// [`Client::submit`] that uses `client_id`.
    ///
    /// A failed upgrade (a proxy that will not upgrade, a server built without it) is not an
    /// error: the subscription is empty and the follow polls `/history`, which reaches the same
    /// answer.
    pub fn subscribe(&self, client_id: &str) -> Subscription {
        let ws_base = self
            .base
            .strip_prefix("http://")
            .map(|rest| format!("ws://{rest}"));
        let socket = ws_base.and_then(|ws_base| {
            let url = format!("{ws_base}/ws?clientId={}", percent_encode(client_id));
            tungstenite::connect(url).ok().map(|(socket, _)| socket)
        });
        Subscription { socket }
    }

    /// Follows every submitted prompt until each has a verdict: finished, failed or lost.
    ///
    /// The subscription's `/ws` stream is the normal path, but a frame is only a hint to look:
    /// pending jobs leave only through [`Client::job_states`], swept on entry, every 2 s and
    /// whenever a job's closing frame arrives. An empty subscription falls back to sweeping every
    /// 500 ms under the same budget; [`Progress::via`] says which happened.
    ///
    /// # Errors
    /// [`code::GENERATION_RUN_TIMED_OUT`] when `timeout` elapses first, naming the job ids and the
    /// `gen status` / `gen fetch` recovery.
    pub fn follow(
        &self,
        subscription: Subscription,
        prompt_ids: &[String],
        timeout: Duration,
    ) -> Result<Progress, Diagnostic> {
        let deadline = Instant::now() + timeout.min(crate::run::MAX_TIMEOUT);
        match subscription.socket {
            Some(socket) => self.follow_stream(socket, prompt_ids, deadline),
            None => self.follow_polling(prompt_ids, deadline),
        }
    }

    fn follow_stream(
        &self,
        mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
        prompt_ids: &[String],
        deadline: Instant,
    ) -> Result<Progress, Diagnostic> {
        let mut pending: BTreeSet<&str> = prompt_ids.iter().map(String::as_str).collect();
        let mut finished = Vec::new();
        let mut next_sweep = Instant::now();
        loop {
            if Instant::now() >= next_sweep {
                self.sweep_history(prompt_ids, &mut pending, &mut finished)?;
                next_sweep = Instant::now() + HISTORY_SWEEP_INTERVAL;
            }
            if pending.is_empty() {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(timed_out(&pending));
            }
            // A zero read timeout is an error, not "no wait"; loop to the sweep or the deadline.
            let wait = next_sweep.min(deadline).saturating_duration_since(now);
            if wait.is_zero() {
                continue;
            }
            if set_read_timeout(&socket, wait).is_err() {
                return self.follow_polling(prompt_ids, deadline);
            }
            let message = match socket.read() {
                Ok(Message::Text(text)) => text.to_string(),
                Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_)) => continue,
                Ok(Message::Close(_)) | Ok(Message::Frame(_)) => {
                    // The server hung up mid-run; `/history` still holds the verdict.
                    return self.follow_polling(prompt_ids, deadline);
                }
                Err(tungstenite::Error::Io(e)) if is_timeout(&e) => continue,
                Err(_) => return self.follow_polling(prompt_ids, deadline),
            };
            let Ok(frame) = serde_json::from_str::<Value>(&message) else {
                continue;
            };
            let kind = frame
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let data = frame.get("data").unwrap_or(&Value::Null);
            let prompt_id = data.get("prompt_id").and_then(Value::as_str);
            // A frame is not a verdict: it only brings the next sweep forward. `executing` with a
            // null node is ComfyUI saying "this prompt is done"; an `executed` frame only means one
            // output node finished, which a multi-saver graph emits more than once.
            let closes = match kind {
                "execution_success" | "execution_error" | "execution_interrupted" => true,
                "executing" => data.get("node").is_some_and(Value::is_null),
                _ => false,
            };
            if closes && prompt_id.is_some_and(|id| pending.contains(id)) {
                next_sweep = Instant::now();
            }
        }
        Ok(Progress {
            finished,
            via: FollowVia::WebSocket,
        })
    }

    fn follow_polling(
        &self,
        prompt_ids: &[String],
        deadline: Instant,
    ) -> Result<Progress, Diagnostic> {
        let mut pending: BTreeSet<&str> = prompt_ids.iter().map(String::as_str).collect();
        let mut finished = Vec::new();
        loop {
            self.sweep_history(prompt_ids, &mut pending, &mut finished)?;
            if pending.is_empty() {
                return Ok(Progress {
                    finished,
                    via: FollowVia::Polling,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(timed_out(&pending));
            }
            std::thread::sleep(POLL_INTERVAL.min(remaining));
        }
    }

    /// One pass over the pending jobs in submit order: a job with a verdict (finished, failed or
    /// lost) leaves `pending`.
    fn sweep_history<'a>(
        &self,
        prompt_ids: &'a [String],
        pending: &mut BTreeSet<&'a str>,
        finished: &mut Vec<String>,
    ) -> Result<(), Diagnostic> {
        let ids: Vec<String> = prompt_ids
            .iter()
            .filter(|id| pending.contains(id.as_str()))
            .cloned()
            .collect();
        let (states, _) = self.job_states(&ids)?;
        for (id, state) in ids.into_iter().zip(states) {
            if state != JobState::Queued {
                pending.remove(id.as_str());
                finished.push(id);
            }
        }
        Ok(())
    }

    fn get_json(&self, path: &str) -> Result<Value, Diagnostic> {
        let (status, bytes) = self.send(path, None, HTTP_TIMEOUT)?;
        self.decode_json(path, status, &bytes)
    }

    fn decode_json(&self, path: &str, status: u16, bytes: &[u8]) -> Result<Value, Diagnostic> {
        if status != 200 {
            return Err(http_rejected(path, status, bytes));
        }
        serde_json::from_slice(bytes).map_err(|e| {
            Diagnostic::new(
                code::COMFY_REJECTED,
                format!("the server's `{path}` reply is not valid JSON: {e}"),
            )
            .with_help("check that the address points at a ComfyUI server and not another service")
        })
    }

    /// One request/response, returning the status and the whole body: a `GET` when `body` is
    /// `None`, a `POST` otherwise — the only two verbs ComfyUI's API needs — under `timeout` for
    /// the whole exchange. A failure to get an answer becomes [`code::COMFY_UNREACHABLE`]; one to
    /// read the body of an answer, [`code::COMFY_REJECTED`]; a status is handed back for the
    /// caller to judge.
    fn send(
        &self,
        path: &str,
        body: Option<(String, Vec<u8>)>,
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>), Diagnostic> {
        let url = format!("{}{path}", self.base);
        let result = match body {
            None => self
                .agent
                .get(&url)
                .config()
                .timeout_global(Some(timeout))
                .build()
                .call(),
            Some((content_type, bytes)) => self
                .agent
                .post(&url)
                .config()
                .timeout_global(Some(timeout))
                .build()
                .header("Content-Type", content_type)
                .send(&bytes[..]),
        };
        let mut response = result.map_err(|e| self.unreachable(&e))?;
        let status = response.status().as_u16();
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()
            .map_err(|e| unreadable(path, &e))?;
        Ok((status, bytes))
    }

    /// Nothing answered. The address is printed only when it did not come from the environment: a
    /// private hostname belongs in no transcript that might be pasted anywhere.
    fn unreachable(&self, error: &ureq::Error) -> Diagnostic {
        let from_env = std::env::var(COMFY_URL_ENV)
            .is_ok_and(|value| value.trim().trim_end_matches('/') == self.base);
        let where_it_tried = if from_env {
            format!("the address in `{COMFY_URL_ENV}`")
        } else {
            format!("`{}`", self.base)
        };
        Diagnostic::new(
            code::COMFY_UNREACHABLE,
            format!("nothing answered at {where_it_tried}: {error}"),
        )
        .with_help(format!(
            "start ComfyUI on that machine, or point `--url` / `{COMFY_URL_ENV}` at one that is \
             running; a remote server needs `--listen` on its side"
        ))
    }
}

/// The server answered but its body could not be read to the end. Never prints the address: the
/// server is known to be there.
fn unreadable(path: &str, error: &ureq::Error) -> Diagnostic {
    let help = match error {
        ureq::Error::BodyExceedsLimit(_) => format!(
            "the reply is larger than the {} MiB this client downloads",
            MAX_DOWNLOAD / (1024 * 1024)
        ),
        ureq::Error::Timeout(_) => {
            "the download outlived its time limit; a slow link needs a smaller output".to_string()
        }
        _ => "the connection dropped mid-reply; run the command again, or check the server's log"
            .to_string(),
    };
    Diagnostic::new(
        code::COMFY_REJECTED,
        format!("the server answered `{path}`, and its reply could not be read: {error}"),
    )
    .with_help(help)
}

/// The multipart body `POST /upload/image` takes: the file plus the form field that files it under
/// the server's `input` directory. Hand-written because one fixed two-part body does not
/// justify a multipart crate.
fn multipart_body(boundary: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 512);
    let mut part = |name: &str, extra: &str, value: &[u8]| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"{extra}\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    };
    part(
        "image",
        &format!("; filename=\"{filename}\"\r\nContent-Type: application/octet-stream"),
        bytes,
    );
    part("type", "", b"input");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// `outputs[node].images` and `outputs[node]["3d"]`, in node-id order.
fn collect_outputs(outputs: Option<&Value>) -> Vec<OutputFile> {
    let Some(outputs) = outputs.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut nodes: Vec<&String> = outputs.keys().collect();
    nodes.sort_by_key(|id| (id.parse::<u64>().unwrap_or(u64::MAX), (*id).clone()));

    let mut files = Vec::new();
    for node in nodes {
        for (key, kind) in [("images", OutputKind::Image), ("3d", OutputKind::Mesh)] {
            let Some(entries) = outputs[node].get(key).and_then(Value::as_array) else {
                continue;
            };
            for (position, entry) in entries.iter().enumerate() {
                let Some(filename) = entry.get("filename").and_then(Value::as_str) else {
                    continue;
                };
                if entry.get("type").and_then(Value::as_str) == Some("temp") {
                    continue;
                }
                files.push(OutputFile {
                    node: node.clone(),
                    filename: filename.to_string(),
                    subfolder: entry
                        .get("subfolder")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    folder_type: entry
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("output")
                        .to_string(),
                    batch_index: u32::try_from(position).unwrap_or(u32::MAX),
                    kind,
                });
            }
        }
    }
    files
}

/// The failure a history entry records, if any: an `execution_error` or `execution_interrupted`
/// message, an `error` status, or a job the server does not call completed.
fn history_error(entry: &Value, completed: bool) -> Option<JobError> {
    let messages = entry
        .get("status")
        .and_then(|status| status.get("messages"))
        .and_then(Value::as_array);
    for message in messages.into_iter().flatten() {
        let kind = message.get(0).and_then(Value::as_str).unwrap_or_default();
        let data = message.get(1).unwrap_or(&Value::Null);
        match kind {
            "execution_error" => return Some(job_error(data, None)),
            "execution_interrupted" => return Some(job_error(data, Some("interrupted"))),
            _ => {}
        }
    }
    let status = entry
        .get("status")
        .and_then(|status| status.get("status_str"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status == "error" {
        return Some(JobError {
            node: None,
            class: None,
            message: "the server recorded the job as failed, with no node named".to_string(),
        });
    }
    (!completed).then(|| JobError {
        node: None,
        class: None,
        message: "the server recorded the job as not completed".to_string(),
    })
}

/// An `execution_error` or `execution_interrupted` message's node, class and the server's own text
/// (or `fixed` when the failure carries none).
fn job_error(data: &Value, fixed: Option<&str>) -> JobError {
    let field = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_string);
    JobError {
        node: field("node_id"),
        class: field("node_type"),
        message: fixed
            .map(str::to_string)
            .or_else(|| field("exception_message"))
            .unwrap_or_else(|| "no message".to_string()),
    }
}

fn timed_out(pending: &BTreeSet<&str>) -> Diagnostic {
    let ids: Vec<&str> = pending.iter().copied().collect();
    Diagnostic::new(
        code::GENERATION_RUN_TIMED_OUT,
        format!(
            "the run outlived its `--timeout` with {} still unfinished: {}",
            if ids.len() == 1 { "one job" } else { "jobs" },
            ids.join(", ")
        ),
    )
    .with_help(
        "the jobs are still queued on the server: `ludion gen status <run-dir>` says where they are \
         and `ludion gen fetch <run-dir>` completes the run once they finish; `--timeout <s>` \
         raises the budget",
    )
}

/// A `/prompt` 400, whose `node_errors` map is reprinted node by node rather than dumped as raw
/// JSON — the one structured refusal ComfyUI gives, and the most useful thing this crate prints.
fn prompt_rejected(status: u16, body: &[u8]) -> Diagnostic {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let mut lines = Vec::new();
    if let Some(error) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        lines.push(error.to_string());
    }
    if let Some(nodes) = value.get("node_errors").and_then(Value::as_object) {
        let mut ids: Vec<&String> = nodes.keys().collect();
        ids.sort_by_key(|id| (id.parse::<u64>().unwrap_or(u64::MAX), (*id).clone()));
        for id in ids {
            let node = &nodes[id];
            let class = node
                .get("class_type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let errors = node.get("errors").and_then(Value::as_array);
            match errors {
                Some(errors) if !errors.is_empty() => {
                    for error in errors {
                        let input = error
                            .get("extra_info")
                            .and_then(|info| info.get("input_name"))
                            .and_then(Value::as_str)
                            .unwrap_or("-");
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("no message");
                        let details = error
                            .get("details")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        lines.push(if details.is_empty() {
                            format!("node {id} ({class}), input `{input}`: {message}")
                        } else {
                            format!("node {id} ({class}), input `{input}`: {message} ({details})")
                        });
                    }
                }
                _ => lines.push(format!("node {id} ({class}): rejected")),
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::from_utf8_lossy(body).trim().to_string());
    }
    Diagnostic::new(
        code::COMFY_REJECTED,
        format!(
            "the server refused the graph ({status}):\n  {}",
            lines.join("\n  ")
        ),
    )
    .with_help(
        "most of these are a value the graph or an override supplies that the node will not take; \
         `ludion gen check <recipe>` catches them before a submit when `/object_info` is reachable",
    )
}

fn http_rejected(path: &str, status: u16, body: &[u8]) -> Diagnostic {
    let body = String::from_utf8_lossy(body);
    let excerpt: String = body.trim().chars().take(400).collect();
    Diagnostic::new(
        code::COMFY_REJECTED,
        format!("the server answered `{path}` with HTTP {status}: {excerpt}"),
    )
    .with_help("the request reached the server intact, so this is its verdict on the content")
}

/// Plain `ws://` only, like the wire client: the workspace pins `tungstenite` to default features,
/// so a socket is always [`MaybeTlsStream::Plain`] and the other arm is unreachable in practice.
fn set_read_timeout(
    socket: &WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<(), ()> {
    let MaybeTlsStream::Plain(stream) = socket.get_ref() else {
        return Err(());
    };
    stream.set_read_timeout(Some(timeout)).map_err(|_| ())
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Percent-encodes one query value: ComfyUI filenames carry spaces, brackets and `#`.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
