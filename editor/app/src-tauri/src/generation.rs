//! The Generation view's one host capability: **run `ludion gen` as a subprocess**.
//!
//! The parity ruling this rests on (`Plan/comfy-generation.md` decision 15): the pipeline verbs are
//! CLI commands, and that satisfies Pillar 2. So the host never speaks HTTP to ComfyUI and never
//! reimplements a verb — it spawns `ludion gen <verb> … --json`, parses the one JSON object the verb
//! prints on stdout, and hands the caller stderr verbatim. Every capability the view has is
//! available to anyone with a shell, which is the rule's whole purpose.
//!
//! THREE PROPERTIES WORTH STATING:
//!
//!   1. **The verb is an allowlist, never a passthrough.** [`GEN_VERBS`] is the whole surface; a
//!      request for anything else is refused before a process exists. `--json` is appended here, so
//!      the frontend cannot ask for the text form and then guess at it.
//!   2. **The address never reaches a file.** `--url` arrives per invocation, from something the
//!      human typed for this session only; with none, the child inherits `LUDION_COMFY_URL` from the
//!      host's own environment. Nothing here writes it anywhere (`ludion docs generation`).
//!   3. **A `gen run` takes minutes.** Every command here is `#[tauri::command(async)]`, the posture
//!      [`crate::stage::run_check_scene`] already takes for its own multi-minute `cargo run`. The
//!      child is owned by [`crate::children::HostChildren`]: its process tree is killed at app exit
//!      and at a deadline ([`gen_deadline`], the build allowance plus the verb's `--timeout`).

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

use crate::children::{BUILD_ALLOWANCE, HostChildren, RunOutput};
use crate::stage::{AppStages, cli_argv};

/// The verbs that never contact a server: they refuse `--url` as an unknown argument, so the host
/// does not append it.
const OFFLINE_VERBS: [&str; 2] = ["accept", "sheet"];

/// Every `gen` verb the host will run. Sorted, because this is a membership test and a reader
/// checking whether a verb is here should not have to scan.
pub const GEN_VERBS: [&str; 7] = [
    "accept", "check", "fetch", "nodes", "run", "sheet", "status",
];

/// The largest candidate the host will hand the webview as a data url. A 1024-square PNG is ~1–2 MB
/// and a run holds up to a few dozen; the cap only exists so a stray file cannot wedge the webview
/// with a hundred-megabyte string.
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// What one `gen` invocation produced, as the frontend sees it.
#[derive(Debug, Clone, Serialize)]
pub struct GenResult {
    /// The child exited 0. A verb's own failure is `ok: false` — never an `Err`, which is reserved
    /// for "the process could not be run at all". Its diagnostics are inside `json`.
    pub ok: bool,
    /// The one JSON object the verb printed. A failing verb still prints it (`ok: false`,
    /// diagnostics inside); `null` only when nothing parseable was printed — clap's exit-2 argv
    /// refusal, a crash.
    pub json: Option<Value>,
    /// The child's stderr verbatim: the `L####` diagnostic blocks, cargo's build output above them.
    pub stderr: String,
    /// The exit code; `None` if the child was killed by a signal.
    #[serde(rename = "exitCode")]
    pub exit_code: Option<i32>,
}

/// The argv after `--`: `gen <verb> <args…> [--url <u>] --json`.
///
/// # Errors
/// `verb` outside [`GEN_VERBS`], or an `args` entry that would fight this function for control of
/// the two flags it owns (`--json`, `--url`) — the address has exactly one route in, and the output
/// format is not the caller's to choose.
pub fn gen_args(verb: &str, args: &[String], url: Option<&str>) -> Result<Vec<String>, String> {
    if !GEN_VERBS.contains(&verb) {
        return Err(format!(
            "`{verb}` is not a `ludion gen` verb the editor runs. Allowed: {}.",
            GEN_VERBS.join(", ")
        ));
    }
    for arg in args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if flag == "--json" || flag == "--url" {
            return Err(format!(
                "`{flag}` is set by the host, not by the caller: pass the server address as the \
                 command's `url` parameter, and every verb already runs with `--json`."
            ));
        }
    }
    let mut argv = vec!["gen".to_string(), verb.to_string()];
    argv.extend(args.iter().cloned());
    let url = url.map(str::trim).filter(|url| !url.is_empty());
    if let Some(url) = url.filter(|_| !OFFLINE_VERBS.contains(&verb)) {
        argv.push("--url".to_string());
        argv.push(url.to_string());
    }
    argv.push("--json".to_string());
    Ok(argv)
}

