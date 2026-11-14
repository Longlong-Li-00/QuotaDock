//! The tray icon while monitoring works: two bars, Codex on the left and Claude on the right, each
//! as tall as its weekly remaining percent. A bar's colour is the worse display tone of the windows
//! it shows (weekly, and 5-hour unless hidden), so a low 5-hour turns its bar amber; a reached
//! limit is red. A provider turned off or without a value leaves an empty track.
//!
//! Paused and error keep their own icons (`TrayIconKind::Paused` / `Error`): then the values are
//! not to be trusted, and the icon says so instead. Drawn here, not in the shell, so it is testable.

use super::tray::{DisplayTone, ProviderTrayLine, TrayIconKind, TrayStatusModel};

/// Side of the square RGBA image (Windows scales it to the tray's 16 or 20 px at 100 % / 125 %).
pub const SIZE: usize = 32;
const TOP: f64 = 4.0;
const HEIGHT: f64 = 24.0;
const BARS: [usize; 2] = [6, 18];
const BAR_WIDTH: usize = 8;

const NEUTRAL: [u8; 3] = [0x76, 0x76, 0x76];
const CAUTION: [u8; 3] = [0xB8, 0x9A, 0x00];
const LOW: [u8; 3] = [0xC7, 0x7C, 0x00];
const CRITICAL: [u8; 3] = [0xC4, 0x2B, 0x1C];
/// The track: faint, so it reads on a light and a dark taskbar. Grey, or the bar's warning colour:
/// a low value is a short bar, and the tinted track keeps the warning visible at 16 px.
const TRACK_ALPHA: f64 = 0.32;
/// A value above 0 is never thinner than this (pixels of the 32 px image).
const MIN_FILL: f64 = 2.0;
/// A muted value (paused data, a passed reset): the neutral grey, half strength.
const MUTED_ALPHA: f64 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub remaining: f64,
    pub tone: DisplayTone,
}

/// The bar for one provider's tray line; None leaves its track empty.
pub fn bar_of(line: Option<&ProviderTrayLine>) -> Option<Bar> {
    let line = line?;
    let remaining = line.weekly_remaining?;
    let rank = |t: DisplayTone| match t {
        DisplayTone::Empty => 5,
        DisplayTone::Critical => 4,
        DisplayTone::Low => 3,
        DisplayTone::Caution => 2,
        DisplayTone::Normal => 1,
        DisplayTone::Muted | DisplayTone::Unknown => 0,
    };
    let mut tone = line.weekly_tone;
    if line.show_five_hour && line.five_hour_remaining.is_some() && rank(line.five_hour_tone) > rank(tone) {
        tone = line.five_hour_tone;
    }
    if line.reasons.contains(&"limit_reached") {
        tone = DisplayTone::Critical;
    }
    Some(Bar { remaining, tone })
}

/// The bars for a model, or None when the model's own icon applies (paused, error).
pub fn bars_for(model: &TrayStatusModel) -> Option<[Option<Bar>; 2]> {
    if matches!(model.icon, TrayIconKind::Paused | TrayIconKind::Error) {
        return None;
    }
    let line = |id: &str| model.providers.iter().find(|p| p.provider == id);
    Some([bar_of(line("codex")), bar_of(line("claude"))])
}

/// A 32 x 32 RGBA image (row by row, top first) of the two bars.
pub fn render(bars: [Option<Bar>; 2]) -> Vec<u8> {
    let mut px = vec![0u8; SIZE * SIZE * 4];
    for (bar, &left) in bars.iter().zip(BARS.iter()) {
        // The fill covers [top, TOP + HEIGHT) in pixel rows; its top row is partial.
        let fill = bar.map(|b| {
            let h = b.remaining.clamp(0.0, 100.0) / 100.0 * HEIGHT;
            (if h > 0.0 { h.max(MIN_FILL) } else { 0.0 }, colour(b.tone))
        });
        let track_rgb = match bar.map(|b| b.tone) {
            Some(DisplayTone::Caution) => CAUTION,
            Some(DisplayTone::Low) => LOW,
            Some(DisplayTone::Critical | DisplayTone::Empty) => CRITICAL,
            _ => NEUTRAL,
        };
        for y in 0..SIZE {
            let (y0, y1) = (y as f64, y as f64 + 1.0);
            let track = overlap(y0, y1, TOP, TOP + HEIGHT);
            if track <= 0.0 {
                continue;
            }
            for x in left..left + BAR_WIDTH {
                let corner = corner_cover(x - left, y as f64 - TOP);
                let mut out = blend([0.0; 4], track_rgb, TRACK_ALPHA * track * corner);
                if let Some((h, (rgb, alpha))) = fill {
                    let cover = overlap(y0, y1, TOP + HEIGHT - h, TOP + HEIGHT);
                    out = blend(out, rgb, alpha * cover * corner);
                }
                let i = (y * SIZE + x) * 4;
                px[i..i + 4].copy_from_slice(&out.map(|c| c.round().clamp(0.0, 255.0) as u8));
            }
        }
    }
    px
}

fn colour(tone: DisplayTone) -> ([u8; 3], f64) {
    match tone {
        DisplayTone::Normal => (NEUTRAL, 1.0),
        DisplayTone::Caution => (CAUTION, 1.0),
        DisplayTone::Low => (LOW, 1.0),
        DisplayTone::Critical | DisplayTone::Empty => (CRITICAL, 1.0),
        DisplayTone::Muted | DisplayTone::Unknown => (NEUTRAL, MUTED_ALPHA),
    }
}

/// How much of the pixel row [a, b) lies inside [lo, hi).
fn overlap(a: f64, b: f64, lo: f64, hi: f64) -> f64 {
    (b.min(hi) - a.max(lo)).clamp(0.0, 1.0)
}

/// Rounded corners of radius 2: the outermost corner pixel is left out, its neighbours softened.
fn corner_cover(dx: usize, dy: f64) -> f64 {
    let edge_x = dx == 0 || dx == BAR_WIDTH - 1;
    let edge_y = dy < 1.0 || dy >= HEIGHT - 1.0;
    let near_x = dx == 1 || dx == BAR_WIDTH - 2;
    let near_y = (1.0..2.0).contains(&dy) || (HEIGHT - 2.0..HEIGHT - 1.0).contains(&dy);
    if edge_x && edge_y {
        0.0
    } else if (edge_x && near_y) || (near_x && edge_y) {
        0.6
    } else {
        1.0
    }
}

/// `src` over `dst` (straight alpha, 0-255 channels).
fn blend(dst: [f64; 4], src: [u8; 3], alpha: f64) -> [f64; 4] {
    if alpha <= 0.0 {
        return dst;
    }
    let da = dst[3] / 255.0;
    let oa = alpha + da * (1.0 - alpha);
    let ch = |i: usize| (f64::from(src[i]) * alpha + dst[i] * da * (1.0 - alpha)) / oa;
    [ch(0), ch(1), ch(2), oa * 255.0]
}
