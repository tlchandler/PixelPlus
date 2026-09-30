//! Leader-side cache of `.ppseq` node slices (ARCHITECTURE §7.3).
//!
//! A slice depends on exactly two things: the source `.fseq` and the node's
//! pixel routing ([`NodeMap`]). Both are hashed into the **slice key**, which
//! names the cached file, is the HTTP ETag and is what followers compare to
//! decide whether to download. Editing anything that does not change the
//! routing (names, playlists, other nodes' props, colour order…) keeps the key,
//! so followers are not asked to re-download gigabytes for a rename.
//!
//! Files live in `<data>/cluster/slices/<nodeId>/<seqId>.<key>.ppseq`. They are
//! generated lazily on request and eagerly in the background after show
//! changes; stale ones are deleted.

use pixelplus_core::mapping::NodeMap;
use pixelplus_core::model::{Sequence, Show};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum SliceError {
    #[error("{0} is not part of this show")]
    UnknownNode(String),
    #[error("sequence {0} does not exist")]
    UnknownSequence(String),
    #[error("the sequence file {0} is missing on the leader")]
    MissingFile(String),
    #[error("could not build the slice: {0}")]
    Generate(String),
}

/// Metadata of a ready slice file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceMeta {
    pub key: String,
    pub path: PathBuf,
    pub bytes: u64,
    /// sha256 of the slice file (hex), verified by followers after download.
    pub sha256: String,
}

/// Everything needed to produce one slice.
#[derive(Debug, Clone)]
pub struct SliceJob {
    pub node_id: String,
    pub seq_id: String,
    pub key: String,
    pub fseq: PathBuf,
    pub map: Arc<NodeMap>,
}

/// Hash of a node's pixel routing (output sizes + copy runs).
pub fn mapping_hash(map: &NodeMap) -> String {
    let mut h = Sha256::new();
    h.update(b"nodemap-v1");
    h.update((map.pixels_per_output().len() as u32).to_le_bytes());
    for p in map.pixels_per_output() {
        h.update(p.to_le_bytes());
    }
    for r in map.runs() {
        h.update(r.src.to_le_bytes());
        h.update(r.dst.to_le_bytes());
        h.update(r.pixels.to_le_bytes());
        h.update([r.reverse as u8]);
    }
    pixelplus_core::fseq::to_hex(&h.finalize())[..32].to_string()
}

/// Identity of a sequence's source data: its sha256, or size+mtime when the
/// hash has not been recorded. `None` when the file is missing.
pub fn source_id(seq: &Sequence, fseq_path: &Path) -> Option<String> {
    let meta = std::fs::metadata(fseq_path).ok()?;
    if !seq.hash.is_empty() {
        return Some(seq.hash.clone());
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(format!("{}-{mtime}", meta.len()))
}

/// The slice key for a source and mapping.
pub fn slice_key(source_id: &str, mapping_hash: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!(
        "ppsq-v{}|{source_id}|{mapping_hash}",
        pixelplus_core::ppseq::VERSION
    ));
    pixelplus_core::fseq::to_hex(&h.finalize())[..40].to_string()
}

/// Ids used in file names must be plain.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Describe the slice for (`node_id`, `seq_id`) in the current show.
pub fn job(
    show: &Show,
    data_dir: &Path,
    node_id: &str,
    seq_id: &str,
) -> Result<SliceJob, SliceError> {
    let map =
        NodeMap::build(show, node_id).map_err(|_| SliceError::UnknownNode(node_id.to_string()))?;
    job_with_map(show, data_dir, Arc::new(map), seq_id)
}

fn job_with_map(
    show: &Show,
    data_dir: &Path,
    map: Arc<NodeMap>,
    seq_id: &str,
) -> Result<SliceJob, SliceError> {
    let seq = show
        .sequence(seq_id)
        .ok_or_else(|| SliceError::UnknownSequence(seq_id.to_string()))?;
    if !safe_id(&map.node_id) || !safe_id(seq_id) {
        return Err(SliceError::UnknownSequence(seq_id.to_string()));
    }
    let fseq = data_dir.join(&seq.file);
    let source = source_id(seq, &fseq).ok_or_else(|| SliceError::MissingFile(seq.file.clone()))?;
    let key = slice_key(&source, &mapping_hash(&map));
    Ok(SliceJob {
        node_id: map.node_id.clone(),
        seq_id: seq_id.to_string(),
        key,
        fseq,
        map,
    })
}

/// Every slice the followers of `show` need right now.
pub fn all_jobs(show: &Show, data_dir: &Path) -> Vec<SliceJob> {
    let mut jobs = Vec::new();
    for node in show
        .nodes
        .iter()
        .filter(|n| n.role == pixelplus_core::model::NodeRole::Follower && n.adopted)
    {
        let Ok(map) = NodeMap::build(show, &node.id) else {
            continue;
        };
        let map = Arc::new(map);
        for seq in &show.sequences {
            if let Ok(job) = job_with_map(show, data_dir, map.clone(), &seq.id) {
                jobs.push(job);
            }
        }
    }
    jobs
}

