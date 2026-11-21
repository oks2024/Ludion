//! Localization (`ludion docs localization`): `kind: strings`, `kind: locales`, the active locale
//! and keyed `UiText`/`Text`. Every test writes its docs to a fresh temp root.

use std::path::{Path, PathBuf};

use ludion_core::codes::code;
use ludion_core::ids::EntityId;
use ludion_engine::components::{Text, UiText};
use ludion_engine::{Diagnostics, Engine, EngineBuilder, extract, localization};
use ludion_protocol::command::{
    AssetReloadParams, EngineRestoreParams, SceneSaveParams, SimStepParams,
};
use ludion_protocol::{Command, ResponseBody};
use ludion_renderer::UiInstance;
use serde_json::Value;

const STRINGS_EN: &str = "format: 1
kind: strings
locale: en
strings:
  hud.start: Start game
  hud.ammo: \"Ammo {count}\"
  hud.only_en: English only
  world.sign: Exit
";

const STRINGS_FR: &str = "format: 1
kind: strings
locale: fr
strings:
  hud.start: Demarrer le jeu
  hud.ammo: \"Munitions {count}\"
  world.sign: Sortie
";

const SCENE: &str = "format: 1
seed: 1
entities:
  - name: start
    components:
      UiText:
        text: Start (authored)
        key: hud.start
  - name: sign
    components:
      Text:
        text: Exit (authored)
        key: world.sign
";

const PLAIN_SCENE: &str = "format: 1
seed: 1
entities:
  - name: plain
    components:
      UiText:
        text: Hello
  - name: sign
    components:
      Text:
        text: World
";

fn fresh_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("ludion-localization-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A fresh root holding the English and French tables.
fn root_with_tables(name: &str) -> PathBuf {
    let root = fresh_dir(name);
    write(&root, "strings/en.yaml", STRINGS_EN);
    write(&root, "strings/fr.yaml", STRINGS_FR);
    root
}

fn game(root: &Path) -> EngineBuilder {
    Engine::builder().project_root(root).localization()
}

fn boot(name: &str, scene: &str) -> (Engine, PathBuf) {
    let root = root_with_tables(name);
    let mut engine = game(&root).build().expect("the tables are valid");
    engine.load_inline_scene(scene).unwrap();
    (engine, root)
}

fn tick(engine: &mut Engine) {
    engine
        .execute(Command::SimStep(SimStepParams {
            ticks: 1,
            hash: false,
        }))
        .unwrap();
}

fn ui_ids(engine: &mut Engine) -> Vec<EntityId> {
    let mut ids: Vec<EntityId> = engine
        .world_mut()
        .query_ids::<(&UiText,)>()
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids
}

fn the_ui_text(engine: &mut Engine) -> UiText {
    let id = ui_ids(engine)[0];
    engine.world().get::<UiText>(id).unwrap().unwrap().clone()
}

fn the_text(engine: &mut Engine) -> Text {
    let id = engine
        .world_mut()
        .query_ids::<(&Text,)>()
        .map(|(id, _)| id)
        .min()
        .unwrap();
    engine.world().get::<Text>(id).unwrap().unwrap().clone()
}

fn reported(engine: &Engine, code: ludion_core::diagnostics::DiagnosticCode) -> Vec<String> {
    engine
        .world()
        .resource::<Diagnostics>()
        .unwrap()
        .reported(code)
        .map(String::from)
        .collect()
}

fn failing(builder: EngineBuilder) -> String {
    match builder.build() {
        Ok(_) => panic!("the build should have failed"),
        Err(error) => format!("{error:?}"),
    }
}

fn drawn_ui_text(engine: &Engine, root: &Path) -> String {
    let scene = extract(
        engine.world(),
        (1920, 1080),
        ludion_assets::AssetRoot::content(root),
    )
    .unwrap();
    match &scene.ui[0] {
        UiInstance::Text { text, .. } => text.clone(),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn a_keyed_text_resolves_and_follows_the_locale() {
    let (mut engine, _root) = boot("resolve", SCENE);
    tick(&mut engine);
    let ui = the_ui_text(&mut engine);
    assert_eq!(ui.shown(), "Start game");
    assert_eq!(
        ui.text, "Start (authored)",
        "the authored text is never written"
    );
    assert_eq!(the_text(&mut engine).shown(), "Exit");

    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    let ui = the_ui_text(&mut engine);
    assert_eq!(ui.shown(), "Demarrer le jeu");
    assert_eq!(ui.text, "Start (authored)");
    assert_eq!(the_text(&mut engine).shown(), "Sortie");
    // Measured over the French string.
    let face = ludion_font::FontFace::embedded();
    let rect = ludion_engine::ui::resolve_ui_text(face, &ui, (1920, 1080));
    let block = ludion_engine::ui::measure_ui_text(face, "Demarrer le jeu", ui.size);
    assert_eq!([rect[2], rect[3]], block);
}

#[test]
fn text_args_fill_placeholders_and_a_missing_one_warns() {
    let scene = "format: 1
seed: 1
entities:
  - name: ammo
    components:
      UiText:
        key: hud.ammo
  - name: bare
    components:
      Text:
        key: hud.ammo
";
    let (mut engine, _root) = boot("args", scene);
    let id = ui_ids(&mut engine)[0];
    assert!(localization::set_text_args(
        engine.world_mut(),
        id,
        &[("count", "12")]
    ));
    tick(&mut engine);
    assert_eq!(the_ui_text(&mut engine).shown(), "Ammo 12");
    assert_eq!(the_text(&mut engine).shown(), "Ammo {count}");
    assert_eq!(
        reported(&engine, code::STRING_ARG_MISSING),
        ["string_arg:hud.ammo/count"]
    );
    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    assert_eq!(the_ui_text(&mut engine).shown(), "Munitions 12");
}

#[test]
fn fallback_and_missing_keys_report_once() {
    let scene = "format: 1
seed: 1
entities:
  - name: a
    components:
      UiText:
        key: hud.only_en
  - name: b
    components:
      Text:
        key: no.such
";
    let (mut engine, _root) = boot("fallback", scene);
    localization::set_locale(engine.world_mut(), "fr");
    for _ in 0..3 {
        tick(&mut engine);
    }
    assert_eq!(the_ui_text(&mut engine).shown(), "English only");
    assert_eq!(the_text(&mut engine).shown(), "no.such");
    assert_eq!(
        reported(&engine, code::STRING_FELL_BACK),
        ["string_fell_back:fr/hud.only_en"]
    );
    assert_eq!(
        reported(&engine, code::STRING_MISSING),
        ["string_missing:no.such"]
    );
}

#[test]
fn a_strings_hot_reload_re_resolves() {
    let (mut engine, root) = boot("reload", SCENE);
    tick(&mut engine);
    assert_eq!(the_ui_text(&mut engine).shown(), "Start game");
    write(
        &root,
        "strings/en.yaml",
        &STRINGS_EN.replace("Start game", "Begin"),
    );
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some("strings/en.yaml".to_string()),
        }))
        .unwrap();
    tick(&mut engine);
    assert_eq!(the_ui_text(&mut engine).shown(), "Begin");
}

