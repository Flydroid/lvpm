//! Supervising the processes a scenario starts: running a command with a
//! deadline, and making sure a LabVIEW started on a venv does not outlive the
//! scenario.

use super::golden::Output;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Component, Path};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How long the output of a command that has ended or been killed is awaited:
/// a process it started may hold the pipes open for much longer.
const READER_GRACE: Duration = Duration::from_secs(5);
/// How long the tool that kills a command's process tree may take.
const KILL_DEADLINE: Duration = Duration::from_secs(10);
/// How long the PowerShell that sweeps a venv's processes may take.
const SWEEP_DEADLINE: Duration = Duration::from_secs(60);
const PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a killed LabVIEW has to give its VI Server port back.
pub const PORT_CLOSE_DEADLINE: Duration = Duration::from_secs(15);
/// The only programs the sweep may stop: the LabVIEW a scenario launched on
/// its venv, and any `lvpm` still running for it.
const SWEEPABLE_PROGRAMS: &[&str] = &["LabVIEW.exe", "lvpm.exe"];
/// A directory this deep below the temp directory is a scenario's own venv
/// (`<sandbox>/work/.lv-venv`), never a directory someone else owns.
const MIN_DEPTH_BELOW_TEMP: usize = 3;
/// The sweep's target travels in the environment, not the command line, so
/// the PowerShell running it does not match itself.
const SWEEP_ROOT_ENV_VAR: &str = "LVPM_SWEEP_ROOT";

/// Why a command gave no exit status.
#[derive(Debug)]
pub struct Failed {
    /// `run_within`'s error text: what happened, and the output so far.
    pub message: String,
    /// Still running at its deadline, and killed.
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Run `command` to completion, or kill it when `timeout` passes. Its stdin
/// carries `input` and then ends; with no input it is empty.
pub fn run_within(
    command: Command,
    input: Option<&str>,
    timeout: Duration,
) -> Result<Output, String> {
    run_capturing(command, input, timeout).map_err(|failed| failed.message)
}

/// `run_within`, with the output a command gave before its deadline kept apart
/// in the error.
pub fn run_capturing(
    mut command: Command,
    input: Option<&str>,
    timeout: Duration,
) -> Result<Output, Failed> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Failed {
            message: format!("could not start: {e}"),
            timed_out: false,
            stdout: String::new(),
            stderr: String::new(),
        })?;
    let stdout = read_in_background(child.stdout.take().expect("piped stdout"));
    let stderr = read_in_background(child.stderr.take().expect("piped stderr"));
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        // From its own thread, so a child that never reads cannot hold the
        // deadline back. A child that exits without reading, or is killed,
        // closes the pipe; that ends the write.
        let text = text.to_owned();
        std::thread::spawn(move || stdin.write_all(text.as_bytes()).ok());
    }
    let status = wait_until(&mut child, Instant::now() + timeout);
    let until = Instant::now() + READER_GRACE;
    let (stdout, stderr) = (stdout.text_by(until), stderr.text_by(until));
    let failed = |message: String, timed_out| Failed {
        message: format!("{message}\nstdout so far: {stdout}\nstderr so far: {stderr}"),
        timed_out,
        stdout: stdout.clone(),
        stderr: stderr.clone(),
    };
    match status {
        Ok(Some(status)) => Ok(Output {
            stdout,
            stderr,
            exit_code: status.code().unwrap_or(-1),
            tree: None,
        }),
        Ok(None) => Err(failed(format!("timed out after {timeout:?}"), true)),
        Err(why) => Err(failed(format!("could not wait for it: {why}"), false)),
    }
}

/// The child's exit status, or `None` once `deadline` passes. The child is
/// dead on every path out of here except a successful exit.
fn wait_until(
    child: &mut Child,
    deadline: Instant,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if Instant::now() >= deadline => {
                stop(child);
                return Ok(None);
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(why) => {
                stop(child);
                return Err(why);
            }
        }
    }
}

/// Kill the child and, on Windows, what it started: a grandchild left running
/// would keep the pipes open.
fn stop(child: &mut Child) {
    if cfg!(windows) {
        let mut taskkill = Command::new("taskkill");
        taskkill.args(["/T", "/F", "/PID", &child.id().to_string()]);
        run_quietly(taskkill, KILL_DEADLINE);
    }
    child.kill().ok();
    child.wait().ok();
}

