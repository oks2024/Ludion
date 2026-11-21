//! The correctness pin for the linear grid decode: `VoxelDocument::for_each_cell` must answer, cell
//! for cell, exactly what a per-cell `VoxelDocument::palette_index` loop answers.
//!
//! `for_each_cell` decodes each row string once through a palette lookup while `palette_index`
//! restarts the row at every column and rescans the palette at every glyph — the whole point of the
//! rewrite, and the reason the two can drift apart silently. So the equality is asserted here rather
//! than assumed: over every voxel model committed in the repo (real palettes, real widths, real
//! empty cells) and over a synthetic model wider than any of them, carrying mixed glyphs, empty
//! cells, and a glyph no palette entry claims.

use ludion_scene::{PaletteEntry, VoxelDocument, check_voxel};

/// Every `*.voxel.yaml` committed under `games/*/scenes/voxels/`, as `(display path, source)`. Empty
/// outside a checkout (a packaged crate has no `games/`), which the caller treats as nothing to
/// compare.
fn committed_models() -> Vec<(String, String)> {
    let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("games");
    let Ok(games) = std::fs::read_dir(&games) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for game in games.flatten() {
        let voxels = game.path().join("scenes").join("voxels");
        let Ok(entries) = std::fs::read_dir(&voxels) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.to_string_lossy().ends_with(".voxel.yaml") {
                continue;
            }
            if let Ok(source) = std::fs::read_to_string(&path) {
                found.push((
                    format!(
                        "games/{}/scenes/voxels/{}",
                        game.file_name().to_string_lossy(),
                        entry.file_name().to_string_lossy()
                    ),
                    source,
                ));
            }
        }
    }
    found.sort();
    found
}

/// Asserts the two decodes agree on every in-bounds cell, and that the sweep visits each cell of
/// `size` exactly once in `(y, z, x)` scan order. Returns the cells visited.
fn assert_decode_matches(name: &str, doc: &VoxelDocument) -> u64 {
    let [sx, sy, sz] = doc.size;
    let mut expected = (0, 0, 0);
    let mut visited = 0u64;
    doc.for_each_cell(|x, y, z, index| {
        assert_eq!(
            (x, y, z),
            expected,
            "{name}: cells arrive in (y, z, x) scan order"
        );
        assert_eq!(
            index,
            doc.palette_index(x, y, z),
            "{name}: cell ({x}, {y}, {z}) decodes differently than `palette_index` reads it"
        );
        visited += 1;
        expected = match expected {
            (x, y, z) if x + 1 < sx => (x + 1, y, z),
            (_, y, z) if z + 1 < sz => (0, y, z + 1),
            (_, y, _) => (0, y + 1, 0),
        };
    });
    assert_eq!(
        visited,
        u64::from(sx) * u64::from(sy) * u64::from(sz),
        "{name}: every cell of `size` is visited exactly once"
    );
    visited
}

/// The pin over real content: every committed model, compared whole. A source that does not
/// validate is skipped rather than failed — this checkout is shared, and whether the models on disk
/// are all valid is `check-scene`'s question, not this one's.
#[test]
fn decoded_cells_match_palette_index_on_committed_models() {
    let models = committed_models();
    let mut compared = 0;
    for (name, source) in &models {
        let Ok(doc) = check_voxel(source) else {
            continue;
        };
        compared += 1;
        let cells = assert_decode_matches(name, &doc);
        assert_eq!(cells, doc.cell_count(), "{name}: cell count agrees");
        let filled = doc.filled_count();
        let by_index = (0..doc.size[1])
            .flat_map(|y| (0..doc.size[2]).map(move |z| (y, z)))
            .flat_map(|(y, z)| (0..doc.size[0]).map(move |x| (x, y, z)))
            .filter(|&(x, y, z)| doc.palette_index(x, y, z).is_some())
            .count() as u64;
        assert_eq!(
            filled, by_index,
            "{name}: `filled_count` counts the same cells"
        );
    }
    assert!(
        models.is_empty() || compared > 0,
        "models were found on disk but none of them validated"
    );
}

/// The pin over a shape no committed model has: wider than any of them, with mixed glyphs, empty
/// cells, a glyph the palette does not define, and a short row — the cases where a lookup and a
/// per-cell scan could disagree. Built as a value rather than parsed, because an undefined glyph is
/// an `L1114` a valid document cannot carry.
#[test]
fn decoded_cells_match_palette_index_on_a_wide_mixed_model() {
    let width = 512u32;
    let palette: Vec<PaletteEntry> = ['#', 'A', 'z', '~']
        .iter()
        .map(|glyph| PaletteEntry {
            glyph: glyph.to_string(),
            color: [0.5, 0.5, 0.5, 1.0],
            emissive: None,
            metallic: None,
            porosity: None,
            roughness: None,
        })
        .collect();
    // Every 5th column cycles through the palette, one in five is `?` (defined by no entry), the
    // rest are empty.
    let row: String = (0..width)
        .map(|x| match x % 5 {
            0 => '#',
            1 => 'A',
            2 => 'z',
            3 => '~',
            _ if x % 10 == 4 => '?',
            _ => '.',
        })
        .collect();
    let doc = VoxelDocument {
        name: Some("wide".to_string()),
        size: [width, 2, 3],
        scale: None,
        generated_from: None,
        budget_exempt: None,
        palette,
        slices: vec![
            vec![row.clone(), row.replace('#', "."), String::new()],
            // A row shorter than `size.x`, and a layer one row short of `size.z`: both read as
            // empty past their end, in both decodes.
            vec![row[..7].to_string(), row.clone()],
        ],
        rim: None,
        lods: Vec::new(),
    };

    let cells = assert_decode_matches("wide", &doc);
    assert_eq!(cells, doc.cell_count());
    assert_eq!(doc.palette_index(0, 0, 0), Some(0), "`#` is entry 0");
    assert_eq!(doc.palette_index(4, 0, 0), None, "`?` names no entry");
    assert_eq!(doc.palette_index(width - 1, 0, 2), None, "an empty row");
    assert_eq!(doc.palette_index(64, 1, 0), None, "past a short row");
}