#[test]
fn the_locales_doc_names_the_default_and_validates() {
    let locales = "format: 1\nkind: locales\ndefault: fr\nlocales:\n  en:\n    name: English\n  fr:\n    name: French\n";
    let root = root_with_tables("locales-default");
    write(&root, "locales.yaml", locales);
    let mut engine = game(&root).build().unwrap();
    assert_eq!(localization::default_locale(engine.world()), "fr");
    assert_eq!(localization::locale(engine.world_mut()), "fr");

    // L1722: a default that is not listed, and a second locales doc.
    let unlisted = root_with_tables("locales-unlisted");
    write(
        &unlisted,
        "locales.yaml",
        "format: 1\nkind: locales\ndefault: de\nlocales:\n  en:\n    name: English\n  fr:\n    name: French\n",
    );
    assert!(failing(game(&unlisted)).contains("L1722"));
    let two = root_with_tables("locales-two");
    write(&two, "locales.yaml", locales);
    write(&two, "more/locales.yaml", locales);
    assert!(failing(game(&two)).contains("L1722"));

    // L1723: a strings locale the doc does not list.
    let missing = root_with_tables("locales-strings");
    write(
        &missing,
        "locales.yaml",
        "format: 1\nkind: locales\ndefault: en\nlocales:\n  en:\n    name: English\n",
    );
    assert!(failing(game(&missing)).contains("L1723"));
}

#[test]
fn keyed_text_without_the_opt_in_draws_its_text_and_warns_once() {
    let root = fresh_dir("no-opt-in");
    let mut engine = Engine::builder().project_root(&root).build().unwrap();
    engine.load_inline_scene(SCENE).unwrap();
    for _ in 0..3 {
        tick(&mut engine);
    }
    assert_eq!(the_ui_text(&mut engine).shown(), "Start (authored)");
    assert_eq!(drawn_ui_text(&engine, &root), "Start (authored)");
    assert_eq!(reported(&engine, code::LOCALIZATION_NOT_ENABLED).len(), 1);
}

