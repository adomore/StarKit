//! Procedural nightscape foreground: ridgeline + tree silhouettes, plus the
//! truth sky mask (FIXTURES.md, `nightscape-fg` suite).
//!
//! This is the fixture that makes INV-1 testable end to end: the foreground is
//! dark but *not* empty (near-zero luminance with texture), which is exactly the
//! case where a naive tool mistakes foreground highlights for stars.

use rand::RngExt;
use rand_chacha::ChaCha20Rng;

use crate::params::{ForegroundParams, Params};

/// Sky mask values. 8-bit PNG, per FIXTURES.md.
pub const SKY: u8 = 255;
pub const FOREGROUND: u8 = 0;

/// Bilinearly-interpolated value noise on a coarse grid.
///
/// Stored coarse and evaluated on demand: a full-resolution texture plane for
/// the 6144×4096 suite would cost 100 MB for no benefit.
pub struct ValueNoise {
    cols: usize,
    rows: usize,
    cell: f64,
    values: Vec<f32>,
}

impl ValueNoise {
    pub fn new(width: u32, height: u32, cell: f64, rng: &mut ChaCha20Rng) -> Self {
        let cols = (width as f64 / cell).ceil() as usize + 2;
        let rows = (height as f64 / cell).ceil() as usize + 2;
        let values = (0..cols * rows).map(|_| rng.random::<f32>()).collect();
        Self {
            cols,
            rows,
            cell,
            values,
        }
    }

    fn at(&self, cx: usize, cy: usize) -> f64 {
        self.values[cy.min(self.rows - 1) * self.cols + cx.min(self.cols - 1)] as f64
    }

    /// Sample in `[0, 1]` at pixel coordinates.
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        let gx = x / self.cell;
        let gy = y / self.cell;
        let x0 = gx.floor() as usize;
        let y0 = gy.floor() as usize;
        let tx = gx - x0 as f64;
        let ty = gy - y0 as f64;
        // Smoothstep the interpolant so the texture has no visible grid seams.
        let sx = tx * tx * (3.0 - 2.0 * tx);
        let sy = ty * ty * (3.0 - 2.0 * ty);
        let top = self.at(x0, y0) * (1.0 - sx) + self.at(x0 + 1, y0) * sx;
        let bot = self.at(x0, y0 + 1) * (1.0 - sx) + self.at(x0 + 1, y0 + 1) * sx;
        top * (1.0 - sy) + bot * sy
    }
}

pub struct Foreground {
    /// `SKY` / `FOREGROUND` per pixel, row-major.
    pub mask: Vec<u8>,
    pub texture: ValueNoise,
}

/// Sum of sinusoid octaves — the ridgeline height (in pixels from the top) at x.
fn ridge_height(x: f64, width: f64, fg: &ForegroundParams, height: f64, phases: &[f64]) -> f64 {
    let mut h = fg.ridge_base_frac * height;
    let mut amp = fg.ridge_amplitude_frac * height;
    let mut freq = 1.0;
    for &phase in phases {
        h += amp * (std::f64::consts::TAU * freq * x / width + phase).sin();
        amp *= 0.5;
        freq *= 2.0;
    }
    h
}

fn stamp_disc(mask: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, radius: f64) {
    let r = radius.max(0.5);
    let x0 = ((cx - r).floor().max(0.0)) as usize;
    let x1 = ((cx + r).ceil().min(w as f64 - 1.0)) as usize;
    let y0 = ((cy - r).floor().max(0.0)) as usize;
    let y1 = ((cy + r).ceil().min(h as f64 - 1.0)) as usize;
    if cx + r < 0.0 || cy + r < 0.0 || cx - r > w as f64 || cy - r > h as f64 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            if (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2) <= r * r {
                mask[y * w + x] = FOREGROUND;
            }
        }
    }
}

