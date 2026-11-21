//! [`subtitle_draw`]: the pixel geometry of a [`SubtitleView`] over the lines on air, and what
//! `extract` draws of it (`Plan/dialogue.md`, the subtitle layout rule).

use ludion_font::{GlyphQuad, YAxis, layout};

use super::{FontFace, pixel_scale, resolve_ui_rect};
use crate::components::SubtitleView;
use crate::dialogue::Subtitles;

/// What a [`SubtitleView`] draws: the panel and the text in colour runs, in pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SubtitleDraw {
    /// The panel `[x, y, w, h]`; `Some` only when the view's `background` alpha is above 0.
    pub background_px: Option<[f32; 4]>,
    /// Glyph quads by colour: each subtitle's label in its speaker's colour, the rest in the view's.
    pub runs: Vec<([f32; 4], Vec<GlyphQuad>)>,
}

/// One wrapped row and the subtitle it belongs to.
struct Row {
    subtitle: usize,
    text: String,
}

/// The subtitles of `view`'s channels, wrapped with the [`UiText`](crate::components::UiText) rule and
/// stacked oldest first; the last `max_lines` rows are drawn.
///
/// A label's glyphs are told apart by character: per subtitle, quads in row order (the dropped rows
/// included) are matched against the label's non-space chars, and a space met before the label is
/// used up joins it. Wrapping only drops or collapses spaces, so the match survives a break inside
/// the label. An empty result draws nothing, panel included.
pub(crate) fn subtitle_draw(
    face: &FontFace,
    view: &SubtitleView,
    subtitles: &Subtitles,
    viewport: (u32, u32),
) -> SubtitleDraw {
    let shown: Vec<_> = subtitles
        .iter()
        .filter(|subtitle| view.channels.contains(&subtitle.channel))
        .collect();
    let mut rows: Vec<Row> = Vec::new();
    let mut label_chars: Vec<Vec<char>> = Vec::with_capacity(shown.len());
    for (index, subtitle) in shown.iter().enumerate() {
        let prefix = if view.speaker_label {
            format!("{}: ", subtitle.speaker)
        } else {
            String::new()
        };
        label_chars.push(prefix.chars().filter(|ch| !ch.is_whitespace()).collect());
        let laid = layout(
            face,
            &format!("{prefix}{}", subtitle.text),
            view.size,
            view.wrap_width,
        );
        rows.extend((0..laid.line_count()).map(|line| Row {
            subtitle: index,
            text: laid.line_text(line),
        }));
    }
    let dropped = match view.max_lines {
        0 => 0,
        limit => rows.len().saturating_sub(limit as usize),
    };
    if rows.len() == dropped {
        return SubtitleDraw {
            background_px: None,
            runs: Vec::new(),
        };
    }

    let kept = &rows[dropped..];
    let widest = kept
        .iter()
        .map(|row| layout(face, &row.text, view.size, 0.0).extent()[0])
        .fold(0.0_f32, f32::max);
    let extent = [
        widest + 2.0 * view.padding,
        kept.len() as f32 * view.size + 2.0 * view.padding,
    ];
    let rect = resolve_ui_rect(view.anchor, view.offset, extent, viewport);
    let scale = pixel_scale(viewport);
    let cell_px = view.size * scale;
    let origin = [
        rect[0] + view.padding * scale,
        rect[1] + view.padding * scale,
    ];

    let mut labels: Vec<([f32; 4], Vec<GlyphQuad>)> = Vec::new();
    let mut body: Vec<GlyphQuad> = Vec::new();
    // How many label chars each subtitle has matched so far.
    let mut matched = vec![0usize; shown.len()];
    for (index, row) in rows.iter().enumerate() {
        let visible = index.checked_sub(dropped);
        let origin = [origin[0], origin[1] + visible.unwrap_or(0) as f32 * cell_px];
        let quads = layout(face, &row.text, view.size, 0.0).quads(origin, cell_px, YAxis::Down);
        let (color, chars) = (shown[row.subtitle].color, &label_chars[row.subtitle]);
        for quad in quads {
            let cursor = &mut matched[row.subtitle];
            let in_label = *cursor < chars.len()
                && (quad.ch.is_whitespace() || {
                    let hit = chars[*cursor] == quad.ch;
                    *cursor += usize::from(hit);
                    hit
                });
            if visible.is_none() {
                continue;
            }
            if in_label {
                match labels.last_mut() {
                    Some((last, run)) if *last == color => run.push(quad),
                    _ => labels.push((color, vec![quad])),
                }
            } else {
                body.push(quad);
            }
        }
    }
    let mut runs = labels;
    if !body.is_empty() {
        runs.push((view.color, body));
    }
    SubtitleDraw {
        background_px: (view.background[3] > 0.0).then_some(rect),
        runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogue::{Channel, LineHandle, Subtitle};
    use ludion_core::ids::EntityId;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn subtitle(channel: Channel, speaker: &str, text: &str) -> Subtitle {
        Subtitle {
            handle: LineHandle(EntityId::new(0, 0)),
            line: String::new(),
            speaker_id: speaker.to_owned(),
            speaker: speaker.to_owned(),
            color: [1.0, 0.5, 0.0, 1.0],
            text: text.to_owned(),
            channel,
            portrait: None,
            start_frame: 0,
            end_frame: 1,
        }
    }

    fn view() -> SubtitleView {
        SubtitleView {
            size: 10.0,
            wrap_width: 0.0,
            padding: 0.0,
            ..SubtitleView::default()
        }
    }

    fn draw(view: &SubtitleView, subtitles: Vec<Subtitle>) -> SubtitleDraw {
        subtitle_draw(
            FontFace::embedded(),
            view,
            &Subtitles::from_active(subtitles),
            VIEWPORT,
        )
    }

    fn quads(draw: &SubtitleDraw) -> usize {
        draw.runs.iter().map(|(_, quads)| quads.len()).sum()
    }

    #[test]
    fn long_text_wraps_to_two_rows() {
        let view = SubtitleView {
            wrap_width: 130.0,
            ..view()
        };
        let one = draw(
            &view,
            vec![subtitle(Channel::Radio, "A", "aaaa bbbb cccc dddd")],
        );
        let panel = one.background_px.expect("a panel");
        assert_eq!(panel[3], 20.0, "two rows of size 10: {panel:?}");
    }

    #[test]
    fn max_lines_drops_the_oldest_rows() {
        let view = SubtitleView {
            max_lines: 1,
            ..view()
        };
        let both = draw(
            &view,
            vec![
                subtitle(Channel::Radio, "A", "first"),
                subtitle(Channel::Ui, "B", "second"),
            ],
        );
        // Only B's row survives: "B: second" is 9 quads, the space included.
        assert_eq!(quads(&both), 9);
    }

    #[test]
    fn label_run_is_the_speakers_glyphs() {
        let mut one = subtitle(Channel::Radio, "Name", "hello");
        one.color = [1.0, 0.0, 0.0, 1.0];
        let out = draw(&view(), vec![one]);
        assert_eq!(out.runs.len(), 2);
        assert_eq!(out.runs[0].0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(out.runs[0].1.len(), "Name:".chars().count());
        assert_eq!(out.runs[1].0, view().color);
        // The space after the colon draws a quad, and sits in the body run.
        assert_eq!(out.runs[1].1.len(), " hello".chars().count());
    }

    #[test]
    fn a_label_broken_across_rows_stays_in_the_speakers_colour() {
        let view = SubtitleView {
            wrap_width: 80.0,
            ..view()
        };
        let mut one = subtitle(Channel::Radio, "Guard Captain", "go");
        one.color = [1.0, 0.0, 0.0, 1.0];
        let out = draw(&view, vec![one]);
        let label: String = out.runs[0].1.iter().map(|q| q.ch).collect();
        assert_eq!(out.runs[0].0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            label.chars().filter(|ch| !ch.is_whitespace()).count(),
            "GuardCaptain:".len()
        );
        let panel = out.background_px.expect("a panel");
        assert!(panel[3] >= 20.0, "the label wrapped: {panel:?}");
        let (color, body) = out.runs.last().unwrap();
        assert_eq!(*color, view.color);
        assert_eq!(body.iter().filter(|q| q.ch == 'g').count(), 1);
        assert!(!label.contains('g'));
    }

    #[test]
    fn no_label_is_one_run() {
        let view = SubtitleView {
            speaker_label: false,
            ..view()
        };
        let out = draw(&view, vec![subtitle(Channel::Radio, "Name", "hello")]);
        assert_eq!(out.runs.len(), 1);
        assert_eq!(quads(&out), 5);
    }

    #[test]
    fn channels_filter_the_lines() {
        let view = SubtitleView {
            channels: vec![Channel::Ui],
            speaker_label: false,
            ..view()
        };
        let out = draw(
            &view,
            vec![
                subtitle(Channel::Radio, "A", "radio"),
                subtitle(Channel::Ui, "B", "ui"),
            ],
        );
        assert_eq!(quads(&out), 2);
    }

    #[test]
    fn nothing_on_air_draws_no_panel() {
        let out = draw(&view(), Vec::new());
        assert_eq!(out.background_px, None);
        assert!(out.runs.is_empty());
        let shown = draw(&view(), vec![subtitle(Channel::Radio, "A", "x")]);
        assert!(shown.background_px.is_some());
    }
}