#[test]
fn a_locale_without_strings_warns_once_and_falls_back() {
    let (mut engine, _root) = boot("no-table", SCENE);
    for _ in 0..2 {
        localization::set_locale(engine.world_mut(), "de");
        tick(&mut engine);
    }
    assert_eq!(localization::locale(engine.world_mut()), "de");
    assert_eq!(the_ui_text(&mut engine).shown(), "Start game");
    assert_eq!(
        reported(&engine, code::LOCALE_WITHOUT_STRINGS),
        ["locale_without_strings:de"]
    );
}

fn saved(engine: &mut Engine, root: &Path, name: &str) -> String {
    ludion_engine::dispatch(
        engine,
        Command::SceneSave(SceneSaveParams {
            path: name.to_string(),
            force: true,
        }),
    )
    .unwrap();
    std::fs::read_to_string(root.join(name)).unwrap()
}

#[test]
fn saving_under_fr_equals_saving_under_en() {
    let (mut engine, root) = boot("save", SCENE);
    tick(&mut engine);
    let en = saved(&mut engine, &root, "out_en.yaml");
    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    let fr = saved(&mut engine, &root, "out_fr.yaml");
    assert_eq!(en, fr);
    assert!(
        en.contains("key: hud.start") && en.contains("key: world.sign"),
        "{en}"
    );
    assert!(
        en.contains("Start (authored)") && en.contains("Exit (authored)"),
        "{en}"
    );
    assert!(!fr.contains("Demarrer") && !fr.contains("Sortie"), "{fr}");
}

#[test]
fn resolved_is_not_hashed() {
    let (mut engine, _root) = boot("hash", SCENE);
    tick(&mut engine);
    let resolved = engine.world_hash().unwrap();
    let ui = ui_ids(&mut engine)[0];
    engine
        .world_mut()
        .get_mut::<UiText>(ui)
        .unwrap()
        .unwrap()
        .resolved = None;
    assert_eq!(engine.world_hash().unwrap(), resolved);
}

#[test]
fn unkeyed_text_hashes_the_same_with_and_without_localization() {
    let root = fresh_dir("unkeyed");
    let mut plain = Engine::builder().project_root(&root).build().unwrap();
    plain.load_inline_scene(PLAIN_SCENE).unwrap();
    let mut localized = game(&root).build().unwrap();
    localized.load_inline_scene(PLAIN_SCENE).unwrap();
    for _ in 0..3 {
        tick(&mut plain);
        tick(&mut localized);
    }
    assert_eq!(plain.world_hash().unwrap(), localized.world_hash().unwrap());
}

fn snapshot_of(engine: &mut Engine) -> Value {
    match engine.execute(Command::EngineSnapshot).unwrap().body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    }
}

fn restore(engine: &mut Engine, snapshot: Value) {
    match engine
        .execute(Command::EngineRestore(EngineRestoreParams { snapshot }))
        .unwrap()
        .body
    {
        ResponseBody::Restored { .. } => {}
        other => panic!("expected a restore, got {other:?}"),
    }
}

fn run_and_snapshot(name: &str) -> (Engine, PathBuf, Value) {
    let (mut engine, root) = boot(name, SCENE);
    tick(&mut engine);
    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    let snapshot = snapshot_of(&mut engine);
    (engine, root, snapshot)
}

#[test]
fn a_restore_keeps_the_locale_and_draws_the_resolved_text() {
    let (mut engine, root, snapshot) = run_and_snapshot("restore");
    let hash = engine.world_hash().unwrap();
    // Wander off, then restore.
    localization::set_locale(engine.world_mut(), "en");
    tick(&mut engine);
    restore(&mut engine, snapshot.clone());
    assert_eq!(localization::locale(engine.world_mut()), "fr");
    assert_eq!(engine.world_hash().unwrap(), hash);
    // Before any tick: the first extract already shows French.
    assert_eq!(drawn_ui_text(&engine, &root), "Demarrer le jeu");

    // A fresh engine restored from the same snapshot lands on the same hash.
    let mut other = game(&root).build().unwrap();
    other.load_inline_scene(SCENE).unwrap();
    restore(&mut other, snapshot);
    assert_eq!(other.world_hash().unwrap(), hash);
    assert_eq!(drawn_ui_text(&other, &root), "Demarrer le jeu");
}

#[test]
fn two_runs_hash_alike() {
    let (mut a, _ra, _sa) = run_and_snapshot("det-a");
    let (mut b, _rb, _sb) = run_and_snapshot("det-b");
    assert_eq!(a.world_hash().unwrap(), b.world_hash().unwrap());
}

/// A root with the keyed `UiText` on the `body` face, whose `fr` override is the bitmap face.
fn root_with_locale_faces(name: &str) -> PathBuf {
    let root = root_with_tables(name);
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in [
        "inter.font.yaml",
        "inter.png",
        "silkscreen.font.yaml",
        "silkscreen.png",
    ] {
        std::fs::create_dir_all(root.join("fonts")).unwrap();
        std::fs::copy(gallery.join(file), root.join("fonts").join(file)).unwrap();
    }
    write(
        &root,
        "fonts.yaml",
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  default: fonts/inter.font.yaml\n\
         locales:\n  fr:\n    body: fonts/silkscreen.font.yaml\n",
    );
    root
}

