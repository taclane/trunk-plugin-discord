//! Which tones a Tones feed takes: its kinds of tone, and tones typed in. The same rules as the
//! notify plugin's "A tone is heard" (its `Pattern`, `ToneSpec`, `tolerance` and `tone_is`), so a
//! tone typed into one is matched the same in the other.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use trunk_recorder_plugin::TonePattern;

/// A kind of tone, as the call JSON's `tone_patterns` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    /// Two-tone page
    TwoTone,
    /// Long tone
    Long,
    /// Hi-low warble
    HiLow,
    /// Pulsed tone
    Pulsed,
    /// DTMF digits
    Digits,
    /// KNOX digits
    Knox,
    /// Morse ID
    Morse,
    /// Tone sequence
    Sequence,
    /// Beep
    Beep,
}

impl Pattern {
    pub fn name(self) -> &'static str {
        match self {
            Pattern::TwoTone => "two_tone",
            Pattern::Long => "long",
            Pattern::HiLow => "hi_low",
            Pattern::Pulsed => "pulsed",
            Pattern::Digits => "digits",
            Pattern::Knox => "knox",
            Pattern::Morse => "morse",
            Pattern::Sequence => "sequence",
            Pattern::Beep => "beep",
        }
    }

    /// For people: "two-tone page".
    pub fn words(self) -> &'static str {
        match self {
            Pattern::TwoTone => "two-tone page",
            Pattern::Long => "long tone",
            Pattern::HiLow => "hi-low warble",
            Pattern::Pulsed => "pulsed tone",
            Pattern::Digits => "DTMF digits",
            Pattern::Knox => "KNOX digits",
            Pattern::Morse => "Morse ID",
            Pattern::Sequence => "tone sequence",
            Pattern::Beep => "beep",
        }
    }
}

/// A tone typed in, as a system's signal has it (its pattern, tones or digits; its lengths are for
/// playing it, not matched).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ToneSpec {
    pub pattern: Pattern,
    /// Hz, in order (two-tone: A then B).
    pub tones_hz: Vec<f64>,
    /// Digits; Morse: the text.
    pub digits: String,
    /// The digits are KNOX tones (else DTMF).
    pub knox: bool,
    pub lengths_s: Vec<f64>,
    /// How much further than usual ([`tolerance`]) a heard tone may be, % of it.
    pub extra_tolerance_pct: f64,
}

impl Default for ToneSpec {
    fn default() -> Self {
        ToneSpec { pattern: Pattern::TwoTone, tones_hz: Vec::new(), digits: String::new(), knox: false, lengths_s: Vec::new(), extra_tolerance_pct: 0.0 }
    }
}

/// How far a heard tone may be from `hz` and still be it, Hz: as TRP matches signals. Measured in
/// audio, 1.5 % (what selective-calling decoders must accept); from Phase 2's tone frames, which
/// name a tone on a 31.25 Hz grid, 15.7 Hz and 0.5 %.
pub fn tolerance(hz: f64, from_frame: bool) -> f64 {
    if from_frame {
        15.7 + 0.005 * hz
    } else {
        0.015 * hz
    }
}

/// A heard tone's kind: its pattern, digit strings told apart by their keypad (`digits` DTMF,
/// `knox` KNOX).
pub fn kind(p: &TonePattern) -> &str {
    if p.pattern == "digits" && p.knox {
        "knox"
    } else {
        &p.pattern
    }
}

/// The heard tone `p` is the one typed in, `t`.
pub fn tone_is(t: &ToneSpec, p: &TonePattern) -> bool {
    // (KNOX digits are the pattern `digits` with `knox`, as TRP has them; `knox` is the same.)
    let knox = t.knox || t.pattern == Pattern::Knox;
    if t.pattern.name() != p.pattern && t.pattern.name() != kind(p) {
        return false;
    }
    match t.pattern {
        Pattern::Digits | Pattern::Knox => knox == p.knox && t.digits == p.digits,
        Pattern::Morse => t.digits.eq_ignore_ascii_case(&p.digits),
        _ => {
            let frame = p.origin == "frame";
            let tol = |f: f64| tolerance(f, frame) + f * t.extra_tolerance_pct / 100.0;
            t.tones_hz.len() == p.tones.len() && t.tones_hz.iter().zip(&p.tones).all(|(&want, &got)| (want - got).abs() <= tol(want))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(pattern: &str, tones: &[f64]) -> TonePattern {
        TonePattern { pattern: pattern.into(), tones: tones.to_vec(), origin: "audio".into(), ..Default::default() }
    }

    #[test]
    fn a_typed_tone_matches_within_its_tolerance() {
        let t = ToneSpec { tones_hz: vec![781.25, 875.0], ..Default::default() };
        assert!(tone_is(&t, &heard("two_tone", &[785.0, 870.0])));
        assert!(!tone_is(&t, &heard("two_tone", &[800.0, 875.0])));
        assert!(!tone_is(&t, &heard("long", &[781.25])));
        let k = ToneSpec { pattern: Pattern::Knox, digits: "072864".into(), ..Default::default() };
        assert!(tone_is(&k, &TonePattern { pattern: "digits".into(), digits: "072864".into(), knox: true, ..Default::default() }));
        assert!(!tone_is(&k, &TonePattern { pattern: "digits".into(), digits: "072864".into(), knox: false, ..Default::default() }));
    }
}
