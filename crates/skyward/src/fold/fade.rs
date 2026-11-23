//! Separate a lit fold into two short ramps. The second ramp begins only
//! when the new panel's first frame can be shown.

const OUT_MS: u64 = 112;
const IN_MS: u64 = 160;
const STEP_MS: u64 = 16;

#[derive(Default)]
pub(crate) enum PanelFade {
    #[default]
    Steady,
    Out { folded: bool, since: u64, from: f32 },
    FirstFrame,
    In { since: u64, from: f32 },
}

fn progress(elapsed: u64, span: u64) -> f32 {
    let t = elapsed.min(span) as f32 / span as f32;
    t * t * (3.0 - 2.0 * t)
}

impl PanelFade {
    /// Returns true when the outgoing panel should stop accepting touches.
    /// A reversal before the handoff brings its light back from where it was.
    pub(crate) fn request(&mut self, current: bool, wanted: bool, now: u64) -> bool {
        match (current == wanted, &*self) {
            (false, Self::Out { .. }) => false,
            (false, _) => {
                *self = Self::Out { folded: wanted, since: now, from: self.level(now) };
                true
            }
            (true, Self::Out { .. }) => {
                *self = Self::In { since: now, from: self.level(now) };
                false
            }
            (true, _) => false,
        }
    }

    pub(crate) fn leaving(&self) -> bool {
        matches!(self, Self::Out { .. })
    }

    /// A phone being put to sleep switches immediately, even if its power
    /// curtain is still fading. That animation must not light the next panel.
    pub(crate) fn ready(&self, now: u64, visible: bool) -> Option<bool> {
        match self {
            Self::Out { folded, .. } if !visible || self.level(now) == 0.0 => Some(*folded),
            _ => None,
        }
    }

    fn level(&self, now: u64) -> f32 {
        match *self {
            Self::Steady => 1.0,
            Self::Out { since, from, .. } => from * (1.0 - progress(now.saturating_sub(since), OUT_MS)),
            Self::FirstFrame => 0.0,
            Self::In { since, from } => from + (1.0 - from) * progress(now.saturating_sub(since), IN_MS),
        }
    }

    /// Called after scanout has caught up. Before this the new backlight is off.
    pub(crate) fn illuminate(&mut self, now: u64) -> f32 {
        if matches!(self, Self::FirstFrame) {
            *self = Self::In { since: now, from: 0.0 };
        }
        let level = self.level(now);
        if matches!(self, Self::In { .. }) && level == 1.0 {
            *self = Self::Steady;
        }
        level
    }

    pub(crate) fn next_in(&self, now: u64) -> Option<i32> {
        let ends = match *self {
            Self::Out { since, .. } => since + OUT_MS,
            Self::In { since, .. } => since + IN_MS,
            _ => return None,
        };
        Some(ends.saturating_sub(now).min(STEP_MS) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_panel_fades_out_then_new_panel_waits_for_its_frame() {
        let mut fade = PanelFade::default();
        assert!(fade.request(true, false, 0));
        assert_eq!(fade.ready(OUT_MS / 2, true), None);
        assert!((0.0..1.0).contains(&fade.illuminate(OUT_MS / 2)));
        assert!(!fade.request(true, false, OUT_MS - 1), "duplicate events don't restart the ramp");
        assert_eq!(fade.ready(OUT_MS, true), Some(false));

        fade = PanelFade::FirstFrame;
        assert_eq!(fade.level(500), 0.0, "waiting for scanout must not count toward the fade");
        assert_eq!(fade.illuminate(500), 0.0);
        assert!((0.0..1.0).contains(&fade.illuminate(500 + IN_MS / 2)));
        assert_eq!(fade.illuminate(500 + IN_MS), 1.0);
        assert_eq!(fade.next_in(500 + IN_MS), None);
    }

    #[test]
    fn a_new_fold_before_the_handoff_reverses_the_ramp() {
        let mut fade = PanelFade::default();
        fade.request(false, true, 0);
        let now = OUT_MS / 2;
        let dim = fade.illuminate(now);
        assert!(!fade.request(false, false, now));
        assert!(!fade.leaving());
        assert_eq!(fade.illuminate(now), dim);
        assert!(fade.illuminate(now + IN_MS / 2) > dim);
        assert_eq!(fade.ready(now + IN_MS, true), None);
        assert_eq!(fade.illuminate(now + IN_MS), 1.0);
    }

    #[test]
    fn a_new_fold_during_fade_in_starts_from_its_current_light() {
        let mut fade = PanelFade::FirstFrame;
        fade.illuminate(100);
        let now = 100 + IN_MS / 2;
        let lit = fade.illuminate(now);
        assert!(fade.request(true, false, now));
        assert_eq!(fade.illuminate(now), lit);
        assert!(fade.illuminate(now + OUT_MS / 2) < lit);
        assert_eq!(fade.ready(now + OUT_MS, true), Some(false));
    }

    #[test]
    fn a_pending_fold_does_not_wait_if_the_screen_goes_to_sleep() {
        let mut fade = PanelFade::default();
        fade.request(false, true, 0);
        assert_eq!(fade.ready(1, true), None);
        assert_eq!(fade.ready(1, false), Some(true));
    }
}
