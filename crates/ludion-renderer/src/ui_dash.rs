//! The dash split shared by the UI path stroke and the engine's hit test.

type Pt = [f32; 2];

/// Runs shorter than this (in the path's units) are dropped.
const MIN_RUN: f32 = 1e-4;
/// A vertex closer than this to a run's start or end is not kept: the stroke's join would take its
/// direction from a segment too short to have one.
const MIN_SEGMENT: f32 = 0.05;

/// Cuts a path into dashes: `on` along, `off` skipped, repeating from the first point. `closed` adds the
/// closing segment; the start of a closed path is not joined to the end of its last dash. Each run is its
/// start point, every interior vertex it covers and its end point. An `on` or `off` that is not above 0
/// (or not finite) returns the whole path, closed or not, as one run.
///
/// ```
/// use ludion_renderer::ui_dash_runs;
///
/// let runs = ui_dash_runs(&[[0.0, 0.0], [100.0, 0.0]], false, 10.0, 5.0);
/// assert_eq!(runs.len(), 7);
/// assert_eq!(runs[6], vec![[90.0, 0.0], [100.0, 0.0]]);
/// ```
pub fn ui_dash_runs(points: &[Pt], closed: bool, on: f32, off: f32) -> Vec<Vec<Pt>> {
    let mut path: Vec<Pt> = points.to_vec();
    if closed && path.len() > 1 {
        path.push(path[0]);
    }
    if !(on.is_finite() && off.is_finite() && on > 0.0 && off > 0.0) {
        return vec![path];
    }
    let mut cumulative = Vec::with_capacity(path.len());
    let mut total = 0.0f32;
    cumulative.push(0.0f32);
    for pair in path.windows(2) {
        total += ((pair[1][0] - pair[0][0]).powi(2) + (pair[1][1] - pair[0][1]).powi(2)).sqrt();
        cumulative.push(total);
    }
    let point_at = |s: f32| -> Pt {
        let i = cumulative
            .partition_point(|&c| c <= s)
            .clamp(1, path.len() - 1);
        let (c0, c1) = (cumulative[i - 1], cumulative[i]);
        let t = if c1 > c0 {
            ((s - c0) / (c1 - c0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = (path[i - 1], path[i]);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    };
    let mut runs = Vec::new();
    let period = on + off;
    let mut k = 0u32;
    loop {
        let start = k as f32 * period;
        if start >= total {
            break;
        }
        let end = (start + on).min(total);
        k += 1;
        if end - start < MIN_RUN {
            continue;
        }
        let mut run = vec![point_at(start)];
        for (i, &c) in cumulative.iter().enumerate() {
            if c > start + MIN_SEGMENT && c < end - MIN_SEGMENT {
                run.push(path[i]);
            }
        }
        run.push(point_at(end));
        runs.push(run);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_splits_into_seven_runs_the_last_ending_on_the_path() {
        let runs = ui_dash_runs(&[[0.0, 0.0], [100.0, 0.0]], false, 10.0, 5.0);
        assert_eq!(runs.len(), 7);
        assert_eq!(runs[0], vec![[0.0, 0.0], [10.0, 0.0]]);
        assert_eq!(runs[6], vec![[90.0, 0.0], [100.0, 0.0]]);
    }

    #[test]
    fn a_run_crossing_a_corner_keeps_the_vertex() {
        let runs = ui_dash_runs(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], false, 14.0, 2.0);
        assert_eq!(runs[0], vec![[0.0, 0.0], [10.0, 0.0], [10.0, 4.0]]);
    }

    #[test]
    fn a_closed_square_ends_at_its_first_point() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let runs = ui_dash_runs(&square, true, 10.0, 5.0);
        assert_eq!(runs.len(), 3);
        assert_eq!(*runs.last().unwrap().last().unwrap(), [0.0, 0.0]);
    }

    #[test]
    fn a_non_positive_length_returns_the_whole_path() {
        let line = [[0.0, 0.0], [50.0, 0.0]];
        assert_eq!(ui_dash_runs(&line, false, 5.0, 0.0), vec![line.to_vec()]);
        assert_eq!(ui_dash_runs(&line, false, 0.0, 5.0), vec![line.to_vec()]);
        assert_eq!(
            ui_dash_runs(&line, false, f32::NAN, 5.0),
            vec![line.to_vec()]
        );
    }
}