type Slot = Arc<tokio::sync::Mutex<Option<SliceMeta>>>;

/// The on-disk slice cache.
#[derive(Clone)]
pub struct SliceCache {
    root: PathBuf,
    slots: Arc<parking_lot::Mutex<HashMap<String, Slot>>>,
}

impl SliceCache {
    pub fn new(root: PathBuf) -> Self {
        SliceCache {
            root,
            slots: Default::default(),
        }
    }

    pub fn path_for(&self, job: &SliceJob) -> PathBuf {
        self.root
            .join(&job.node_id)
            .join(format!("{}.{}.ppseq", job.seq_id, job.key))
    }

    fn slot(&self, key: &str) -> Slot {
        self.slots
            .lock()
            .entry(key.to_string())
            .or_default()
            .clone()
    }

    /// Return the slice, generating it first if needed. Concurrent callers for
    /// the same key wait for one generation.
    pub async fn ensure(&self, job: SliceJob) -> Result<SliceMeta, SliceError> {
        let slot = self.slot(&job.key);
        let mut guard = slot.lock().await;
        if let Some(meta) = guard.as_ref() {
            if meta.path.exists() {
                return Ok(meta.clone());
            }
        }
        let path = self.path_for(&job);
        let key = job.key.clone();
        let meta = tokio::task::spawn_blocking(move || -> Result<SliceMeta, SliceError> {
            if !path.exists() {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)
                        .map_err(|e| SliceError::Generate(e.to_string()))?;
                }
                if !job.fseq.exists() {
                    return Err(SliceError::MissingFile(job.fseq.display().to_string()));
                }
                let started = std::time::Instant::now();
                pixelplus_core::ppseq::write_slice_from_path(&job.fseq, &job.map, &path)
                    .map_err(|e| SliceError::Generate(e.to_string()))?;
                tracing::debug!(
                    "built slice {} for node {} in {:?}",
                    job.seq_id,
                    job.node_id,
                    started.elapsed()
                );
            }
            let bytes = std::fs::metadata(&path)
                .map_err(|e| SliceError::Generate(e.to_string()))?
                .len();
            let sha256 = pixelplus_core::fseq::sha256_file(&path)
                .map_err(|e| SliceError::Generate(e.to_string()))?;
            Ok(SliceMeta {
                key,
                path,
                bytes,
                sha256,
            })
        })
        .await
        .map_err(|e| SliceError::Generate(e.to_string()))??;
        *guard = Some(meta.clone());
        Ok(meta)
    }

    /// Like [`ensure`](Self::ensure) but gives up waiting after `wait`
    /// (generation continues in the background). `Ok(None)` = not ready yet.
    pub async fn ensure_within(
        &self,
        job: SliceJob,
        wait: Duration,
    ) -> Result<Option<SliceMeta>, SliceError> {
        let this = self.clone();
        let task = tokio::spawn(async move { this.ensure(job).await });
        match tokio::time::timeout(wait, task).await {
            Ok(Ok(r)) => r.map(Some),
            Ok(Err(e)) => Err(SliceError::Generate(e.to_string())),
            Err(_) => Ok(None),
        }
    }

    /// Delete cached slices that no current job needs.
    pub fn cleanup(&self, jobs: &[SliceJob]) -> usize {
        let keep: HashSet<PathBuf> = jobs.iter().map(|j| self.path_for(j)).collect();
        let keys: HashSet<&str> = jobs.iter().map(|j| j.key.as_str()).collect();
        self.slots.lock().retain(|k, _| keys.contains(k.as_str()));
        let mut removed = 0;
        let Ok(nodes) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        for node_dir in nodes.flatten() {
            let dir = node_dir.path();
            if !dir.is_dir() {
                continue;
            }
            if let Ok(files) = std::fs::read_dir(&dir) {
                for f in files.flatten() {
                    let p = f.path();
                    if !keep.contains(&p) && std::fs::remove_file(&p).is_ok() {
                        removed += 1;
                    }
                }
            }
            // Remove directories of nodes that have no slices left.
            let _ = std::fs::remove_dir(&dir);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::manifest::tests::{prop, seg, three_node_show};
    use pixelplus_core::fseq::{FseqWriter, FseqWriterOptions};

    pub fn write_test_fseq(path: &Path, channels: u32, frames: u32) {
        let mut w = FseqWriter::create(path, FseqWriterOptions::new(channels, 25)).unwrap();
        let mut frame = vec![0u8; channels as usize];
        for f in 0..frames {
            for (i, b) in frame.iter_mut().enumerate() {
                *b = (i as u32 * 7 + f * 13) as u8;
            }
            w.write_frame(&frame).unwrap();
        }
        w.finish().unwrap();
    }

    fn with_sequence(dir: &Path) -> Show {
        let mut show = three_node_show();
        std::fs::create_dir_all(dir.join("sequences")).unwrap();
        write_test_fseq(&dir.join("sequences/s1.fseq"), 120, 50);
        show.sequences.push(Sequence {
            id: "s1".into(),
            name: "Song".into(),
            file: "sequences/s1.fseq".into(),
            duration_ms: 1250,
            frame_ms: 25,
            channel_count: 120,
            media_id: None,
            xlights_name: None,
            thumbnail: None,
            hash: "abc".into(),
        });
        show
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pp-slices-{}", pixelplus_core::model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn key_changes_only_with_routing_or_source() {
        let dir = tempdir();
        let show = with_sequence(&dir);
        let k = |s: &Show, node: &str| job(s, &dir, node, "s1").unwrap().key;
        let base_f1 = k(&show, "f1");
        let base_f2 = k(&show, "f2");
        assert_ne!(base_f1, base_f2);

        // Renames, colour order and props elsewhere do not matter.
        let mut s = show.clone();
        s.name = "Renamed".into();
        s.props[1].name = "B!".into();
        s.nodes[1].outputs[0].color_order = pixelplus_core::model::ColorOrder::GRB;
        s.props.push(prop("z", 5, 200, vec![seg("f2", 1, 0, 5, 0)]));
        assert_eq!(k(&s, "f1"), base_f1);
        assert_ne!(k(&s, "f2"), base_f2, "f2 gained a prop");

        // Moving a segment or the channel start does.
        let mut s = show.clone();
        s.props[1].segments[0].start_pixel = 3;
        assert_ne!(k(&s, "f1"), base_f1);
        let mut s = show.clone();
        s.props[1].channel_start = 33;
        assert_ne!(k(&s, "f1"), base_f1);
        let mut s = show.clone();
        s.props[1].segments[0].reverse = true;
        assert_ne!(k(&s, "f1"), base_f1);

        // New sequence data does.
        let mut s = show.clone();
        s.sequences[0].hash = "def".into();
        assert_ne!(k(&s, "f1"), base_f1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_files_and_unknown_ids() {
        let dir = tempdir();
        let mut show = with_sequence(&dir);
        assert!(matches!(
            job(&show, &dir, "nope", "s1"),
            Err(SliceError::UnknownNode(_))
        ));
        assert!(matches!(
            job(&show, &dir, "f1", "nope"),
            Err(SliceError::UnknownSequence(_))
        ));
        show.sequences[0].file = "sequences/gone.fseq".into();
        assert!(matches!(
            job(&show, &dir, "f1", "s1"),
            Err(SliceError::MissingFile(_))
        ));
        assert!(all_jobs(&show, &dir).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn generates_once_and_cleans_up_stale_slices() {
        let dir = tempdir();
        let show = with_sequence(&dir);
        let cache = SliceCache::new(dir.join("cluster/slices"));
        let jobs = all_jobs(&show, &dir);
        assert_eq!(jobs.len(), 2, "two followers × one sequence");

        let a = cache.ensure(jobs[0].clone()).await.unwrap();
        let b = cache.ensure(jobs[0].clone()).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.sha256,
            pixelplus_core::fseq::sha256_file(&a.path).unwrap()
        );
        let f = pixelplus_core::ppseq::PpseqFile::open(&a.path).unwrap();
        assert_eq!(f.frame_count(), 50);
        assert_eq!(
            f.header().source_sha256_hex(),
            pixelplus_core::fseq::sha256_file(dir.join("sequences/s1.fseq")).unwrap()
        );

        // Concurrent requests for the other slice share one generation.
        let (x, y) = tokio::join!(cache.ensure(jobs[1].clone()), cache.ensure(jobs[1].clone()));
        assert_eq!(x.unwrap(), y.unwrap());

        // Re-route f1: its old slice becomes stale and is removed.
        let mut s2 = show.clone();
        s2.props[1].segments[0].start_pixel = 10;
        let jobs2 = all_jobs(&s2, &dir);
        let new_f1 = cache.ensure(jobs2[0].clone()).await.unwrap();
        assert_ne!(new_f1.path, a.path);
        assert_eq!(cache.cleanup(&jobs2), 1);
        assert!(!a.path.exists());
        assert!(new_f1.path.exists());

        // A cache that lost its memory (restart) finds files on disk again.
        let cache2 = SliceCache::new(dir.join("cluster/slices"));
        let again = cache2
            .ensure_within(jobs2[0].clone(), Duration::from_secs(5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again, new_f1);
        std::fs::remove_dir_all(dir).ok();
    }
}