const FACE_SCENE: &str = "format: 1
seed: 1
entities:
  - name: start
    components:
      UiText:
        text: Start (authored)
        key: hud.start
        font: body
";

/// What extract draws for the one keyed `UiText`: whether its face is SDF, and the right edge of
/// its quads and their vertical middle.
fn drawn_face(engine: &Engine, root: &Path) -> (bool, f32, f32) {
    let scene = extract(
        engine.world(),
        (1920, 1080),
        ludion_assets::AssetRoot::content(root),
    )
    .unwrap();
    match &scene.ui[0] {
        UiInstance::Text { font, quads, .. } => {
            let top = quads.iter().map(|q| q.pos[1]).fold(f32::MAX, f32::min);
            let bottom = quads.iter().map(|q| q.pos[3]).fold(f32::MIN, f32::max);
            (
                font.expect("a table face is placed").sdf,
                quads.iter().map(|q| q.pos[2]).fold(f32::MIN, f32::max),
                (top + bottom) / 2.0,
            )
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

fn hits(engine: &Engine, cursor: [f32; 2]) -> bool {
    let id = engine
        .world()
        .entities()
        .find(|id| engine.world().get::<UiText>(*id).ok().flatten().is_some())
        .unwrap();
    ludion_engine::ui::ui_hit(
        engine.world(),
        id,
        (1920, 1080),
        cursor,
        ludion_core::time::SimTime::from_ticks(0, ludion_core::time::TickRate::DEFAULT),
    )
}

#[test]
fn the_active_locale_picks_the_face_for_atlas_layout_and_hit_test() {
    let root = root_with_locale_faces("face");
    let mut engine = game(&root).build().expect("valid");
    engine.load_inline_scene(FACE_SCENE).unwrap();
    tick(&mut engine);

    let (sdf_en, right_en, mid_en) = drawn_face(&engine, &root);
    assert!(sdf_en, "en uses the base SDF face");
    assert!(hits(&engine, [right_en - 1.0, mid_en]));
    assert!(!hits(&engine, [right_en + 40.0, mid_en]));

    localization::set_locale(engine.world_mut(), "fr");
    tick(&mut engine);
    let (sdf_fr, right_fr, mid_fr) = drawn_face(&engine, &root);
    assert!(!sdf_fr, "fr uses the override's bitmap face");
    assert!(hits(&engine, [right_fr - 1.0, mid_fr]));
    assert!(!hits(&engine, [right_fr + 40.0, mid_fr]));
}

fn fonts_root(name: &str, fonts_table: &str, en_extra: &str) -> PathBuf {
    let root = root_with_tables(name);
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in [
        "inter.font.yaml",
        "inter.png",
        "silkscreen.font.yaml",
        "silkscreen.png",
    ] {
        std::fs::create_dir_all(root.join("fonts")).unwrap();
        std::fs::copy(gallery.join(file), root.join("fonts").join(file)).unwrap();
    }
    write(&root, "fonts.yaml", fonts_table);
    write(
        &root,
        "strings/extra.yaml",
        &format!("format: 1\nkind: strings\nlocale: en\nstrings:\n{en_extra}"),
    );
    root
}

fn glyph_warnings(root: &Path) -> Vec<String> {
    let engine = game(root).build().expect("the tables are valid");
    engine
        .doc_validation_warnings()
        .iter()
        .filter(|d| d.code == ludion_core::codes::code::FONT_GLYPH_MISSING)
        .map(|d| d.path.clone().unwrap_or_default())
        .collect()
}

const CHAINED_DEFAULT: &str = "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  default:\n    path: fonts/silkscreen.font.yaml\n    fallback:\n      - body\n";

#[test]
fn a_string_table_is_checked_whole_against_the_default_face_chain() {
    // The accent is in the fallback; the snowman is in no chain face.
    let root = fonts_root(
        "table-glyphs",
        CHAINED_DEFAULT,
        "  chain.covered: \"CAF\u{C9}\"\n  chain.missing: \"CAF\u{C9} \u{2603}\"\n",
    );
    assert_eq!(
        glyph_warnings(&root),
        ["strings/strings/extra.yaml/strings/chain.missing"]
    );
}

#[test]
fn a_font_table_without_a_default_skips_the_whole_table_check() {
    let root = fonts_root(
        "table-no-default",
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n",
        "  chain.missing: \"\u{2603}\"\n",
    );
    assert!(glyph_warnings(&root).is_empty());
}