/// Run `command` with its output dropped; kill it when `timeout` passes.
/// Not `run_capturing`: that stops what it runs through `stop`, which runs this.
fn run_quietly(mut command: Command, timeout: Duration) {
    let Ok(mut child) = command.stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return;
    };
    let deadline = Instant::now() + timeout;
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// What a pipe has given so far, and whether it has reached its end.
struct Reader {
    bytes: Arc<Mutex<Vec<u8>>>,
    ended: Receiver<()>,
}

impl Reader {
    /// The text read once the pipe ended, or by `deadline`, whichever is first.
    fn text_by(&self, deadline: Instant) -> String {
        self.ended
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok();
        let bytes = self.bytes.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

// The thread outlives a pipe that never ends, until its holder does.
fn read_in_background(mut pipe: impl Read + Send + 'static) -> Reader {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (ended_tx, ended) = std::sync::mpsc::channel();
    let shared = Arc::clone(&bytes);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n @ 1..) = pipe.read(&mut chunk) {
            let mut bytes = shared.lock().unwrap_or_else(PoisonError::into_inner);
            bytes.extend_from_slice(&chunk[..n]);
        }
        ended_tx.send(()).ok();
    });
    Reader { bytes, ended }
}

pub fn port_is_listening(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, PORT_PROBE_TIMEOUT).is_ok()
}

/// Wait for nothing to listen on `port` any more, up to `deadline_after`.
pub fn wait_until_port_is_quiet(port: u16, deadline_after: Duration) -> bool {
    let deadline = Instant::now() + deadline_after;
    while port_is_listening(port) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    true
}

/// Why `root` may not be swept, or `Ok` when it is a scenario's own venv: an
/// absolute path, strictly below the temp directory, `MIN_DEPTH_BELOW_TEMP`
/// components deep.
pub fn check_sweepable(root: &Path, temp_dir: &Path) -> Result<(), String> {
    if root.as_os_str().is_empty() || !root.is_absolute() {
        return Err(format!("`{}` is not an absolute path", root.display()));
    }
    let below = root
        .strip_prefix(temp_dir)
        .map_err(|_| format!("`{}` is not below {}", root.display(), temp_dir.display()))?;
    let depth = below
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .count();
    match depth >= MIN_DEPTH_BELOW_TEMP {
        true => Ok(()),
        false => Err(format!(
            "`{}` is only {depth} below {}; it must be {MIN_DEPTH_BELOW_TEMP} deep",
            root.display(),
            temp_dir.display()
        )),
    }
}

/// Stop the `LabVIEW.exe` and `lvpm.exe` processes whose command line names a
/// path inside `venv_dir`, and say how many there were. A no-op where
/// PowerShell is not available.
///
/// # Panics
/// When `venv_dir` is not a scenario's own venv (see `check_sweepable`): a
/// sweep must never be able to reach a process the scenario did not start.
pub fn stop_processes_in(venv_dir: &Path) -> usize {
    if let Err(why) = check_sweepable(venv_dir, &std::env::temp_dir()) {
        panic!("refusing to stop processes: {why}");
    }
    if !cfg!(windows) {
        return 0;
    }
    let mut powershell = Command::new("powershell");
    powershell
        .args(["-NoProfile", "-NonInteractive", "-Command", &sweep_script()])
        .env(SWEEP_ROOT_ENV_VAR, venv_dir);
    run_capturing(powershell, None, SWEEP_DEADLINE)
        .ok()
        .and_then(|out| out.stdout.trim().parse().ok())
        .unwrap_or(0)
}

