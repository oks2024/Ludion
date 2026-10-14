//! The engine's `wrap_ui_text` and `measure_ui_text` (now `ludion_font::layout`) agree with the
//! square-cell implementations they replaced, frozen here as oracles: the same wrapped string and a
//! bit-identical extent on the embedded 8×8 face. The one intended change is the new break after
//! `-`/`—`, so wrapped corpus strings containing either are skipped.

use std::borrow::Cow;

use ludion_engine::ui::{measure_ui_text, wrap_ui_text};
use ludion_font::FontFace;

fn legacy_measure(text: &str, size: f32) -> [f32; 2] {
    if text.is_empty() {
        return [0.0, 0.0];
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let longest = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    [longest as f32 * size, lines.len() as f32 * size]
}

fn legacy_wrap(text: &str, size: f32, wrap_width: f32) -> Cow<'_, str> {
    if !wrap_width.is_finite() || wrap_width <= 0.0 || !size.is_finite() || size <= 0.0 {
        return Cow::Borrowed(text);
    }
    let columns = ((wrap_width / size).floor() as usize).max(1);

    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        let mut current_len = 0usize;
        for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
            let word_len = word.chars().count();
            if current_len > 0 && current_len + 1 + word_len <= columns {
                current.push(' ');
                current.push_str(word);
                current_len += 1 + word_len;
                continue;
            }
            if current_len > 0 {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            if word_len <= columns {
                current.push_str(word);
                current_len = word_len;
            } else {
                let chars: Vec<char> = word.chars().collect();
                let mut chunks = chars.chunks(columns).peekable();
                while let Some(chunk) = chunks.next() {
                    if chunks.peek().is_some() {
                        lines.push(chunk.iter().collect());
                    } else {
                        current = chunk.iter().collect();
                        current_len = chunk.len();
                    }
                }
            }
        }
        lines.push(current);
    }
    Cow::Owned(lines.join("\n"))
}

const CORPUS: [&str; 15] = [
    "",
    "A",
    "HI\nTHERE",
    "THE OLD KEEPER WAITS",
    "THE WATCHMAN FOUND THE MAGNETOHYDRODYNAMICS CORE\nSUPPLIES LOW",
    "MAGNETOHYDRODYNAMICS",
    "SHORT MAGNETOHYDRODYNAMICS TAIL WORDS",
    "A  B   C    D",
    "   LEADING AND TRAILING   ",
    "ONE\n\nTHREE\n",
    "\n\n",
    "lower Case ~ \u{e9} unknown chars",
    "A\u{a0}B NBSP IS NOT A SPACE",
    "     ",
    "WORD WORD WORD WORD WORD WORD WORD WORD",
];
const SIZES: [f32; 4] = [8.0, 10.0, 24.0, 13.37 * 1.5];
const WRAPS: [f32; 9] = [
    0.0,
    -5.0,
    f32::NAN,
    f32::INFINITY,
    1.0,
    100.0,
    336.0,
    77.7,
    1000.0,
];

#[test]
fn wrap_and_measure_match_the_legacy_cell_model() {
    for text in CORPUS {
        for size in SIZES {
            for wrap in WRAPS {
                let context = format!("{text:?} size {size} wrap {wrap}");
                let wrapping = wrap.is_finite() && wrap > 0.0;
                // The one intended change: a wrapped line may now break after a hyphen or em dash.
                if wrapping && text.contains(['-', '\u{2014}']) {
                    continue;
                }
                let wrapped = wrap_ui_text(FontFace::embedded(), text, size, wrap);
                assert_eq!(wrapped, legacy_wrap(text, size, wrap), "{context}");

                let new = measure_ui_text(FontFace::embedded(), &wrapped, size);
                let old = legacy_measure(&wrapped, size);
                assert_eq!(
                    [new[0].to_bits(), new[1].to_bits()],
                    [old[0].to_bits(), old[1].to_bits()],
                    "{context}"
                );
            }
        }
    }
}

#[test]
fn wrap_breaks_after_a_hyphen() {
    // 8 glyphs per line at size 10: "ANTI-HERO" (9) no longer fits on one line, and now splits at the hyphen.
    assert_eq!(
        wrap_ui_text(FontFace::embedded(), "ANTI-HERO", 10.0, 80.0),
        "ANTI-\nHERO"
    );
}
