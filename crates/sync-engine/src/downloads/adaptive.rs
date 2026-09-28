/// Application-level throughput probing, inspired by additive increase / multiplicative
/// decrease. This controls episode admissions; TCP still controls individual connections.
pub(super) struct AdaptiveLimit {
    pub limit: usize,
    baseline: Option<(f64, usize)>,
    probe: Option<(usize, f64, usize)>,
    cooldown: usize,
    stalled: usize,
}

pub const INITIAL_TRANSFERS: usize = 2;
pub const MAX_TRANSFERS: usize = 16;

impl Default for AdaptiveLimit {
    fn default() -> Self {
        Self {
            limit: INITIAL_TRANSFERS,
            baseline: None,
            probe: None,
            cooldown: 0,
            stalled: 0,
        }
    }
}

impl AdaptiveLimit {
    pub fn sample(&mut self, rate: u64, saturated: bool, downloading: usize, retried: bool) {
        if retried {
            self.back_off();
            return;
        }
        // An empty queue, conversion, or upload is not network congestion.
        if !saturated || downloading == 0 {
            self.baseline = None;
            // Inconclusive probes do not permanently raise the budget.
            if let Some((previous_limit, _, _)) = self.probe.take() {
                self.limit = previous_limit;
                self.cooldown = 3;
            }
            self.stalled = 0;
            return;
        }
        self.stalled = if rate == 0 { self.stalled + 1 } else { 0 };
        let rate = rate as f64;
        let slowdown = self.baseline.is_some_and(|(baseline, receivers)| {
            downloading >= receivers && rate < baseline * 0.65
        });
        if self.stalled >= 2 || (rate > 0. && slowdown) {
            self.back_off();
            return;
        }
        if rate == 0. {
            return;
        }
        let smoothed = self
            .baseline
            .map_or(rate, |(baseline, _)| baseline * 0.5 + rate * 0.5);
        self.baseline = Some((smoothed, downloading));
        if let Some((previous_limit, previous_rate, receivers)) = self.probe.take() {
            // Keep an extra slot only if aggregate throughput improves with at
            // least as many receivers; otherwise revert and allow time to settle.
            // Five percent filters small fluctuations while still recognizing a
            // useful extra receiver near the 16-transfer ceiling (about 7%).
            if downloading < receivers || rate < previous_rate * 1.05 {
                self.limit = previous_limit;
                self.cooldown = 3;
                self.baseline = Some((rate, downloading));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
        } else if self.limit < MAX_TRANSFERS {
            self.probe = Some((self.limit, smoothed, downloading));
            self.limit += 1;
        }
    }

    fn back_off(&mut self) {
        self.limit = (self.limit / 2).max(1);
        self.baseline = None;
        self.probe = None;
        self.cooldown = 3;
        self.stalled = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn useful_bandwidth_growth_opens_slots_up_to_the_cap() {
        let mut limit = AdaptiveLimit::default();
        for _ in 0..100 {
            limit.sample(limit.limit as u64 * 1_000_000, true, limit.limit, false);
        }
        assert_eq!(limit.limit, MAX_TRANSFERS);
    }

    #[test]
    fn plateau_reverts_the_probe_and_waits_before_trying_again() {
        let mut limit = AdaptiveLimit::default();
        limit.sample(1_000_000, true, 2, false);
        assert_eq!(limit.limit, 3);
        limit.sample(1_040_000, true, 3, false);
        assert_eq!(limit.limit, 2);
        for _ in 0..3 {
            limit.sample(1_000_000, true, 2, false);
            assert_eq!(limit.limit, 2);
        }
        limit.sample(1_000_000, true, 2, false);
        assert_eq!(limit.limit, 3);
    }

    #[test]
    fn inconclusive_probes_do_not_accumulate_slots() {
        let mut limit = AdaptiveLimit::default();
        for _ in 0..30 {
            limit.sample(1_000_000, true, 2, false);
            limit.sample(0, true, 0, false);
            assert_eq!(limit.limit, INITIAL_TRANSFERS);
        }
        let mut limit = AdaptiveLimit::default();
        limit.sample(1_000_000, true, 2, false);
        limit.sample(500_000, true, 1, false);
        assert_eq!(limit.limit, INITIAL_TRANSFERS);
        for _ in 0..10 {
            limit.sample(10_000_000, false, 1, false);
        }
        assert_eq!(limit.limit, INITIAL_TRANSFERS);
    }

    #[test]
    fn congestion_and_stalls_back_off_but_idle_uploads_and_pause_do_not() {
        let mut limit = AdaptiveLimit::default();
        for _ in 0..12 {
            limit.sample(limit.limit as u64 * 1_000_000, true, limit.limit, false);
        }
        let before = limit.limit;
        limit.sample(1_000, true, before, false);
        assert_eq!(limit.limit, before / 2);
        let before = limit.limit;
        for _ in 0..10 {
            limit.sample(0, true, 0, false);
            limit.sample(0, false, before, false);
        }
        assert_eq!(limit.limit, before);
        limit.sample(0, true, before, false);
        assert_eq!(limit.limit, before);
        limit.sample(0, true, before, false);
        assert_eq!(limit.limit, (before / 2).max(1));
        for _ in 0..10 {
            limit.sample(0, true, 1, true);
        }
        assert_eq!(limit.limit, 1);
        for _ in 0..10 {
            limit.sample(limit.limit as u64 * 1_000_000, true, limit.limit, false);
        }
        assert!(limit.limit > 1);
    }
}
