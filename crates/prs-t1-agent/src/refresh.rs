//! PRS-T1 refresh policy.
//!
//! This module is deliberately device-side. It turns a UI redraw reason into
//! an EPDC waveform/completion plan, while `framebuffer` remains responsible
//! for damage calculation and ioctl submission. It does not own reader
//! pagination: a page turn has already happened before a plan is requested.

use crate::framebuffer::WaveformMode;

/// The minimum visual quality required by a rendered Markdown page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTone {
    /// Ordinary black/white text and rules. Font antialiasing is retained,
    /// but the document contains no intentional gray paint or raster image.
    Monochrome,
    /// A page containing an image, gray decoration/background, or gray syntax
    /// ink. Use a grayscale waveform so those pixels are not treated as a
    /// fast one-bit interaction update.
    Grayscale,
}

/// Why the visible frame is being submitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshReason {
    FullRedraw,
    PageTurn(PageTone),
    StatusBar,
    Transient,
}

impl RefreshReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::FullRedraw => "full-redraw",
            Self::PageTurn(PageTone::Monochrome) => "text-page-turn",
            Self::PageTurn(PageTone::Grayscale) => "grayscale-page-turn",
            Self::StatusBar => "status-bar",
            Self::Transient => "transient",
        }
    }
}

/// The EPDC-facing choices for one rendered frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefreshPlan {
    waveform: WaveformMode,
    wait_for_completion: bool,
    force_refresh: bool,
    cleanup: bool,
}

impl RefreshPlan {
    pub const fn new(
        waveform: WaveformMode,
        wait_for_completion: bool,
        force_refresh: bool,
        cleanup: bool,
    ) -> Self {
        Self {
            waveform,
            wait_for_completion,
            force_refresh,
            cleanup,
        }
    }

    pub const fn waveform(self) -> WaveformMode {
        self.waveform
    }

    pub const fn wait_for_completion(self) -> bool {
        self.wait_for_completion
    }

    pub const fn force_refresh(self) -> bool {
        self.force_refresh
    }

    pub const fn is_cleanup(self) -> bool {
        self.cleanup
    }
}

/// Number of ordinary page turns allowed between quality cleanup updates.
///
/// The T1 evidence shows DU is about twice as fast as GC16, but repeated DU
/// ghosting was not measurable from framebuffer memory. Four page turns is a
/// bounded, reviewable starting cadence; the hardware observation log records
/// that this remains tunable after an optical test.
pub const FAST_PAGE_TURNS_BEFORE_CLEANUP: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RefreshPolicy {
    fast_page_turns: u8,
}

impl RefreshPolicy {
    /// Select a plan without changing policy state. State advances only after
    /// the corresponding EPDC submission succeeds.
    pub fn plan(&self, reason: RefreshReason) -> RefreshPlan {
        match reason {
            RefreshReason::FullRedraw => Self::full_plan(),
            RefreshReason::PageTurn(PageTone::Grayscale) => {
                // A page with intentional gray paint or imagery gets a
                // synchronous full-content quality update. The caller's
                // dirty region still limits the document area.
                RefreshPlan::new(WaveformMode::Gc16, true, true, false)
            }
            RefreshReason::PageTurn(PageTone::Monochrome) => {
                if self.fast_page_turns >= FAST_PAGE_TURNS_BEFORE_CLEANUP {
                    RefreshPlan::new(WaveformMode::Gc16, true, true, true)
                } else {
                    RefreshPlan::new(WaveformMode::Du, false, false, false)
                }
            }
            RefreshReason::StatusBar | RefreshReason::Transient => {
                RefreshPlan::new(WaveformMode::Du, false, false, false)
            }
        }
    }

