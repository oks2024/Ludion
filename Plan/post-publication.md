# Post-publication

Hygiene items the pre-publication audit deferred past publication. Each is one line with the trigger
that brings it back; none blocks the repository being public. Nothing here is a build plan.

- **Budget file header tone** — `budgets/*.toml` headers read as internal notes. *Trigger:* the next budget re-measure.
- **Schedule-language comments** — about 500 in code and about 160 scene-YAML header lines describe sequencing instead of state. *Trigger:* the file is touched, or a dedicated sweep once the queue is empty.
- **Remaining `eprintln!` sites** — those outside host and CLI output should report through diagnostics. *Trigger:* the next audit pass.
- **Editor native-command scoping** — the integrated terminal (`pty_spawn`) and file I/O are unrestricted. *Trigger:* the editor opening projects from untrusted sources, or a security report.
- **Linux build proof** — a green hosted `push.yml` and `ci.yml` run, then Linux becomes a supported target. *Trigger:* the first hosted CI run.
- **M4 performance budgets and the cross-arch hash comparison** — a measurement run on the Apple M4. *Trigger:* the next Mac sitting.
- **Replace `bincode` 1.3.3** — RUSTSEC-2025-0141 (unmaintained; ignored in `deny.toml`), a direct dependency of `ludion-physics` pinned for the physics snapshot bytes. *Trigger:* the next physics snapshot format change, since replacing it moves the format.