/// PowerShell that stops each matching process by id: the program is named in
/// `SWEEPABLE_PROGRAMS`, and the directory is followed by a path separator, a
/// quote, whitespace or the end of the command line (not by more name).
fn sweep_script() -> String {
    let programs = SWEEPABLE_PROGRAMS
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$pattern = '(?i)' + [regex]::Escape($env:{SWEEP_ROOT_ENV_VAR}) + '(?=[\\\\/\"\\s]|$)'; \
         $n = 0; \
         Get-CimInstance Win32_Process | Where-Object {{ \
             $_.Name -in {programs} -and $_.CommandLine -and $_.CommandLine -match $pattern \
         }} | ForEach-Object {{ \
             Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; $n++ \
         }}; \
         Write-Output $n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\Temp" } else { "/tmp" })
    }

    fn inside_temp(parts: &[&str]) -> PathBuf {
        parts.iter().fold(temp(), |path, part| path.join(part))
    }

    #[test]
    fn a_process_that_outlives_the_timeout_is_killed_and_reported() {
        let started = Instant::now();
        let error = run_within(sleeper(), None, Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "was not killed promptly"
        );
    }

    /// Whether a process named `program` is running.
    #[cfg(windows)]
    fn is_running(program: &str) -> bool {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {program}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains(&program.to_lowercase())
    }

    #[cfg(windows)]
    fn stop_by_name(program: &str) {
        Command::new("taskkill")
            .args(["/F", "/IM", program])
            .output()
            .ok();
    }

    #[cfg(windows)]
    #[test]
    fn a_grandchild_holding_the_pipes_dies_with_its_parent_at_the_deadline() {
        let scratch = tempfile::Builder::new()
            .prefix("lvpm-tree-")
            .tempdir()
            .unwrap();
        let program = "lvpm-tree-ping.exe";
        let mut command = Command::new("cmd");
        command
            .args(["/c", "echo", "partial", "&"])
            .arg(ping_copy(scratch.path(), program))
            .args(["-n", "30", "127.0.0.1"]);
        let started = Instant::now();
        let failed = run_capturing(command, None, Duration::from_millis(1500)).unwrap_err();
        let took = started.elapsed();
        let survived = is_running(program);
        stop_by_name(program);
        assert!(failed.timed_out, "{}", failed.message);
        assert!(!survived, "the grandchild outlived the kill");
        assert!(took < READER_GRACE, "the pipes were not freed: {took:?}");
        assert!(
            failed.stdout.contains("partial"),
            "the output so far is kept: {}",
            failed.message
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_straggler_holding_the_pipes_after_the_command_exited_does_not_hang_it() {
        let scratch = tempfile::Builder::new()
            .prefix("lvpm-straggler-")
            .tempdir()
            .unwrap();
        let program = "lvpm-straggler-ping.exe";
        let mut command = Command::new("cmd");
        command
            .args(["/c", "echo", "partial", "&", "start", "", "/b"])
            .arg(ping_copy(scratch.path(), program))
            .args(["-n", "30", "127.0.0.1"]);
        let started = Instant::now();
        let output = run_within(command, None, Duration::from_secs(60));
        let took = started.elapsed();
        stop_by_name(program);
        let output = output.unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.stdout.contains("partial"), "{}", output.stdout);
        assert!(
            took < READER_GRACE + Duration::from_secs(10),
            "waited for the straggler: {took:?}"
        );
    }

    #[test]
    fn input_reaches_the_childs_stdin_and_then_the_pipe_closes() {
        let output =
            run_within(stdin_echo(), Some("an answer\n"), Duration::from_secs(60)).unwrap();
        assert_eq!(output.stdout.trim(), "an answer");
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn input_the_child_never_reads_does_not_hold_back_the_timeout() {
        let input = "x".repeat(8 * 1024 * 1024);
        let started = Instant::now();
        let error = run_within(sleeper(), Some(&input), Duration::from_millis(500)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the write held the deadline back"
        );
    }

    #[test]
    fn a_helper_command_past_its_deadline_returns_promptly() {
        let started = Instant::now();
        run_quietly(sleeper(), Duration::from_millis(300));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "was not killed promptly"
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error_not_a_hang() {
        let error = run_within(
            Command::new("no-such-program-lvpm-test"),
            None,
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(error.contains("could not start"), "{error}");
    }

    #[test]
    fn a_closed_port_is_quiet_at_once_and_an_open_one_is_not() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_is_listening(port));
        assert!(!wait_until_port_is_quiet(port, Duration::from_millis(300)));
        drop(listener);
        assert!(wait_until_port_is_quiet(port, PORT_CLOSE_DEADLINE));
    }

    #[test]
    fn a_port_that_closes_while_waiting_is_reported_quiet() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(listener);
        });
        assert!(wait_until_port_is_quiet(port, PORT_CLOSE_DEADLINE));
        closer.join().unwrap();
    }

    #[test]
    fn a_scenarios_venv_dir_may_be_swept() {
        let venv = inside_temp(&["lvpm-acceptance-x", "work", ".lv-venv"]);
        assert_eq!(check_sweepable(&venv, &temp()), Ok(()));
    }

    #[test]
    fn an_empty_relative_outside_shallow_or_temp_itself_may_not_be_swept() {
        let temp = temp();
        for refused in [
            PathBuf::new(),
            PathBuf::from("work/.lv-venv"),
            temp.clone(),
            inside_temp(&["a"]),
            inside_temp(&["a", "b"]),
            PathBuf::from(if cfg!(windows) {
                r"C:\Elsewhere\a\b\c"
            } else {
                "/elsewhere/a/b/c"
            }),
        ] {
            assert!(check_sweepable(&refused, &temp).is_err(), "{refused:?}");
        }
    }

    #[test]
    #[should_panic(expected = "refusing to stop processes")]
    fn sweeping_an_empty_path_panics() {
        stop_processes_in(Path::new(""));
    }

    #[test]
    #[should_panic(expected = "refusing to stop processes")]
    fn sweeping_the_temp_directory_itself_panics() {
        stop_processes_in(&std::env::temp_dir());
    }

    /// A copy of `ping.exe` named `program` in `dir`, so a process running it
    /// has that name and a command line that names `dir`.
    #[cfg(windows)]
    fn ping_copy(dir: &Path, program: &str) -> PathBuf {
        let system_root = std::env::var("SystemRoot").expect("SystemRoot");
        let copy = dir.join(program);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::copy(Path::new(&system_root).join(r"System32\PING.EXE"), &copy).unwrap();
        copy
    }

    /// A spawned child that is killed when this goes out of scope, so a failed
    /// assertion leaves no `ping` running for a minute.
    #[cfg(windows)]
    struct KillOnDrop(Child);

    #[cfg(windows)]
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    #[cfg(windows)]
    fn spawn_named(dir: &Path, program: &str) -> KillOnDrop {
        let child = Command::new(ping_copy(dir, program))
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        KillOnDrop(child)
    }

    #[cfg(windows)]
    #[test]
    fn the_sweep_stops_an_allowed_program_in_the_venv_and_nothing_else() {
        let scratch = tempfile::Builder::new()
            .prefix("lvpm-sweep-")
            .tempdir()
            .unwrap();
        let venv = scratch.path().join("work").join(".lv-venv");
        let sibling = scratch.path().join("work").join(".lv-venv-other");
        let mut named_in_venv = spawn_named(&venv, "LabVIEW.exe");
        let mut other_program_in_venv = spawn_named(&venv, "other.exe");
        let mut named_in_sibling = spawn_named(&sibling, "LabVIEW.exe");

        assert_eq!(stop_processes_in(&venv), 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        while named_in_venv.0.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(
            named_in_venv.0.try_wait().unwrap().is_some(),
            "LabVIEW.exe in the venv survived"
        );
        assert!(
            other_program_in_venv.0.try_wait().unwrap().is_none(),
            "a program not on the list was stopped"
        );
        assert!(
            named_in_sibling.0.try_wait().unwrap().is_none(),
            "a name sharing only a prefix was stopped"
        );
    }

    #[test]
    fn the_sweep_script_names_the_programs_and_requires_a_path_boundary() {
        let script = sweep_script();
        assert!(script.contains("'LabVIEW.exe','lvpm.exe'"), "{script}");
        assert!(
            script.contains("(?=["),
            "a boundary lookahead is required: {script}"
        );
        assert!(
            !script.contains("taskkill") && !script.contains("/T"),
            "{script}"
        );
    }

    fn stdin_echo() -> Command {
        // `findstr "^"` copies stdin to stdout on Windows, as `cat` does elsewhere.
        if cfg!(windows) {
            let mut command = Command::new("findstr");
            command.arg("^");
            command
        } else {
            Command::new("cat")
        }
    }

    fn sleeper() -> Command {
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("ping", &["-n", "30", "127.0.0.1"])
        } else {
            ("sleep", &["30"])
        };
        let mut command = Command::new(program);
        command.args(args);
        command
    }
}