    /// Record a successful submission. The reader's logical page has already
    /// changed independently; this only controls the next EPDC plan.
    pub fn record_success(&mut self, reason: RefreshReason, plan: RefreshPlan) {
        match reason {
            RefreshReason::FullRedraw | RefreshReason::PageTurn(PageTone::Grayscale) => {
                self.fast_page_turns = 0;
            }
            RefreshReason::PageTurn(PageTone::Monochrome) if plan.force_refresh => {
                self.fast_page_turns = 0;
            }
            RefreshReason::PageTurn(PageTone::Monochrome) => {
                self.fast_page_turns = self.fast_page_turns.saturating_add(1);
            }
            RefreshReason::StatusBar | RefreshReason::Transient => {}
        }
    }

    fn full_plan() -> RefreshPlan {
        RefreshPlan::new(WaveformMode::Gc16, true, true, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_page_turns_use_async_du_then_gc16_cleanup() {
        let mut policy = RefreshPolicy::default();
        for turn in 0..FAST_PAGE_TURNS_BEFORE_CLEANUP {
            let reason = RefreshReason::PageTurn(PageTone::Monochrome);
            let plan = policy.plan(reason);
            assert_eq!(plan.waveform(), WaveformMode::Du, "turn {turn}");
            assert!(!plan.wait_for_completion());
            assert!(!plan.force_refresh());
            assert!(!plan.is_cleanup());
            policy.record_success(reason, plan);
        }

        let reason = RefreshReason::PageTurn(PageTone::Monochrome);
        let plan = policy.plan(reason);
        assert_eq!(plan.waveform(), WaveformMode::Gc16);
        assert!(plan.wait_for_completion());
        assert!(plan.force_refresh());
        assert!(plan.is_cleanup());
        policy.record_success(reason, plan);
        assert_eq!(policy.fast_page_turns, 0);
    }

    #[test]
    fn gray_page_turns_are_synchronous_quality_updates_and_reset_cadence() {
        let mut policy = RefreshPolicy::default();
        let text = RefreshReason::PageTurn(PageTone::Monochrome);
        let text_plan = policy.plan(text);
        policy.record_success(text, text_plan);
        assert_eq!(policy.fast_page_turns, 1);

        let reason = RefreshReason::PageTurn(PageTone::Grayscale);
        let plan = policy.plan(reason);
        assert_eq!(plan.waveform(), WaveformMode::Gc16);
        assert!(plan.wait_for_completion());
        assert!(plan.force_refresh());
        assert!(!plan.is_cleanup());
        policy.record_success(reason, plan);
        assert_eq!(policy.fast_page_turns, 0);
    }

    #[test]
    fn status_and_transient_updates_are_async_du_without_page_cadence() {
        let mut policy = RefreshPolicy::default();
        let page = RefreshReason::PageTurn(PageTone::Monochrome);
        let page_plan = policy.plan(page);
        policy.record_success(page, page_plan);

        for reason in [RefreshReason::StatusBar, RefreshReason::Transient] {
            let plan = policy.plan(reason);
            assert_eq!(plan.waveform(), WaveformMode::Du);
            assert!(!plan.wait_for_completion());
            assert!(!plan.force_refresh());
            policy.record_success(reason, plan);
        }
        assert_eq!(policy.fast_page_turns, 1);
    }

    #[test]
    fn full_redraw_is_synchronous_gc16_and_resets_cadence() {
        let mut policy = RefreshPolicy::default();
        let page = RefreshReason::PageTurn(PageTone::Monochrome);
        let page_plan = policy.plan(page);
        policy.record_success(page, page_plan);

        let reason = RefreshReason::FullRedraw;
        let plan = policy.plan(reason);
        assert_eq!(
            plan,
            RefreshPlan::new(WaveformMode::Gc16, true, true, false)
        );
        policy.record_success(reason, plan);
        assert_eq!(policy.fast_page_turns, 0);
    }

    #[test]
    fn failed_submission_does_not_advance_cleanup_cadence() {
        let policy = RefreshPolicy::default();
        let reason = RefreshReason::PageTurn(PageTone::Monochrome);
        let plan = policy.plan(reason);
        assert_eq!(plan.waveform(), WaveformMode::Du);
        assert_eq!(policy.fast_page_turns, 0);
    }
}
