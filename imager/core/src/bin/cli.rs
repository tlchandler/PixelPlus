//! `pixelplus-imager-cli` - command-line PixelPlus Imager (and the same elevated helper the
//! GUI uses).
//!
//! ```text
//! pixelplus-imager-cli list                                  # removable drives as JSON
//! pixelplus-imager-cli customize IMAGE.img SETTINGS.json     # write pixelplus.txt into an .img file
//! sudo pixelplus-imager-cli write JOB.json [--progress FILE] # write + verify + customise a card
//! ```
//! JOB.json: `{"image": "...img.xz", "device": "/dev/sdX", "settings": {...},
//! "extractSize": n, "extractSha256": "..."}` (see `job::WriteJob`); it is deleted after reading.

use std::process::ExitCode;

use pixelplus_imager_core::{drives, helper, job, ImagerSettings};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("list") => match drives::list() {
            Ok(d) => {
                println!("{}", serde_json::to_string_pretty(&d).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        Some("customize") if args.len() == 3 => {
            let parsed: Result<ImagerSettings, String> = std::fs::read_to_string(&args[2])
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
            match parsed.map_err(|e| e.to_string()).and_then(|s| {
                job::customize_image_file(std::path::Path::new(&args[1]), &s)
                    .map_err(|e| e.to_string())
            }) {
                Ok(()) => {
                    println!("pixelplus.txt written to {}", args[1]);
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Some("write") => helper::main_with_args(&args),
        _ => {
            eprintln!("usage: pixelplus-imager-cli list | customize IMAGE SETTINGS.json | write JOB.json [--progress FILE]");
            2
        }
    };
    ExitCode::from(code as u8)
}
