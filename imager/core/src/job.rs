//! The privileged "write an SD card" job run by the helper process.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::device;
use crate::disk::{inject_settings, Aligned};
use crate::drives::{self, Drive};
use crate::settings::ImagerSettings;
use crate::write::{self, Phase, Progress, WriteError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteJob {
    pub image: PathBuf,
    pub device: String,
    #[serde(default)]
    pub settings: ImagerSettings,
    #[serde(default)]
    pub extract_size: Option<u64>,
    #[serde(default)]
    pub extract_sha256: Option<String>,
    #[serde(default = "yes")]
    pub verify: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("{0} is not a removable drive PixelPlus Imager may write to")]
    NotAllowed(String),
    #[error("the card is too small ({0}); use a card of at least 4 GB")]
    TooSmall(String),
    #[error("some settings are invalid: {0}")]
    Settings(String),
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    List(#[from] drives::ListError),
}

/// Re-check (as root) that the target is still a removable, non-system drive.
pub fn find_drive(device: &str) -> Result<Drive, JobError> {
    drives::list()?
        .into_iter()
        .find(|d| d.device == device)
        .ok_or_else(|| JobError::NotAllowed(device.to_string()))
}

pub fn run(
    job: &WriteJob,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), JobError> {
    let errs = job.settings.validate();
    if !errs.is_empty() {
        return Err(JobError::Settings(
            errs.iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    let drive = find_drive(&job.device)?;
    if drive.too_small {
        return Err(JobError::TooSmall(drives::human_size(drive.size)));
    }
    progress(Progress::msg(
        Phase::Prepare,
        format!("Preparing {}", drive.name),
    ));
    let mut dev = device::open(&drive)?;
    let out = write::write_image(
        &job.image,
        &mut dev.file,
        Some(dev.size),
        job.extract_size,
        job.extract_sha256.as_deref(),
        progress,
        cancel,
    )?;
    if job.verify {
        device::drop_caches(&dev)?;
        write::verify(&mut dev.file, out.bytes, &out.sha256, progress, cancel)?;
    }
    progress(Progress::msg(
        Phase::Customize,
        "Saving your settings (pixelplus.txt)",
    ));
    {
        let aligned = Aligned::new(&mut dev.file, dev.block);
        inject_settings(aligned, &job.settings)?;
    }
    device::finish(dev, &drive)?;
    progress(Progress::msg(Phase::Done, "Done"));
    Ok(())
}

/// Customise an image *file* in place (no device involved): `pixelplus-imager-cli customize`.
pub fn customize_image_file(path: &Path, settings: &ImagerSettings) -> Result<(), JobError> {
    let errs = settings.validate();
    if !errs.is_empty() {
        return Err(JobError::Settings(
            errs.iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join(" "),
        ));
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    inject_settings(&mut f, settings)?;
    f.flush()?;
    f.sync_all()?;
    Ok(())
}
