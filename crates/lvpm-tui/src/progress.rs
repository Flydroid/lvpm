//! How far a running install has got, phase by phase, folded from core's
//! events.

use crate::worker::Outcome;
use lvpm_core::events::{Event, Phase, SkipReason};

/// The phases in the order an install goes through them.
pub const PHASES: [Phase; 6] = [
    Phase::Resolve,
    Phase::Download,
    Phase::Unpack,
    Phase::Relink,
    Phase::Hooks,
    Phase::Refresh,
];

pub fn name(phase: Phase) -> &'static str {
    match phase {
        Phase::Resolve => "resolve",
        Phase::Download => "download",
        Phase::Unpack => "unpack",
        Phase::Relink => "relink",
        Phase::Hooks => "hooks",
        Phase::Refresh => "refresh",
    }
}

/// Why a phase was left out, in a reader's words.
pub fn reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::ScratchDestination => "scratch destination",
        SkipReason::Headless => "LabVIEW runs headless",
        SkipReason::Declined => "left out",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Waiting,
    Running,
    Done,
    Skipped,
    /// The install ended here, cancelled or failed.
    Stopped,
}

impl State {
    pub fn mark(self) -> &'static str {
        match self {
            State::Waiting => "·",
            State::Running => ">",
            State::Done => "√",
            State::Skipped => "-",
            State::Stopped => "×",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// The phase started last; Download and Unpack start once per package.
    current: Option<usize>,
    /// The furthest phase started.
    furthest: usize,
    skipped: [Option<SkipReason>; PHASES.len()],
    /// Files are being written, so a cancel is refused.
    unpacked: bool,
    pub outcome: Option<Outcome>,
    /// A cancel was asked for and accepted.
    pub cancelling: bool,
    /// A cancel was asked for after files were written.
    pub refused: bool,
}

impl Progress {
    pub fn add(&mut self, event: &Event) {
        match event {
            Event::PhaseStarted { phase } => {
                let at = index(*phase);
                self.current = Some(at);
                self.furthest = self.furthest.max(at);
                self.unpacked |= *phase == Phase::Unpack;
            }
            Event::PhaseSkipped { phase, reason, .. } => {
                self.skipped[index(*phase)] = Some(*reason)
            }
            // Taking out the old version is a replacement's first file write.
            Event::PackageReplacing { dry_run: false, .. } => self.unpacked = true,
            _ => {}
        }
    }

    pub fn state(&self, phase: Phase) -> State {
        let at = index(phase);
        if self.skipped[at].is_some() {
            return State::Skipped;
        }
        match (self.current, &self.outcome) {
            (Some(current), None) if current == at => State::Running,
            (Some(current), Some(outcome)) if current == at && *outcome != Outcome::Done => {
                State::Stopped
            }
            (Some(_), Some(Outcome::Done)) => State::Done,
            (Some(_), _) if at <= self.furthest => State::Done,
            _ => State::Waiting,
        }
    }

    /// Why `phase` was left out, if it was.
    pub fn skipped(&self, phase: Phase) -> Option<SkipReason> {
        self.skipped[index(phase)]
    }

    /// The phase started last.
    pub fn current(&self) -> Option<Phase> {
        self.current.map(|at| PHASES[at])
    }

    /// A cancel still stops the install before it writes a file.
    pub fn cancellable(&self) -> bool {
        self.outcome.is_none() && !self.unpacked
    }
}

fn index(phase: Phase) -> usize {
    PHASES.iter().position(|&p| p == phase).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(phase: Phase) -> Event {
        Event::PhaseStarted { phase }
    }

    fn states(progress: &Progress) -> Vec<State> {
        PHASES.iter().map(|&phase| progress.state(phase)).collect()
    }

    #[test]
    fn phases_before_the_current_one_are_done_and_later_ones_wait() {
        let mut progress = Progress::default();
        assert_eq!(states(&progress), [State::Waiting; 6]);
        progress.add(&started(Phase::Resolve));
        progress.add(&started(Phase::Download));
        use State::*;
        assert_eq!(
            states(&progress),
            [Done, Running, Waiting, Waiting, Waiting, Waiting]
        );
        assert!(progress.cancellable());
    }

    #[test]
    fn a_second_package_downloads_after_the_first_unpacked() {
        let mut progress = Progress::default();
        for phase in [Phase::Resolve, Phase::Download, Phase::Unpack] {
            progress.add(&started(phase));
        }
        progress.add(&started(Phase::Download));
        use State::*;
        assert_eq!(
            states(&progress),
            [Done, Running, Done, Waiting, Waiting, Waiting]
        );
        assert!(!progress.cancellable(), "files are written");
    }

    #[test]
    fn a_skipped_phase_stays_skipped_and_the_end_marks_the_rest() {
        let mut progress = Progress::default();
        progress.add(&started(Phase::Resolve));
        progress.add(&Event::PhaseSkipped {
            phase: Phase::Hooks,
            reason: lvpm_core::events::SkipReason::ScratchDestination,
            skipped: 0,
        });
        progress.outcome = Some(Outcome::Done);
        use State::*;
        assert_eq!(states(&progress), [Done, Done, Done, Done, Skipped, Done]);
    }

    #[test]
    fn removing_the_old_version_ends_the_clean_cancel() {
        let mut progress = Progress::default();
        let replacing = |dry_run| Event::PackageReplacing {
            name: "alpha".into(),
            installed: "2.0".into(),
            wanted: "1.0".into(),
            upgrade: false,
            old_files: 1,
            dry_run,
        };
        progress.add(&started(Phase::Resolve));
        progress.add(&replacing(true));
        assert!(progress.cancellable(), "a dry run writes nothing");
        progress.add(&replacing(false));
        assert!(!progress.cancellable());
    }

    #[test]
    fn a_cancel_or_a_failure_stops_the_current_phase() {
        let mut progress = Progress::default();
        progress.add(&started(Phase::Resolve));
        progress.add(&started(Phase::Download));
        progress.outcome = Some(Outcome::Cancelled);
        assert_eq!(progress.state(Phase::Download), State::Stopped);
        assert_eq!(progress.state(Phase::Resolve), State::Done);
        assert_eq!(progress.state(Phase::Unpack), State::Waiting);
        assert!(!progress.cancellable());
    }
}
