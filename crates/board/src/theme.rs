//! Pure theme mapping: DeckStatus → RGB / brightness / fx
//! Ported from packages/host/src/board/theme.ts

use agent_deck_protocol::{DeckStatus, LedFx, Risk, URGENCY_FULL_WAIT_MS, WORKING_LONG_MS};

#[derive(Debug, Clone)]
pub struct ThemeInput {
    pub status: DeckStatus,
    pub risk: Option<Risk>,
    pub waiting_since: Option<u64>,
    pub now: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeOutput {
    pub rgb: Option<[u8; 3]>,
    pub br: u8,
    pub fx: LedFx,
}

#[derive(Debug, Clone)]
pub struct ThemePalette {
    pub off: &'static str,
    pub idle: &'static str,
    pub working: &'static str,
    pub waiting: &'static str,
    pub done: &'static str,
    pub error: &'static str,
}

pub const CODEX_THEME: ThemePalette = ThemePalette {
    off: "#000000",
    idle: "#FFFFFF",
    working: "#304FFE",
    waiting: "#FF6D00",
    done: "#00FF4C",
    error: "#FF0033",
};

pub fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub fn hex_to_rgb(hex: &str) -> [u8; 3] {
    let h = hex.trim().trim_start_matches('#');
    let full = if h.len() == 3 {
        let chars: Vec<char> = h.chars().collect();
        format!(
            "{}{}{}{}{}{}",
            chars[0], chars[0], chars[1], chars[1], chars[2], chars[2]
        )
    } else {
        h.to_string()
    };
    assert_eq!(full.len(), 6, "invalid hex: {hex}");
    let r = u8::from_str_radix(&full[0..2], 16).expect("r");
    let g = u8::from_str_radix(&full[2..4], 16).expect("g");
    let b = u8::from_str_radix(&full[4..6], 16).expect("b");
    [r, g, b]
}

pub fn lerp_hex(a: &str, b: &str, t: f64) -> [u8; 3] {
    let ca = hex_to_rgb(a);
    let cb = hex_to_rgb(b);
    [
        lerp(ca[0] as f64, cb[0] as f64, t).round() as u8,
        lerp(ca[1] as f64, cb[1] as f64, t).round() as u8,
        lerp(ca[2] as f64, cb[2] as f64, t).round() as u8,
    ]
}

pub fn paint(input: &ThemeInput, palette: &ThemePalette) -> ThemeOutput {
    match input.status {
        DeckStatus::Off => ThemeOutput {
            rgb: None,
            br: 0,
            fx: LedFx::Solid,
        },
        DeckStatus::Idle => ThemeOutput {
            rgb: Some(hex_to_rgb(palette.idle)),
            br: 60,
            fx: LedFx::Solid,
        },
        DeckStatus::Done => ThemeOutput {
            rgb: Some(hex_to_rgb(palette.done)),
            br: 255,
            fx: LedFx::Solid,
        },
        DeckStatus::Error => ThemeOutput {
            rgb: Some(hex_to_rgb(palette.error)),
            br: 255,
            fx: LedFx::Solid,
        },
        DeckStatus::Working => {
            if let Some(since) = input.waiting_since {
                let age_sec = (input.now.saturating_sub(since)) as f64 / 1000.0;
                let long_run = clamp01(age_sec / (WORKING_LONG_MS as f64 / 1000.0));
                let rgb = lerp_hex(palette.working, "#7B1FA2", long_run);
                ThemeOutput {
                    rgb: Some(rgb),
                    br: 180,
                    fx: LedFx::Breathe,
                }
            } else {
                ThemeOutput {
                    rgb: Some(hex_to_rgb(palette.working)),
                    br: 180,
                    fx: LedFx::Breathe,
                }
            }
        }
        DeckStatus::Waiting => {
            let age_sec = match input.waiting_since {
                Some(since) => (input.now.saturating_sub(since)) as f64 / 1000.0,
                None => 0.0,
            };
            let time_urgency = clamp01(age_sec / (URGENCY_FULL_WAIT_MS as f64 / 1000.0));
            let risk_boost = input.risk.map(|r| r.boost()).unwrap_or(0.0);
            let u = time_urgency.max(risk_boost);
            let rgb = lerp_hex("#FFB074", "#FF2200", u);
            let br = lerp(80.0, 255.0, u).round() as u8;
            let fx = if u < 0.33 {
                LedFx::Solid
            } else if u < 0.66 {
                LedFx::BlinkSlow
            } else {
                LedFx::BlinkFast
            };
            ThemeOutput {
                rgb: Some(rgb),
                br,
                fx,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_is_blue_breathe() {
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Working,
                risk: None,
                waiting_since: None,
                now: 0,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::Breathe);
        let rgb = out.rgb.unwrap();
        assert!(rgb[0] < 120);
        assert!(rgb[2] > 200);
    }

    #[test]
    fn done_is_green_solid() {
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Done,
                risk: None,
                waiting_since: None,
                now: 0,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::Solid);
        let rgb = out.rgb.unwrap();
        assert!(rgb[1] > 200);
    }

    #[test]
    fn error_is_red_solid() {
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Error,
                risk: None,
                waiting_since: None,
                now: 0,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::Solid);
        let rgb = out.rgb.unwrap();
        assert!(rgb[0] > 200);
        assert!(rgb[1] < 80);
    }

    #[test]
    fn waiting_low_risk_starts_solid() {
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Waiting,
                risk: Some(Risk::Low),
                waiting_since: Some(1000),
                now: 1000,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::Solid);
    }

    #[test]
    fn waiting_high_risk_starts_blink_slow() {
        // high risk boost = 0.5 → blink_slow
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Waiting,
                risk: Some(Risk::High),
                waiting_since: Some(1000),
                now: 1000,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::BlinkSlow);
    }

    #[test]
    fn waiting_urgency_full_after_2min() {
        let out = paint(
            &ThemeInput {
                status: DeckStatus::Waiting,
                risk: Some(Risk::Low),
                waiting_since: Some(0),
                now: 3 * 60 * 1000,
            },
            &CODEX_THEME,
        );
        assert_eq!(out.fx, LedFx::BlinkFast);
        let rgb = out.rgb.unwrap();
        assert!(rgb[0] > 240);
        assert!(rgb[1] < 80);
    }
}
