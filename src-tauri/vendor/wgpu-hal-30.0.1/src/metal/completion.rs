//! Completion notification and ordered retirement for Metal fence waits.

#[derive(Debug, Default)]
pub(super) struct Progress {
    /// Highest fully checked, contiguous submission, never merely a callback.
    pub value: u64,
    /// Wakes waiters without treating a later callback as earlier completion.
    pub revision: u64,
    pub failed: bool,
}

impl Progress {
    pub fn completed(&mut self, failed: bool) {
        self.failed |= failed;
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum BatchState { Complete, Pending, Failed }

/// Inspect every batch for errors, but retire only a completed prefix. Metal
/// may finish independent later commands before an earlier one completes.
pub(super) fn latest(retired: u64, batches: impl IntoIterator<Item = (u64, BatchState)>) -> Result<u64, ()> {
    let mut value = retired;
    let mut prefix_complete = true;
    for (next, state) in batches {
        match state {
            BatchState::Complete if prefix_complete => value = value.max(next),
            BatchState::Failed => return Err(()),
            BatchState::Pending => prefix_complete = false,
            BatchState::Complete => {},
        }
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_compute_before_successful_signal_is_not_success() {
        let mut progress = Progress::default();
        progress.completed(true);
        progress.completed(false);
        assert!(progress.failed);
        assert_eq!(progress.value, 0);
        assert_eq!(progress.revision, 2);
    }

    #[test]
    fn callbacks_wake_waiters_without_advancing_the_checked_fence() {
        let mut progress = Progress::default();
        progress.completed(false);
        progress.completed(false);
        assert_eq!(progress.value, 0);
        assert_eq!(progress.revision, 2);
        assert!(!progress.failed);
    }

    #[test]
    fn later_completion_cannot_retire_earlier_pending_work() {
        assert_eq!(latest(2, [(4, BatchState::Complete), (9, BatchState::Pending), (12, BatchState::Complete)]), Ok(4));
        assert_eq!(latest(4, [(9, BatchState::Complete), (12, BatchState::Complete)]), Ok(12));
    }

    #[test]
    fn failure_after_pending_work_is_still_reported() {
        assert_eq!(latest(2, [(4, BatchState::Pending), (9, BatchState::Failed)]), Err(()));
    }
}
