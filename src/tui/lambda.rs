//! The Logos λ, drawn with quadrant block characters (`▘▝▖▗▌▐▀▄…`): each
//! cell holds a 2×2 grid of pixels, so a glyph a few rows tall still has
//! clean diagonals. The shape is two straight strokes with flat ends, sampled
//! per pixel, so it scales to any size and can spin about its vertical axis.

use std::f32::consts::TAU;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

/// A straight stroke in glyph space, `[-1, 1]²` with y down. Ends past
/// the box are cut flat by it.
type Stroke = ((f32, f32), (f32, f32), f32);

/// (start, end, width) of each stroke.
const STROKES: [Stroke; 2] = [
    // The long stroke, top left to bottom right.
    ((-0.62, -1.2), (0.80, 1.2), 0.34),
    // The leg, from the middle of the long stroke to bottom left.
    ((0.03, -0.1), (-0.76, 1.2), 0.27),
];

/// Indexed by quadrant bits: top left 1, top right 2, bottom left 4, bottom right 8.
const QUADRANTS: [&str; 16] = [
    " ", "▘", "▝", "▀", "▖", "▌", "▞", "▛", "▗", "▚", "▐", "▜", "▄", "▙", "▟", "█",
];

/// The Logos warm gradient: yellow → orange → pink.
const GRADIENT: [(f32, f32, f32); 3] = [(247., 181., 0.), (255., 138., 76.), (255., 94., 142.)];

/// One full turn while a job runs.
pub(crate) const SPIN_PERIOD: Duration = Duration::from_millis(1600);

/// A λ filling `area` (centred, keeping its proportions).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Lambda {
    /// Rotation about the vertical axis, in radians; 0 faces the viewer.
    pub(crate) angle: f32,
    /// How much of the glyph has been drawn, 0..=1, sweeping from top left.
    /// 1 (or more) shows all of it.
    pub(crate) reveal: f32,
    /// Position of a bright band sweeping across the glyph, 0..=1; `None`
    /// for no band.
    pub(crate) shine: Option<f32>,
}

impl Lambda {
    pub(crate) fn still() -> Self {
        Self {
            angle: 0.0,
            reveal: 1.0,
            shine: None,
        }
    }

    /// Spinning, `elapsed` into the spin.
    pub(crate) fn spinning(elapsed: Duration) -> Self {
        let turns = elapsed.as_secs_f32() / SPIN_PERIOD.as_secs_f32();
        Self {
            angle: turns.fract() * TAU,
            ..Self::still()
        }
    }
}

fn in_glyph(x: f32, y: f32) -> bool {
    if !(-1.0..=1.0).contains(&y) {
        return false;
    }
    STROKES.iter().any(|&((x0, y0), (x1, y1), width)| {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        let u = ((x - x0) * dx + (y - y0) * dy) / len2;
        if !(0.0..=1.0).contains(&u) {
            return false;
        }
        let (px, py) = (x0 + u * dx - x, y0 + u * dy - y);
        px * px + py * py <= width * width / 4.0
    })
}

fn gradient(t: f32) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0) * (GRADIENT.len() - 1) as f32;
    let i = (t as usize).min(GRADIENT.len() - 2);
    let f = t - i as f32;
    let (a, b) = (GRADIENT[i], GRADIENT[i + 1]);
    (
        a.0 + (b.0 - a.0) * f,
        a.1 + (b.1 - a.1) * f,
        a.2 + (b.2 - a.2) * f,
    )
}

impl Widget for Lambda {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        // A cell is about twice as tall as it is wide; measure in cell
        // widths, so the glyph box is `side` wide and `side` tall on screen.
        let width = f32::from(area.width);
        let height = f32::from(area.height) * 2.0;
        let side = width.min(height);
        let (cx, cy) = (width / 2.0, height / 2.0);
        let half = side / 2.0;

        // Seen edge-on the glyph is a sliver, not nothing; the back face is
        // the mirror image, as on a spinning coin.
        let cos = self.angle.cos();
        let squash = if cos.abs() < 0.06 {
            0.06f32.copysign(cos)
        } else {
            cos
        };
        let light = 0.5 + 0.5 * cos.abs();

