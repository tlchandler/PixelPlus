//! `pixelplus-imager-cli` - command-line PixelPlus Imager, and the elevated helper used by
//! the GUI on every OS.
//!
//! ```text
//! pixelplus-imager-cli list                                  # removable drives as JSON
//! pixelplus-imager-cli customize IMAGE.img SETTINGS.json     # write pixelplus.txt into an .img file
//! pixelplus-imager-cli write JOB.json [--progress FILE]      # write + verify + customise a card (root)
//! ```
//! Progress is printed as JSON lines (and appended to `--progress FILE`, which is how the
//! GUI follows an elevated helper whose stdout it cannot read, e.g. under osascript).

use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use pixelplus_imager_core::job::{self, WriteJob};
use pixelplus_imager_core::write::{Phase, Progress};
use pixelplus_imager_core::{drives, ImagerSettings};

fn usage() -> ExitCode {
    eprintln!("usage: pixelplus-imager-cli list | customize IMAGE SETTINGS.json | write JOB.json [--progress FILE]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => match drives::list() {
            Ok(d) => {
                println!("{}", serde_json::to_string_pretty(&d).unwrap());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("customize") if args.len() == 3 => {
            let s: ImagerSettings = match std::fs::read_to_string(&args[2]).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match job::customize_image_file(std::path::Path::new(&args[1]), &s) {
                Ok(()) => {
                    println!("pixelplus.txt written to {}", args[1]);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("write") if args.len() == 2 || (args.len() == 4 && args[2] == "--progress") => {
            let progress_file = args.get(3).cloned();
            run_write(&args[1], progress_file.as_deref())
        }
        _ => usage(),
    }
}

/// Shared with the GUI binary's `--helper` mode.
pub fn run_write(job_path: &str, progress_file: Option<&str>) -> ExitCode {
    let mut sink = progress_file.and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
    let mut emit = |p: &Progress| {
        let line = serde_json::to_string(p).unwrap();
        println!("{line}");
        if let Some(f) = sink.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    };
    let job: WriteJob = match std::fs::read_to_string(job_path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(j) => j,
        Err(e) => {
            emit(&Progress::msg(Phase::Error, format!("cannot read job: {e}")));
            return ExitCode::FAILURE;
        }
    };
    // The job file holds the Wi-Fi password: remove it as soon as it is loaded.
    let _ = std::fs::remove_file(job_path);
    let cancel = AtomicBool::new(false);
    let mut last_pct = u64::MAX;
    let r = job::run(
        &job,
        &mut |p: Progress| {
            // throttle: one line per percent per phase
            let pct = p.total.map(|t| p.bytes * 100 / t.max(1)).unwrap_or(0);
            if p.message.is_some() || pct != last_pct {
                last_pct = pct;
                emit(&p);
            }
        },
        &cancel,
    );
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            emit(&Progress::msg(Phase::Error, e.to_string()));
            ExitCode::FAILURE
        }
    }
}
