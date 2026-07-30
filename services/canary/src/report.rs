//! Per-step results and the overall run report. Pure: recording a step and deciding pass/fail does
//! no I/O, so the aggregation logic is unit-tested. The metrics emitter and `main` consume this.

use std::time::Duration;

/// The outcome of a single canary step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    pub latency_ms: u128,
    /// Failure detail (the assertion error), if any.
    pub detail: Option<String>,
}

/// The result of a whole tier run (shallow or deep).
#[derive(Debug, Clone)]
pub struct RunReport {
    pub tier: String,
    pub steps: Vec<StepResult>,
}

impl RunReport {
    pub fn new(tier: impl Into<String>) -> Self {
        Self {
            tier: tier.into(),
            steps: Vec::new(),
        }
    }

    /// Record a completed step from its name, elapsed time, and result.
    pub fn record(
        &mut self,
        name: impl Into<String>,
        latency: Duration,
        result: Result<(), String>,
    ) {
        let (ok, detail) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        self.steps.push(StepResult {
            name: name.into(),
            ok,
            latency_ms: latency.as_millis(),
            detail,
        });
    }

    /// True iff every recorded step passed.
    pub fn passed(&self) -> bool {
        self.steps.iter().all(|s| s.ok)
    }

    /// The first failure detail, if any (handy for a concise top-line error).
    pub fn first_failure(&self) -> Option<&StepResult> {
        self.steps.iter().find(|s| !s.ok)
    }

    /// A multi-line, human-readable summary of every step.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        for s in &self.steps {
            let mark = if s.ok { "PASS" } else { "FAIL" };
            out.push_str(&format!("[{}] {} ({} ms)", mark, s.name, s.latency_ms));
            if let Some(d) = &s.detail {
                out.push_str(&format!(" -- {d}"));
            }
            out.push('\n');
        }
        let passed = self.steps.iter().filter(|s| s.ok).count();
        out.push_str(&format!(
            "{} tier: {}/{} steps passed -> {}",
            self.tier,
            passed,
            self.steps.len(),
            if self.passed() { "SUCCESS" } else { "FAILURE" }
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_report_passes_vacuously() {
        let r = RunReport::new("shallow");
        assert!(r.passed());
        assert!(r.first_failure().is_none());
    }

    #[test]
    fn records_pass_and_fail() {
        let mut r = RunReport::new("deep");
        r.record("login", Duration::from_millis(12), Ok(()));
        r.record("delete", Duration::from_millis(34), Err("boom".to_string()));
        assert!(!r.passed());
        assert_eq!(r.steps.len(), 2);
        assert!(r.steps[0].ok);
        assert!(!r.steps[1].ok);
        assert_eq!(r.first_failure().unwrap().name, "delete");
        assert_eq!(r.first_failure().unwrap().detail.as_deref(), Some("boom"));
    }

    #[test]
    fn passes_when_all_steps_ok() {
        let mut r = RunReport::new("shallow");
        r.record("a", Duration::from_millis(1), Ok(()));
        r.record("b", Duration::from_millis(2), Ok(()));
        assert!(r.passed());
    }

    #[test]
    fn summary_contains_counts_and_outcome() {
        let mut r = RunReport::new("deep");
        r.record("ok-step", Duration::from_millis(1), Ok(()));
        r.record("bad-step", Duration::from_millis(2), Err("nope".into()));
        let s = r.summary();
        assert!(s.contains("[PASS] ok-step"));
        assert!(s.contains("[FAIL] bad-step"));
        assert!(s.contains("-- nope"));
        assert!(s.contains("1/2 steps passed"));
        assert!(s.contains("FAILURE"));
    }
}