/// The one JSON object a `--json` verb printed, or `None`.
///
/// `cargo run -q` keeps its own chatter on stderr, so stdout is normally the object alone — but a
/// stray line before it must not lose the object, so the scan starts at the first line that opens
/// one. Deliberately total: a verb that died before printing yields `None`, not an error, because
/// its stderr is the thing worth showing.
pub fn parse_one_object(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim_start();
    if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }
    let start = stdout
        .char_indices()
        .find(|(_, c)| *c == '{')
        .map(|(index, _)| index)?;
    match serde_json::from_str::<Value>(&stdout[start..]) {
        Ok(value @ Value::Object(_)) => Some(value),
        _ => None,
    }
}

/// `cargo run -p ludion-cli` with the editor's standard posture, run **in the game project**.
///
/// Mirrors [`crate::stage`]'s own spawn posture (no inherited stdin, piped output, no flashing
/// console window on Windows) and stays separate from it for the one thing that differs: a `gen`
/// verb resolves its recipe, its run folders and `gen status`'s bare project walk against the
/// *current directory*, so this runs in `project_dir` while `--manifest-path` keeps the workspace
/// pinned. The environment is inherited untouched — that is how `LUDION_COMFY_URL` arrives.
fn gen_command(repo_root: &Path, project_dir: &Path, argv: &[String]) -> Command {
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut command = Command::new("cargo");
    command
        .args(cli_argv(repo_root, &refs))
        .current_dir(project_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// The data-url media type for `path`, or `None` for anything this is not willing to inline.
fn image_media_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The frontend's cancel tickets: the caller names a run before it starts, and `gen_cancel` finds
/// that run's child in the registry by it.
#[derive(Default)]
pub struct GenTickets(Mutex<HashMap<String, u64>>);

impl GenTickets {
    fn begin(&self, ticket: &str, id: u64) {
        if let Ok(mut map) = self.0.lock() {
            map.insert(ticket.to_string(), id);
        }
    }

    fn end(&self, ticket: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(ticket);
        }
    }

    /// Ask the child run under `ticket` to stop; `false` if none is running under it.
    fn cancel(&self, children: &HostChildren, ticket: &str) -> bool {
        let id = self.0.lock().ok().and_then(|map| map.get(ticket).copied());
        id.is_some_and(|id| children.kill(id))
    }
}

/// Releases a ticket on every exit path, a panic included.
struct TicketGuard<'a> {
    tickets: &'a GenTickets,
    ticket: &'a str,
}

impl Drop for TicketGuard<'_> {
    fn drop(&mut self) {
        self.tickets.end(self.ticket);
    }
}

/// The note appended to stderr when the host, not the verb, ended the run.
fn stop_note(verb: &str, output: &RunOutput, deadline: Duration) -> Option<String> {
    if output.timed_out {
        Some(format!(
            "\nthe editor stopped `ludion gen {verb}` after {} s (its deadline: the build \
             allowance plus the verb's --timeout); rerun with a larger --timeout if it needed \
             longer.\n",
            deadline.as_secs()
        ))
    } else if output.cancelled {
        Some(format!(
            "\nthe editor cancelled `ludion gen {verb}`; jobs it already queued on the ComfyUI \
             server are not cancelled there.\n"
        ))
    } else {
        None
    }
}

// ---- Tauri glue ------------------------------------------------------------

