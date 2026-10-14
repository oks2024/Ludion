# Wave artifacts

One-shot machine outputs kept as evidence for a plan in `Plan/`: golden-bless SSIM diff reports
(`*.diff.json`, written by the testkit's comparison run) and `ludion map-check` transcripts
(`*.map-check.txt`). Each is named `<plan-or-arc>-<wave>-<subject>` and is referenced from the
Status section of the plan it belongs to.

They are committed because a re-bless has to be auditable after the fact — the report says which
tiles moved and by how much, which the golden PNG alone cannot. They are never regenerated in place
and nothing reads them at build or test time; a stale one is history, not drift.

A PNG sweep sheet in this folder is kept only while an open plan still cites it; once every citing
plan closes, the PNG is deleted (its filename and analysis stay in prose, marked "in Lore history").
JSON, Markdown and TXT artifacts stay regardless. A new sweep sheet is downscaled below 1 MB before
it is committed: `scripts/size-gate.mjs` fails any file over that line not named in
`scripts/size-allowlist.txt`.
