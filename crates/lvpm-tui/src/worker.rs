//! Runs core's operations off the UI thread, one job at a time, and hands
//! each result back over a channel the event loop polls.

use crate::destination::{EnvBuilder, Quiet};
use lvpm_core::cancel::CancelToken;
use lvpm_core::errors::Cancelled;
use lvpm_core::events::{Event, EventSink, Phase};
use lvpm_core::install::Manifest;
use lvpm_core::ops::Destination;
use lvpm_core::ops::install::{self, InstallOptions};
use lvpm_core::ops::query::{self, Catalog, PackageInfo};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Every job the worker can run.
#[derive(Debug, Clone)]
pub enum Job {
    /// Read the catalog and what the destination has installed; `refresh`
    /// revalidates the feeds as `--refresh` does.
    Load { refresh: bool },
    /// Read the details of one version of a package, as `lvpm info
    /// <name>@<version>` does.
    Info { name: String, version: String },
    /// Install `package`, `name@version`, as `lvpm install` does; a dry run
    /// only says what it would do. `cancel` stops it until files are written.
    Install {
        package: String,
        dry_run: bool,
        allow_downgrade: bool,
        cancel: CancelToken,
    },
}

/// What a job sends: an install's events as they come, then its result.
#[derive(Debug)]
pub enum Done {
    /// One of core's events; the job runs on.
    Event(Event),
    /// The destination as resolved for this read, and the catalog with what
    /// that destination has installed, or core's error, whole.
    Loaded {
        destination: Result<Destination, String>,
        listing: Result<(Catalog, Vec<Manifest>), String>,
    },
    /// The details of `name` `version`, or core's error, whole.
    Info {
        name: String,
        version: String,
        info: Result<PackageInfo, String>,
    },
    /// An install, or its dry run, ended.
    Installed { dry_run: bool, outcome: Outcome },
}

/// How an operation ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Cancelled,
    /// Core's error, whole.
    Failed(String),
}

/// Holds an install at the start of one phase until released, so a test sees
/// what the TUI shows meanwhile. Only a test sets one; a dry run passes it by.
#[derive(Debug)]
pub struct Park {
    at: Phase,
    state: Mutex<Gate>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct Gate {
    /// The install got to the phase.
    reached: bool,
    /// The install may go on.
    released: bool,
}

impl Park {
    pub fn new(at: Phase) -> Park {
        Park {
            at,
            state: Mutex::new(Gate::default()),
            changed: Condvar::new(),
        }
    }

    /// Block until the install is held; an install that never gets there
    /// fails the test instead of hanging it.
    pub fn wait_reached(&self) {
        let state = self.state.lock().expect("the park's lock");
        let limit = Duration::from_secs(30);
        let (held, waited) = (self.changed)
            .wait_timeout_while(state, limit, |gate| !gate.reached)
            .expect("the park's lock");
        // Unlocked first: a panic holding it would poison the park for teardown.
        drop(held);
        assert!(
            !waited.timed_out(),
            "the install never reached {:?}",
            self.at
        );
    }

    /// Let the install go on; it is not held again.
    pub fn release(&self) {
        self.state.lock().expect("the park's lock").released = true;
        self.changed.notify_all();
    }

    fn hold(&self, phase: Phase) {
        if phase != self.at {
            return;
        }
        let mut state = self.state.lock().expect("the park's lock");
        state.reached = true;
        self.changed.notify_all();
        let released = self.changed.wait_while(state, |gate| !gate.released);
        drop(released.expect("the park's lock"));
    }
}

/// Hands core's events to the UI as they come.
struct Forward<'a> {
    send: Sender<Done>,
    park: Option<&'a Park>,
}

impl EventSink for Forward<'_> {
    fn emit(&self, event: Event) {
        let phase = match &event {
            Event::PhaseStarted { phase } => Some(*phase),
            _ => None,
        };
        // Sent first, so the UI shows the phase the install is held at.
        self.send.send(Done::Event(event)).ok();
        if let (Some(phase), Some(park)) = (phase, self.park) {
            park.hold(phase);
        }
    }
}

/// Start `job` on its own thread; its result arrives on the receiver.
pub fn spawn(builder: &EnvBuilder, job: Job) -> Receiver<Done> {
    let (send, receive) = mpsc::channel();
    let builder = builder.clone();
    std::thread::spawn(move || {
        // The TUI has gone when nobody receives; the result is not needed then.
        send.send(run(&builder, job, &send)).ok();
    });
    receive
}

fn run(builder: &EnvBuilder, job: Job, send: &Sender<Done>) -> Done {
    match job {
        Job::Load { refresh } => {
            let sink = Forward {
                send: send.clone(),
                park: None,
            };
            let env = builder.env(&sink, refresh);
            let destination = builder.destination();
            let listing = query::catalog(&env, None).and_then(|catalog| {
                let installed = match destination.is_ok() {
                    true => query::list(&env)?,
                    false => Vec::new(),
                };
                Ok((catalog, installed))
            });
            Done::Loaded {
                destination,
                listing: listing.map_err(|error| format!("{error:#}")),
            }
        }
        Job::Info { name, version } => {
            let env = builder.env(&Quiet, false);
            let info = query::info(&env, &format!("{name}@{version}"));
            Done::Info {
                name,
                version,
                info: info.map_err(|error| format!("{error:#}")),
            }
        }
        Job::Install {
            package,
            dry_run,
            allow_downgrade,
            cancel,
        } => {
            let park = builder.park.as_deref().filter(|_| !dry_run);
            let sink = Forward {
                send: send.clone(),
                park,
            };
            let mut env = builder.env(&sink, false);
            env.cancel = cancel;
            let opts = InstallOptions {
                dry_run,
                allow_downgrade,
                ..InstallOptions::default()
            };
            let outcome = match install::install(&env, Some(&package), &opts) {
                Ok(()) => Outcome::Done,
                Err(error) if error.chain().any(|cause| cause.is::<Cancelled>()) => {
                    Outcome::Cancelled
                }
                Err(error) => Outcome::Failed(format!("{error:#}")),
            };
            Done::Installed { dry_run, outcome }
        }
    }
}
