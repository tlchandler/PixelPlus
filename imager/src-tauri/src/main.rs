// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `pixelplus-imager --helper write JOB.json --progress FILE` is how the app re-runs
    // itself with administrator rights (pkexec / osascript / already elevated on Windows)
    // to write the card. No GUI is created in that mode.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--helper") {
        std::process::exit(pixelplus_imager_core::helper::main_with_args(&args[1..]));
    }
    pixelplus_imager_lib::run();
}
