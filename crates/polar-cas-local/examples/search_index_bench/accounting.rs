//! Benchmark-only accounting, always outside the timed lookup interval.
use serde::Serialize;
use serde_json::json;
use std::{collections::BTreeMap, fs, io, os::unix::fs::MetadataExt, path::Path, time::Instant};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Space {
    pub file_paths: u64,
    pub unique_inodes: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub path_logical_bytes: u64,
    pub unique_logical_bytes: u64,
    pub allocated_bytes: u64,
}

pub fn inventory(root: &Path) -> io::Result<Space> {
    let mut result = Space::default();
    let mut inodes = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_symlink() {
            result.symlinks += 1;
        } else if metadata.is_dir() {
            result.directories += 1;
            for entry in fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if metadata.is_file() {
            result.file_paths += 1;
            result.path_logical_bytes += metadata.len();
            inodes.insert(
                (metadata.dev(), metadata.ino()),
                (metadata.len(), metadata.blocks() * 512),
            );
        }
    }
    result.unique_inodes = inodes.len() as u64;
    for (logical, allocated) in inodes.values() {
        result.unique_logical_bytes += logical;
        result.allocated_bytes += allocated;
    }
    Ok(result)
}

pub fn space(layer: &str, phase: &str, rows: usize, root: &Path) -> Space {
    let value = inventory(root).unwrap();
    println!(
        "{}",
        json!({"kind":"index_space", "layer":layer, "phase":phase,
        "rows":rows,"scope":"entire_private_fixture_root_including_old_and_temporary_files",
        "space":value,"bytes_per_row":value.unique_logical_bytes as f64 / rows.max(1) as f64,
        "allocation_scope":"inode blocks; shared extents not deduplicated",
        "transient_peak_bytes":null})
    );
    value
}

pub fn active(
    layer: &str,
    reference: &polar_cas::ContentRef,
    partitions: &BTreeMap<u8, polar_cas_local::shared_index::partitions::PartitionRef>,
    rows: usize,
    phase: &str,
) {
    let mut files = BTreeMap::new();
    files.insert(reference.id.to_string(), reference.size_bytes);
    for reference in partitions.values() {
        let file = reference.portable_file();
        files.insert(file.id.to_string(), file.size_bytes);
        if reference.storage.is_none() {
            files.insert(reference.native.id.to_string(), reference.native.size_bytes);
        }
    }
    println!(
        "{}",
        json!({"kind":"index_active_snapshot","layer":layer,"phase":phase,
        "rows":rows,"root_and_segment_files":files.len(),"unique_logical_bytes":files.values().sum::<u64>(),
        "logical_partitions":partitions.len(),"root":reference})
    );
}

fn counters(path: &str) -> BTreeMap<String, u64> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.into(), value.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}

pub struct Phase {
    start: Instant,
    io: BTreeMap<String, u64>,
    cpu: Option<(u64, u64)>,
}
fn cpu_ticks() -> Option<(u64, u64)> {
    let stat = fs::read_to_string("/proc/self/stat").ok()?;
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(11)?.parse().ok()?, fields.get(12)?.parse().ok()?))
}
impl Phase {
    pub fn start() -> Self {
        Self {
            io: counters("/proc/self/io"),
            cpu: cpu_ticks(),
            start: Instant::now(),
        }
    }
    pub fn finish(self, layer: &str, phase: &str, rows: usize) -> u128 {
        let ns = self.start.elapsed().as_nanos();
        let after = counters("/proc/self/io");
        let delta: BTreeMap<_, _> = self
            .io
            .iter()
            .filter_map(|(k, v)| after.get(k).map(|n| (k, n.saturating_sub(*v))))
            .collect();
        let status = counters("/proc/self/status");
        let cpu = self
            .cpu
            .zip(cpu_ticks())
            .map(|(a, b)| (b.0.saturating_sub(a.0), b.1.saturating_sub(a.1)));
        println!(
            "{}",
            json!({"kind":"index_phase", "layer":layer,"phase":phase,
            "rows":rows,"wall_ns":ns,"process_io_delta":delta,"process_cpu_user_system_ticks":cpu,
            "rss_kib":status.get("VmRSS"),"process_lifetime_peak_rss_kib":status.get("VmHWM"),
            "stage_peak_rss_kib":null,"decoded_records":null,
            "note":"process I/O includes runtime and measurement overhead; peak is process lifetime, not stage"})
        );
        ns
    }
}

/// Sampling is opt-in for mutation phases, never the pure lookup hot loop.
pub struct PeakSampler {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: std::thread::JoinHandle<Peaks>,
}
#[derive(Debug, Default, Serialize)]
pub struct Peaks {
    pub samples: u64,
    pub raced_samples: u64,
    pub observed_max_logical_bytes: u64,
    pub observed_max_allocated_bytes: u64,
    pub observed_max_file_paths: u64,
    pub observed_max_rss_kib: u64,
}
impl PeakSampler {
    pub fn start(root: &Path) -> Self {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let root = root.to_path_buf();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut peaks = Peaks::default();
            loop {
                let stopping = thread_stop.load(Ordering::Acquire);
                match inventory(&root) {
                    Ok(value) => {
                        peaks.samples += 1;
                        peaks.observed_max_logical_bytes = peaks
                            .observed_max_logical_bytes
                            .max(value.unique_logical_bytes);
                        peaks.observed_max_allocated_bytes = peaks
                            .observed_max_allocated_bytes
                            .max(value.allocated_bytes);
                        peaks.observed_max_file_paths =
                            peaks.observed_max_file_paths.max(value.file_paths);
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => peaks.raced_samples += 1,
                    Err(e) => panic!("peak accounting: {e}"),
                }
                peaks.observed_max_rss_kib = peaks
                    .observed_max_rss_kib
                    .max(*counters("/proc/self/status").get("VmRSS").unwrap_or(&0));
                if stopping {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            peaks
        });
        Self { stop, thread }
    }
    pub fn finish(self, layer: &str, phase: &str, rows: usize) -> Peaks {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let peaks = self.thread.join().unwrap();
        println!(
            "{}",
            json!({"kind":"index_sampled_peaks","layer":layer,"phase":phase,"rows":rows,
            "peaks":peaks,"interval_ms":100,"scope":"whole private fixture and current process RSS",
            "exact_transient_peak":false,"note":"sampled maxima; sampler adds filesystem and CPU overhead to mutation phases"})
        );
        peaks
    }
}

pub fn latency(layer: &str, operation: &str, mut samples: Vec<u128>) {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let percentile = |percent: usize| samples[(samples.len() * percent).div_ceil(100) - 1];
    println!(
        "{}",
        json!({"kind":"index_latency","layer":layer,"operation":operation,
        "samples":samples.len(),"p50_ns":percentile(50),"p95_ns":percentile(95),
        "p99_ns":percentile(99),"timing":"separate instrumented run; timer cost included"})
    );
}