/// Run one `ludion gen` verb in `projectDir` and return its JSON object, stderr and exit code.
///
/// `Err` means the process could not be run at all (no cargo, an unusable project directory, a
/// refused verb). A verb that ran and failed is `Ok` with `ok: false`.
#[tauri::command(async)]
pub fn gen_invoke(
    state: tauri::State<'_, AppStages>,
    tickets: tauri::State<'_, GenTickets>,
    verb: String,
    args: Vec<String>,
    project_dir: String,
    url: Option<String>,
    ticket: Option<String>,
) -> Result<GenResult, String> {
    let argv = gen_args(&verb, &args, url.as_deref())?;
    let project = Path::new(&project_dir);
    if !project.is_dir() {
        return Err(format!(
            "`{project_dir}` is not a directory, so there is no project to run `ludion gen` in. \
             Switch project from the title bar."
        ));
    }
    let deadline = gen_deadline(&argv);
    let _release = ticket.as_deref().map(|ticket| TicketGuard {
        tickets: &tickets,
        ticket,
    });
    let output = state
        .children
        .run_with(
            gen_command(&state.repo_root, project, &argv),
            deadline,
            |id| {
                if let Some(ticket) = ticket.as_deref() {
                    tickets.begin(ticket, id);
                }
            },
        )
        .map_err(|e| {
            format!(
                "could not run `ludion gen {verb}`: {e}. The editor runs it as \
                 `cargo run -p ludion-cli -- gen` out of {} — check that cargo is on PATH.",
                state.repo_root.display()
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if let Some(note) = stop_note(&verb, &output, deadline) {
        stderr.push_str(&note);
    }
    Ok(GenResult {
        ok: output.code == Some(0),
        json: parse_one_object(&stdout),
        stderr,
        exit_code: output.code,
    })
}

/// Stop the `gen` child running under `ticket`; `false` if none is (it already finished, or never
/// started). The run's own `gen_invoke` then returns `ok: false` with the cancel note.
#[tauri::command]
pub fn gen_cancel(
    state: tauri::State<'_, AppStages>,
    tickets: tauri::State<'_, GenTickets>,
    ticket: String,
) -> bool {
    tickets.cancel(&state.children, &ticket)
}

/// How long the host lets one `gen` child run: the build allowance plus the verb's own wait — the
/// human's `--timeout` if it is longer than the verb's 600 s default, else that default.
pub fn gen_deadline(args: &[String]) -> Duration {
    const VERB_DEFAULT: u64 = 600;
    let mut timeout = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = match arg.strip_prefix("--timeout=") {
            Some(value) => Some(value),
            None if arg == "--timeout" => iter.next().map(String::as_str),
            None => continue,
        };
        timeout = value.and_then(|v| v.parse::<u64>().ok());
    }
    BUILD_ALLOWANCE + Duration::from_secs(timeout.unwrap_or(0).max(VERB_DEFAULT))
}

/// Read an image file as a `data:` url, for the candidate grid.
///
/// The webview has no file access of its own and the host declares no asset protocol, so an image
/// reaches a page as bytes in a string — the same shape the Art Director view already renders a
/// `render.screenshot` from. A run folder is scratch, so nothing here is cached.
#[tauri::command(async)]
pub fn read_editor_image(path: String) -> Result<String, String> {
    let file = Path::new(&path);
    let media = image_media_type(file).ok_or_else(|| {
        format!(
            "`{path}` is not an image the editor can show (expected `.png`, `.jpg` or `.webp`)."
        )
    })?;
    let size = std::fs::metadata(file)
        .map_err(|e| format!("cannot read `{path}`: {e}."))?
        .len();
    if size > MAX_IMAGE_BYTES {
        return Err(format!(
            "`{path}` is {size} bytes, over the editor's {MAX_IMAGE_BYTES}-byte inline limit — \
             open it in an image viewer instead."
        ));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read `{path}`: {e}."))?;
    Ok(format!(
        "data:{media};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// A file's size in bytes.
///
/// The one fact the candidate grid can state about a **mesh** candidate before it is accepted:
/// `gen sheet` refuses a mesh run (`L1171`) so there is no thumbnail, and its vertex and triangle
/// counts arrive only with the accept. Separate from [`crate::pty::stat_editor_file`], which reports
/// the mtime the editing sessions poll on and nothing else.
#[tauri::command(async)]
pub fn editor_file_size(path: String) -> Result<u64, String> {
    std::fs::metadata(Path::new(&path))
        .map(|meta| meta.len())
        .map_err(|e| format!("cannot read `{path}`: {e}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| (*a).to_string()).collect()
    }

    fn output(timed_out: bool, cancelled: bool) -> RunOutput {
        RunOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            code: None,
            timed_out,
            cancelled,
        }
    }

    #[test]
    fn the_stop_note_names_who_ended_the_run() {
        let timed = stop_note("run", &output(true, false), Duration::from_secs(90)).unwrap();
        assert!(
            timed.contains("the editor stopped `ludion gen run` after 90 s"),
            "{timed}"
        );
        let cancelled = stop_note("fetch", &output(false, true), Duration::from_secs(90)).unwrap();
        assert_eq!(
            cancelled,
            "\nthe editor cancelled `ludion gen fetch`; jobs it already queued on the ComfyUI \
             server are not cancelled there.\n"
        );
        assert_eq!(
            stop_note("run", &output(false, false), Duration::ZERO),
            None
        );
    }

    #[test]
    fn a_ticket_cancels_the_run_it_names_and_only_while_it_runs() {
        use crate::children::tests::child_of;
        use std::sync::{Arc, mpsc};

        let children = Arc::new(HostChildren::default());
        let tickets = Arc::new(GenTickets::default());
        let (tx, rx) = mpsc::channel();
        let runner = {
            let (children, tickets) = (children.clone(), tickets.clone());
            std::thread::spawn(move || {
                let out = children.run_with(
                    child_of("children::tests::sleeper_child", "LUDION_CHILDREN_SLEEPER"),
                    Duration::from_secs(60),
                    |id| {
                        tickets.begin("t", id);
                        tx.send(()).unwrap();
                    },
                );
                tickets.end("t");
                out
            })
        };
        rx.recv().unwrap();
        assert!(!tickets.cancel(&children, "unknown"));
        assert!(tickets.cancel(&children, "t"));
        let out = runner.join().unwrap().unwrap();
        assert!(out.cancelled);
        assert!(!out.timed_out);
        assert!(!tickets.cancel(&children, "t"), "released once finished");
    }

    #[test]
    fn the_verb_list_is_the_whole_surface() {
        for verb in GEN_VERBS {
            assert!(
                gen_args(verb, &[], None).is_ok(),
                "`{verb}` is listed but refused"
            );
        }
        for denied in [
            "serve", "render", "import", "", "gen", "Check", "run --", "rm",
        ] {
            let err = gen_args(denied, &[], None).unwrap_err();
            assert!(
                err.contains("is not a `ludion gen` verb"),
                "`{denied}`: {err}"
            );
            assert!(
                err.contains("accept, check"),
                "the refusal lists them: {err}"
            );
        }
    }

    #[test]
    fn the_argv_is_the_verb_then_the_callers_args_then_json() {
        assert_eq!(
            gen_args(
                "accept",
                &args(&[
                    "generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d",
                    "3",
                    "--out",
                    "scenes/assets/idol.png",
                ]),
                None,
            )
            .unwrap(),
            args(&[
                "gen",
                "accept",
                "generation/runs/idol-portrait/20260918T113000Z-1a2b3c4d",
                "3",
                "--out",
                "scenes/assets/idol.png",
                "--json",
            ])
        );
    }

    #[test]
    fn a_typed_url_is_appended_once_and_an_empty_one_is_not_appended_at_all() {
        let with = gen_args(
            "check",
            &args(&["r.gen.yaml"]),
            Some(" http://127.0.0.1:8188 "),
        )
        .unwrap();
        assert_eq!(
            with,
            args(&[
                "gen",
                "check",
                "r.gen.yaml",
                "--url",
                "http://127.0.0.1:8188",
                "--json",
            ]),
            "trimmed, and the flag appears exactly once"
        );
        // An empty field in the view means "say nothing": the child then inherits
        // `LUDION_COMFY_URL`, which is the documented default path.
        for blank in ["", "   "] {
            let without = gen_args("check", &args(&["r.gen.yaml"]), Some(blank)).unwrap();
            assert!(!without.contains(&"--url".to_string()), "{without:?}");
        }
    }

    #[test]
    fn an_offline_verb_never_gets_a_url() {
        for verb in OFFLINE_VERBS {
            let argv = gen_args(verb, &args(&["run-dir"]), Some("http://127.0.0.1:8188")).unwrap();
            assert!(!argv.contains(&"--url".to_string()), "{verb}: {argv:?}");
        }
    }

    #[test]
    fn the_caller_cannot_take_over_the_two_flags_the_host_owns() {
        for stolen in ["--json", "--url", "--url=http://elsewhere:8188"] {
            let err = gen_args("run", &args(&["r.gen.yaml", stolen]), None).unwrap_err();
            assert!(err.contains("is set by the host"), "`{stolen}`: {err}");
        }
        // A verb's own flags are untouched — the guard is two names, not a general filter.
        assert!(
            gen_args(
                "run",
                &args(&["r.gen.yaml", "--count", "8", "--seed", "7", "--no-wait"]),
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn the_json_object_is_read_whatever_precedes_it_on_stdout() {
        let object = "{\n  \"kind\": \"image\",\n  \"offline\": \"ok\"\n}\n";
        let parsed = parse_one_object(object).expect("a bare object parses");
        assert_eq!(parsed["kind"], "image");

        // The verbs print one compact line, with or without a stray line before it.
        let compact = "{\"kind\":\"image\",\"offline\":\"ok\"}\n";
        assert_eq!(parse_one_object(compact).unwrap()["offline"], "ok");
        let noisy_compact = format!("Blocking waiting for file lock\n{compact}");
        assert_eq!(parse_one_object(&noisy_compact).unwrap()["kind"], "image");

        // A line the tool did not mean to print must not lose the object under it.
        let noisy = format!("Blocking waiting for file lock\n{object}");
        assert_eq!(
            parse_one_object(&noisy).expect("the object is still found")["offline"],
            "ok"
        );
    }

    #[test]
    fn stdout_without_an_object_is_none_not_a_guess() {
        assert_eq!(parse_one_object(""), None);
        assert_eq!(parse_one_object("error: no such recipe\n"), None);
        // A JSON array or scalar is not what a `--json` verb prints, so it is not accepted as one.
        assert_eq!(parse_one_object("[1, 2]"), None);
        assert_eq!(parse_one_object("\"ok\""), None);
        // A truncated object (a killed child) is a parse failure, never a partial reading.
        assert_eq!(parse_one_object("{\"kind\": \"im"), None);
    }

    #[test]
    fn the_deadline_is_the_build_allowance_plus_the_longer_of_the_timeout_and_the_default() {
        let floor = BUILD_ALLOWANCE + Duration::from_secs(600);
        let long = BUILD_ALLOWANCE + Duration::from_secs(3600);
        assert_eq!(gen_deadline(&args(&["r.gen.yaml"])), floor);
        assert_eq!(
            gen_deadline(&args(&["r.gen.yaml", "--timeout", "3600"])),
            long
        );
        assert_eq!(gen_deadline(&args(&["r.gen.yaml", "--timeout=3600"])), long);
        assert_eq!(gen_deadline(&args(&["--timeout", "5"])), floor);
        assert_eq!(gen_deadline(&args(&["--timeout", "soon"])), floor);
        assert_eq!(gen_deadline(&args(&["--timeout"])), floor);
    }

    #[test]
    fn only_inlineable_image_extensions_are_offered_a_media_type() {
        assert_eq!(image_media_type(Path::new("0003.png")), Some("image/png"));
        assert_eq!(image_media_type(Path::new("a/B.PNG")), Some("image/png"));
        assert_eq!(image_media_type(Path::new("x.jpeg")), Some("image/jpeg"));
        assert_eq!(image_media_type(Path::new("x.webp")), Some("image/webp"));
        // A mesh candidate has no thumbnail by construction (`gen sheet` refuses a mesh run), so
        // the grid must never be handed one to try.
        assert_eq!(image_media_type(Path::new("0000.glb")), None);
        assert_eq!(image_media_type(Path::new("run.json")), None);
        assert_eq!(image_media_type(Path::new("noext")), None);
    }

    #[test]
    fn a_refused_image_names_the_path_and_what_was_expected() {
        let err = read_editor_image("0000.glb".to_string()).unwrap_err();
        assert!(err.contains("0000.glb"), "{err}");
        assert!(err.contains(".png"), "{err}");
        let err = read_editor_image("no-such-candidate.png".to_string()).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    #[test]
    fn an_image_round_trips_as_a_data_url_and_its_size_is_readable() {
        let dir = std::env::temp_dir().join(format!("ludion-gen-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0000.png");
        std::fs::write(&path, b"\x89PNG\r\n").unwrap();
        let name = path.to_string_lossy().into_owned();
        let url = read_editor_image(name.clone()).unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        let encoded = url.trim_start_matches("data:image/png;base64,");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
            b"\x89PNG\r\n"
        );
        assert_eq!(editor_file_size(name).unwrap(), 6);
        assert!(
            editor_file_size(dir.join("nope.glb").to_string_lossy().into_owned())
                .unwrap_err()
                .contains("cannot read")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