        // 2×2 pixels per cell, each sampled 3×3 for smooth edges.
        const SUB: usize = 3;
        for row in 0..area.height {
            for col in 0..area.width {
                let mut bits = 0;
                for q in 0..4 {
                    let (qx, qy) = ((q % 2) as f32, (q / 2) as f32);
                    let mut hits = 0;
                    for s in 0..SUB * SUB {
                        let sx = ((s % SUB) as f32 + 0.5) / SUB as f32;
                        let sy = ((s / SUB) as f32 + 0.5) / SUB as f32;
                        let ux = f32::from(col) + (qx + sx) * 0.5;
                        let uy = (f32::from(row) + (qy + sy) * 0.5) * 2.0;
                        let gx = (ux - cx) / half / squash;
                        let gy = (uy - cy) / half;
                        if (gx + gy + 2.0) / 4.0 <= self.reveal && in_glyph(gx, gy) {
                            hits += 1;
                        }
                    }
                    if hits * 2 > SUB * SUB {
                        bits |= 1 << q;
                    }
                }
                if bits == 0 {
                    continue;
                }
                // Colour by where the cell sits on screen, so the gradient
                // stays put while the shape turns through it.
                let gx = (f32::from(col) + 0.5 - cx) / half;
                let gy = ((f32::from(row) + 0.5) * 2.0 - cy) / half;
                let diag = ((gx + gy + 2.0) / 4.0).clamp(0.0, 1.0);
                let (mut r, mut g, mut b) = gradient(diag);
                let mut boost = 0.0;
                if let Some(shine) = self.shine {
                    boost = (1.0 - (diag - shine).abs() / 0.12).max(0.0) * 0.75;
                }
                if self.reveal < 1.0 {
                    // The leading edge of the reveal glows white.
                    boost = boost.max((1.0 - (self.reveal - diag) / 0.08).clamp(0.0, 1.0));
                }
                r = (r + (255.0 - r) * boost) * light;
                g = (g + (255.0 - g) * boost) * light;
                b = (b + (255.0 - b) * boost) * light;
                if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                    cell.set_symbol(QUADRANTS[bits])
                        .set_fg(Color::Rgb(r as u8, g as u8, b as u8));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(lambda: Lambda, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        lambda.render(area, &mut buf);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn still_lambda_is_recognisable() {
        let rows = draw(Lambda::still(), 24, 12);
        let filled = |row: &str| row.chars().filter(|c| *c != ' ').count();
        assert!(rows.iter().all(|row| filled(row) > 0), "{rows:#?}");
        // The top has only the long stroke; the bottom has both feet,
        // apart: one near each side.
        let bottom = &rows[11];
        let left = bottom.find(|c| c != ' ').unwrap();
        let right = bottom.rfind(|c| c != ' ').unwrap();
        assert!(bottom[left..right].contains(' '), "{rows:#?}");
        assert!(filled(&rows[0]) < filled(bottom), "{rows:#?}");
    }

    #[test]
    fn half_a_turn_mirrors_it_and_a_quarter_is_a_sliver() {
        let front = draw(Lambda::still(), 24, 12);
        let back = draw(
            Lambda {
                angle: std::f32::consts::PI,
                ..Lambda::still()
            },
            24,
            12,
        );
        // The long stroke starts top left facing us and top right from behind.
        assert!(front[0].find('█') < Some(12), "{front:#?}");
        assert!(back[0].find(|c| c != ' ') > Some(11), "{back:#?}");
        let edge_on = draw(
            Lambda {
                angle: std::f32::consts::FRAC_PI_2,
                ..Lambda::still()
            },
            24,
            12,
        );
        assert!(edge_on
            .iter()
            .all(|row| row.chars().filter(|c| *c != ' ').count() <= 2));
    }

    #[test]
    fn reveal_draws_from_the_top_left() {
        let rows = draw(
            Lambda {
                reveal: 0.3,
                ..Lambda::still()
            },
            24,
            12,
        );
        assert!(rows[0].contains(|c| c != ' '));
        assert!(rows[11].chars().all(|c| c == ' '));
    }

    #[test]
    fn tiny_or_empty_areas_do_not_panic() {
        draw(Lambda::still(), 1, 1);
        draw(Lambda::spinning(Duration::from_millis(400)), 3, 2);
        draw(Lambda::still(), 0, 0);
    }
}