fn stamp_line(mask: &mut [u8], w: usize, h: usize, p0: (f64, f64), p1: (f64, f64), width: f64) {
    let len = ((p1.0 - p0.0).powi(2) + (p1.1 - p0.1).powi(2)).sqrt();
    let steps = (len * 2.0).ceil().max(1.0) as usize;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        stamp_disc(
            mask,
            w,
            h,
            p0.0 + t * (p1.0 - p0.0),
            p0.1 + t * (p1.1 - p0.1),
            width / 2.0,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_branch(
    mask: &mut [u8],
    w: usize,
    h: usize,
    origin: (f64, f64),
    angle: f64,
    len: f64,
    width: f64,
    depth: u32,
    rng: &mut ChaCha20Rng,
) {
    // Screen y grows downward; a branch angle of +π/2 must point at the sky.
    let tip = (origin.0 + len * angle.cos(), origin.1 - len * angle.sin());
    stamp_line(mask, w, h, origin, tip, width);
    if depth == 0 {
        return;
    }
    let n_children = rng.random_range(2..=3u32);
    for _ in 0..n_children {
        let spread: f64 = rng.random_range(0.25..0.75);
        let sign = if rng.random::<bool>() { 1.0 } else { -1.0 };
        let child_len = len * rng.random_range(0.55..0.75);
        draw_branch(
            mask,
            w,
            h,
            tip,
            angle + sign * spread,
            child_len,
            (width * 0.62).max(1.0),
            depth - 1,
            rng,
        );
    }
}

/// Build the foreground mask and texture, or `None` if the suite has no
/// foreground.
pub fn build(p: &Params, rng: &mut ChaCha20Rng) -> Option<Foreground> {
    let fg = p.foreground.as_ref()?;
    let w = p.image.width as usize;
    let h = p.image.height as usize;
    let mut mask = vec![SKY; w * h];

    let phases: Vec<f64> = (0..fg.ridge_octaves)
        .map(|_| rng.random_range(0.0..std::f64::consts::TAU))
        .collect();

    for x in 0..w {
        let ridge = ridge_height(x as f64, w as f64, fg, h as f64, &phases);
        let y0 = ridge.clamp(0.0, h as f64) as usize;
        for y in y0..h {
            mask[y * w + x] = FOREGROUND;
        }
    }

    for _ in 0..fg.tree_count {
        let x: f64 = rng.random_range(0.0..w as f64);
        let base_y = ridge_height(x, w as f64, fg, h as f64, &phases);
        let trunk_len: f64 = rng.random_range(0.08..0.20) * h as f64;
        let trunk_width: f64 = rng.random_range(0.004..0.010) * h as f64;
        draw_branch(
            &mut mask,
            w,
            h,
            (x, base_y),
            std::f64::consts::FRAC_PI_2 + rng.random_range(-0.15..0.15),
            trunk_len,
            trunk_width,
            fg.tree_depth,
            rng,
        );
    }

    let texture = ValueNoise::new(p.image.width, p.image.height, fg.texture_cell_px, rng);
    Some(Foreground { mask, texture })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn value_noise_stays_in_unit_range_and_is_continuous() {
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let n = ValueNoise::new(64, 64, 8.0, &mut rng);
        let mut prev = n.sample(0.0, 10.0);
        for i in 1..600 {
            let v = n.sample(i as f64 * 0.1, 10.0);
            assert!((0.0..=1.0).contains(&v), "value noise out of range: {v}");
            // Bilinear + smoothstep over an 8 px cell cannot jump far in 0.1 px.
            assert!((v - prev).abs() < 0.1, "value noise is discontinuous");
            prev = v;
        }
    }

    #[test]
    fn mask_is_sky_at_the_top_and_foreground_at_the_bottom() {
        let mut p = crate::suite::test_params("fg", 5, 128, 128, 20);
        p.foreground = Some(ForegroundParams {
            ridge_base_frac: 0.7,
            ridge_amplitude_frac: 0.05,
            ridge_octaves: 3,
            tree_count: 2,
            tree_depth: 3,
            level_e: 12.0,
            texture_amplitude: 0.6,
            texture_cell_px: 9.0,
        });
        let mut rng = ChaCha20Rng::seed_from_u64(p.seed);
        let fgnd = build(&p, &mut rng).expect("foreground params present");

        let w = 128usize;
        assert!(
            fgnd.mask[..w].iter().all(|&m| m == SKY),
            "top row must be sky"
        );
        assert!(
            fgnd.mask[w * 127..].iter().all(|&m| m == FOREGROUND),
            "bottom row must be foreground"
        );
        let sky_frac =
            fgnd.mask.iter().filter(|&&m| m == SKY).count() as f64 / fgnd.mask.len() as f64;
        assert!(
            (0.4..0.85).contains(&sky_frac),
            "implausible sky fraction {sky_frac} for a ridge at 0.7 h"
        );
    }

    #[test]
    fn no_foreground_params_means_no_foreground() {
        let p = crate::suite::test_params("plain", 5, 64, 64, 10);
        let mut rng = ChaCha20Rng::seed_from_u64(p.seed);
        assert!(build(&p, &mut rng).is_none());
    }
}
