//! The elevated helper protocol shared by `pixelplus-imager-cli write` and the GUI
//! binary's `--helper` mode.
//!
//! * The job (image path, device, settings incl. the Wi-Fi password) arrives as a JSON file
//!   that is deleted as soon as it has been read.
//! * Progress goes out as JSON lines on stdout *and* appended to `--progress FILE`
//!   (osascript/pkexec do not always pass stdout through while running).
//! * Creating `<progress file>.cancel` asks the helper to stop (the unprivileged GUI
//!   cannot signal a root process).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::job::{self, WriteJob};
use crate::write::{Phase, Progress};

pub fn cancel_path(progress_file: &Path) -> PathBuf {
    let mut p = progress_file.as_os_str().to_owned();
    p.push(".cancel");
    PathBuf::from(p)
}

/// Run a write job; returns the process exit code.
pub fn run_write(job_path: &Path, progress_file: Option<&Path>) -> i32 {
    let mut sink =
        progress_file.and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
    let mut emit = |p: &Progress| {
        let line = serde_json::to_string(p).unwrap_or_default();
        println!("{line}");
        if let Some(f) = sink.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    };
    let job: WriteJob = match std::fs::read_to_string(job_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(j) => j,
        Err(e) => {
            emit(&Progress::msg(
                Phase::Error,
                format!("cannot read job: {e}"),
            ));
            return 1;
        }
    };
    // The job file holds the Wi-Fi password: remove it as soon as it is loaded.
    let _ = std::fs::remove_file(job_path);

    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    if let Some(pf) = progress_file {
        let (cancel, done, flag) = (cancel.clone(), done.clone(), cancel_path(pf));
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if flag.exists() {
                    cancel.store(true, Ordering::Relaxed);
                    let _ = std::fs::remove_file(&flag);
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
    }

    let mut last = (Phase::Prepare, u64::MAX);
    let r = job::run(
        &job,
        &mut |p: Progress| {
            // throttle to one line per percent per phase
            let pct = p.total.map(|t| p.bytes * 100 / t.max(1)).unwrap_or(0);
            if p.message.is_some() || (p.phase, pct) != last {
                last = (p.phase, pct);
                emit(&p);
            }
        },
        &cancel,
    );
    done.store(true, Ordering::Relaxed);
    match r {
        Ok(()) => 0,
        Err(e) => {
            emit(&Progress::msg(Phase::Error, e.to_string()));
            1
        }
    }
}

/// `args` = everything after `--helper` (GUI) or the program name (CLI):
/// `write JOB.json [--progress FILE]`.
pub fn main_with_args(args: &[String]) -> i32 {
    match args {
        [cmd, job] if cmd == "write" => run_write(Path::new(job), None),
        [cmd, job, flag, pf] if cmd == "write" && flag == "--progress" => {
            run_write(Path::new(job), Some(Path::new(pf)))
        }
        _ => {
            eprintln!("usage: write JOB.json [--progress FILE]");
            2
        }
    }
}
