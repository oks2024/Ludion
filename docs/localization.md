---
description: Localization: kind strings/locales, the active locale, keyed UiText/Text, fallbacks, args
---

# Localization (strings, locales, keyed text)

**Scope:** string tables (`kind: strings`), the shipped-locale list (`kind: locales`), the active locale and
`UiText`/`Text` with a `key`. Voiced and queued lines that use the same tables are `ludion docs dialogue`; faces and the
`font` field are `ludion docs fonts`; the UI components are `ludion docs components-ui`.

## The loop

1. `Engine::builder().localization()` opts in (`.dialogue(..)` implies it). It registers both doc kinds, the `LocaleState`
   component and the `localize_text` system, which runs where you called it in the system order.
2. Write the docs under the content root (below).
3. Put `key: hud.start` on a `UiText` or `Text`. Leave `text` as the authored fallback.
4. `localization::set_locale(world, "fr")` switches the language; keyed text re-resolves on the next tick.
5. For a placeholder, `localization::set_text_args(world, entity, &[("count", "12")])` after the system that knows the
   value.

## The docs

```yaml
format: 1
kind: strings
locale: en
strings:
  hud.start: Start game
  hud.ammo: "Ammo {count}"
```

One locale per doc; several docs may share a locale, and a key appears once per locale (`L1711`). `{name}` is a
placeholder; `{{` and `}}` are literal braces. A translation uses the default locale's placeholders (`L1716`, warning).
A doc has a `locale` (`L1718`).

```yaml
format: 1
kind: locales
default: en
locales:
  en:
    name: English
  fr:
    name: Francais
```

At most one `kind: locales` doc, and `default` must be one of the listed `locales` (`L1722`). With the doc, every
`kind: strings` locale must be listed (`L1723`). Without it the default is `en` and any tag is accepted. The default is
the fallback for every other locale and the one a dialogue line's keys must exist in (`L1715`).

## Keyed text

`UiText` and `Text` take `key` (a string-table key) and `args` (placeholder values; usually set through `set_text_args`).
The engine never writes `text`: it resolves into a runtime-only `resolved` field, and every consumer (layout, wrap,
hit test, bounds, draw) reads `shown()`, which is `resolved` when set, else `text`. So:

- A scene saved under `fr` is byte-identical to one saved under `en`, and `text` is what the editor shows.
- `resolved` is never serialized, hashed or snapshotted; `key`, `args` and the locale are. A restore resolves again
  before the first extract. Measurement is sim-visible: `measure_ui_text` over `shown()` is the French width in `fr`.
- Clearing `key` clears `resolved`; the authored `text` draws again.

Lookup is the active locale, then the default locale (`L5132`, once per locale and key), then the key itself (`L5133`,
once per key). A missing argument leaves `{name}` and warns `L5134` once. Text re-resolves every tick, so a strings doc
hot reload (`asset.reload`) shows on the next tick.

## The active locale

`LocaleState` is one lazily spawned entity, hashed and snapshotted, and skipped by `scene.save`: the locale is not saved into scenes. `localization::locale`
reads it (the default when unset). `set_locale` to a tag with no `kind: strings` doc still applies and warns `L5140`
once per tag.

## Per-locale faces

`fonts.yaml` may carry `locales: { fr: { body: fonts/other.font.yaml } }` (`ludion docs fonts`). Every site that draws,
measures or hit-tests text asks `FontLibrary::face(name, locale)` with `localization::active_locale(world)`, so the
layout, the atlas and the hit test agree per locale. A world with no `locales:` overrides resolves exactly as before.

A face with a `fallback` chain is checked whole: `validate_localization_docs` warns `L1256` once per (locale, key)
whose string has characters that locale's `default` face chain lacks (skipped without a `default` entry).

## Render hosts

A bare render host (`ludion render`, a `GoldenTest`, a gallery discovery run) has no engine builder, so it loads the
content root's `kind: strings` and `kind: locales` docs itself and resolves keyed text before it extracts. A scene
with no keyed `UiText`/`Text` skips this, so a malformed strings file beside it cannot break its render.

- `ludion render --scene <scene> --locale fr` renders in a locale; absent, the folder's default locale. It is refused
  with `--sweep`, a multi-value `--frames` strip and the model, sprite, atlas, font and paired modes.
- `GoldenTest::with_locale("fr")` is the same for a hand-written golden test.
- Worked example: `examples/gallery/localization.yaml` (keys in `strings/en.yaml` and `strings/fr.yaml`; the gallery's
  `fonts.yaml` swaps the `pixel` face to Inter under `fr`). Its discovered golden is `en`; `localization-fr` is a test
  in `crates/ludion-testkit/tests/gallery.rs`.

## check-scene

`ludion check-scene` loads the content root's `kind: strings` and `kind: locales` docs when the scene has a keyed `UiText`
or `Text`. A key the default locale lacks is `L1715` (error, at `.../key`). Each locale's string, with `{placeholders}`
removed, is checked against that locale's face: characters it lacks are `L1256` (warning) naming the locale. A locale
without the key is checked with the default's string. Unkeyed text is checked once, against the base face.

## Gotchas

- A keyed text on an engine without `.localization()` draws its authored `text` and warns `L5139` once.
- `localization::text` and `format_text` resolve a key for game code (a toast, a log line); they use the active locale.
- Keys on `UiList` and `UiRadial` do not exist: resolve the string with `text` and write it to the item.
- A locale draws with the face the component names, or that locale's override (see Per-locale faces). Fallback chains
  do not exist: a locale whose characters its face lacks draws `.notdef` glyphs.
- Plurals, gender, right-to-left and CJK shaping are not handled.
- Codes: `L1722` bad locales doc, `L1723` unlisted strings locale, `L5139`/`L5140` warnings.
- Run the example: `cargo run -p ludion --example dialogue`.
