//! Live reload of a font table or face document (`Plan/font-assets.md` W3): `asset.reload` rebuilds
//! the library, and is refused while a recording is armed.

use std::path::{Path, PathBuf};

use ludion_engine::{Engine, FontLibrary};
use ludion_protocol::command::AssetReloadParams;
use ludion_protocol::{Command, ResponseBody};

fn root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ludion-font-reload-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    let gallery = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gallery/fonts");
    for file in [
        "inter.font.yaml",
        "inter.png",
        "silkscreen.font.yaml",
        "silkscreen.png",
    ] {
        std::fs::copy(gallery.join(file), dir.join("fonts").join(file)).unwrap();
    }
    std::fs::write(
        dir.join("fonts.yaml"),
        "format: 1\nkind: fonts\nfonts:\n  body: fonts/inter.font.yaml\n  pixel: fonts/silkscreen.font.yaml\n",
    )
    .unwrap();
    dir
}

fn engine(dir: &Path) -> Engine {
    let mut engine = Engine::builder().project_root(dir).build().unwrap();
    engine
        .load_inline_scene("format: 1\nentities: []\n")
        .unwrap();
    engine
}

fn reload(
    engine: &mut Engine,
    path: &str,
) -> Result<ResponseBody, ludion_core::diagnostics::Diagnostic> {
    engine
        .execute(Command::AssetReload(AssetReloadParams {
            path: Some(path.to_string()),
        }))
        .map(|answer| answer.body)
}

fn advance_of(engine: &Engine, ch: char) -> f32 {
    let library = engine.world().resource::<FontLibrary>().unwrap();
    ludion_font::layout(library.face(Some("body")), &ch.to_string(), 100.0, 0.0).extent()[0]
}

#[test]
fn reloading_a_face_document_refreshes_measurement() {
    let dir = root("reload");
    let mut engine = engine(&dir);
    let before = advance_of(&engine, 'H');

    // Widen the 'H' glyph's advance in the baked document.
    let doc_path = dir.join("fonts/inter.font.yaml");
    let source = std::fs::read_to_string(&doc_path).unwrap();
    let doc = ludion_scene::check_font(&source).unwrap();
    let glyph = doc.glyphs.iter().find(|g| g.ch == 'H').expect("an H glyph");
    let old = format!("    advance: {}\n", glyph.advance);
    let at = source.find("  - char: H\n").expect("the H entry");
    let pos = at + source[at..].find(&old).expect("its advance line");
    let mut edited = source.clone();
    edited.replace_range(
        pos..pos + old.len(),
        &format!("    advance: {}\n", glyph.advance + 600),
    );
    std::fs::write(&doc_path, edited).unwrap();

    let reply = reload(&mut engine, "fonts/inter.font.yaml").expect("the reload applies");
    assert!(matches!(reply, ResponseBody::AssetsReloaded { paths } if paths.len() == 1));
    assert!(
        advance_of(&engine, 'H') > before,
        "measurement sees the new advance"
    );

    // A path the library does not hold changes nothing.
    let reply = reload(&mut engine, "fonts/unrelated.yaml").expect("an unrelated path is ignored");
    assert!(matches!(reply, ResponseBody::AssetsReloaded { paths } if paths.is_empty()));
}

#[test]
fn reloading_a_face_while_recording_is_refused() {
    let dir = root("reload-recording");
    let mut engine = engine(&dir);
    let before = advance_of(&engine, 'H');
    engine.set_recording(std::io::sink()).unwrap();
    std::fs::write(
        dir.join("fonts/inter.font.yaml"),
        std::fs::read_to_string(dir.join("fonts/inter.font.yaml")).unwrap() + "\n",
    )
    .unwrap();

    let error = reload(&mut engine, "fonts/inter.font.yaml").expect_err("refused while recording");
    assert_eq!(error.code.as_str(), "L1255");
    assert_eq!(advance_of(&engine, 'H'), before, "the library is unchanged");
}
