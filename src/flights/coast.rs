//! The coastlines the map is drawn with: Natural Earth's at three scales,
//! written by `scripts/build-coastline-data.py` (see its docstring for the
//! format). ratatui's own world map is 5,125 dots, too coarse to zoom into, so
//! these are drawn as lines, at the scale the view needs.

use std::sync::OnceLock;

use ratatui::style::Color;
use ratatui::widgets::canvas::{Line as CanvasLine, Painter, Shape};

/// Committed rather than built by `build.rs`, since building it downloads
/// Natural Earth's coastlines.
static DATA: &[u8] = include_bytes!("../../data/coastlines.bin");

const MAGIC: &[u8; 4] = b"CST1";

/// A map view or box: `(lon_min, lon_max, lat_min, lat_max)`.
pub type Bounds = (f64, f64, f64, f64);

/// One coastline, as `(lon, lat)` points, with the box around it so lines off
/// the view can be skipped whole.
pub struct Coastline {
    pub points: Vec<(f64, f64)>,
    pub bbox: Bounds,
}

/// Every scale's coastlines, coarsest (1:110m) first, parsed on first use.
fn scales() -> &'static [Vec<Coastline>] {
    static SCALES: OnceLock<Vec<Vec<Coastline>>> = OnceLock::new();
    SCALES.get_or_init(|| parse(DATA))
}

fn parse(data: &[u8]) -> Vec<Vec<Coastline>> {
    assert_eq!(
        &data[..4],
        MAGIC,
        "coastlines.bin has the wrong magic number"
    );
    let mut pos = 5;
    let mut take = |n: usize| {
        let bytes = &data[pos..pos + n];
        pos += n;
        bytes
    };
    let u32_at = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes"));
    let i32_at = |b: &[u8]| i32::from_le_bytes(b.try_into().expect("4 bytes"));
    let i16_at = |b: &[u8]| i16::from_le_bytes(b.try_into().expect("2 bytes"));
    (0..data[4])
        .map(|_| {
            let per_degree = f64::from(u32_at(take(4)));
            let lines = u32_at(take(4));
            (0..lines)
                .map(|_| {
                    let n = u32_at(take(4)) as usize;
                    let (mut x, mut y) = (i32_at(take(4)), i32_at(take(4)));
                    let mut points = Vec::with_capacity(n);
                    points.push((f64::from(x) / per_degree, f64::from(y) / per_degree));
                    for _ in 1..n {
                        x += i32::from(i16_at(take(2)));
                        y += i32::from(i16_at(take(2)));
                        points.push((f64::from(x) / per_degree, f64::from(y) / per_degree));
                    }
                    let bbox = points.iter().fold(
                        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
                        |(x0, x1, y0, y1), &(x, y)| (x0.min(x), x1.max(x), y0.min(y), y1.max(y)),
                    );
                    Coastline { points, bbox }
                })
                .collect()
        })
        .collect()
}

/// Degrees of latitude per braille dot above which the coarsest scale is
/// enough, and above which the middle one is; finer views get 1:10m. (A
/// terminal row is four braille dots tall.)
const COARSE_ABOVE: f64 = 0.3;
const MIDDLE_ABOVE: f64 = 0.05;

/// The coastlines detailed enough for a view `lat_span` degrees tall drawn in
/// `rows` terminal rows: no finer than its dots can show, which keeps drawing
/// the whole world as quick as ratatui's own map.
pub fn for_view(lat_span: f64, rows: u16) -> &'static [Coastline] {
    let per_dot = lat_span / (f64::from(rows.max(1)) * 4.0);
    let scales = scales();
    let i = if per_dot > COARSE_ABOVE {
        0
    } else if per_dot > MIDDLE_ABOVE {
        1
    } else {
        2
    };
    &scales[i.min(scales.len() - 1)]
}

/// The part of the line from `a` to `b` inside `bounds` (Liang-Barsky), since
/// the canvas drops a line with either end outside its bounds, which a zoomed
/// view would otherwise do to every line running off its edge.
pub fn clip(
    a: (f64, f64),
    b: (f64, f64),
    (x0, x1, y0, y1): Bounds,
) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [
        (-dx, a.0 - x0),
        (dx, x1 - a.0),
        (-dy, a.1 - y0),
        (dy, y1 - a.1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((
        (a.0 + t0 * dx, a.1 + t0 * dy),
        (a.0 + t1 * dx, a.1 + t1 * dy),
    ))
}

/// Coastlines drawn as lines clipped to `bounds`, skipping any wholly off it.
pub struct Coast<'a> {
    pub lines: &'a [Coastline],
    pub bounds: Bounds,
    pub color: Color,
}

impl Shape for Coast<'_> {
    fn draw(&self, painter: &mut Painter) {
        let (x0, x1, y0, y1) = self.bounds;
        for line in self.lines {
            let (bx0, bx1, by0, by1) = line.bbox;
            if bx1 < x0 || bx0 > x1 || by1 < y0 || by0 > y1 {
                continue;
            }
            for w in line.points.windows(2) {
                if let Some(((ax, ay), (bx, by))) = clip(w[0], w[1], self.bounds) {
                    CanvasLine::new(ax, ay, bx, by, self.color).draw(painter);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_scales_coarsest_first() {
        let scales = scales();
        assert_eq!(scales.len(), 3);
        let points: Vec<usize> = scales
            .iter()
            .map(|s| s.iter().map(|l| l.points.len()).sum())
            .collect();
        assert!(points.windows(2).all(|w| w[0] < w[1]), "{points:?}");
        assert!(points[2] > 300_000, "{points:?}");
        for line in scales.iter().flatten() {
            let (x0, x1, y0, y1) = line.bbox;
            assert!(x0 >= -180.0001 && x1 <= 180.0001 && y0 >= -90.0001 && y1 <= 90.0001);
        }
    }

    #[test]
    fn detail_follows_the_zoom() {
        let scales = scales();
        let pick = |span, rows| for_view(span, rows).as_ptr();
        assert_eq!(pick(180.0, 28), scales[0].as_ptr());
        assert_eq!(pick(30.0, 28), scales[1].as_ptr());
        assert_eq!(pick(3.0, 28), scales[2].as_ptr());
    }

    #[test]
    fn lines_are_clipped_to_the_view() {
        let b = (0.0, 10.0, 0.0, 10.0);
        assert_eq!(
            clip((1.0, 1.0), (9.0, 9.0), b),
            Some(((1.0, 1.0), (9.0, 9.0)))
        );
        let ((ax, ay), (bx, by)) = clip((-10.0, 5.0), (5.0, 5.0), b).unwrap();
        assert_eq!((ax, ay, bx, by), (0.0, 5.0, 5.0, 5.0));
        assert!(clip((-5.0, 5.0), (15.0, 5.0), b).is_some());
        assert_eq!(clip((-5.0, -5.0), (-1.0, 20.0), b), None);
    }
}
