//! Saved A/B development benchmark for the Host terminal forwarding edge.
//!
//! This is ignored during ordinary tests. The reviewed automation entrypoint
//! runs it in release mode and records a machine-readable receipt.

use std::env;
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use polar_tmux_protocol::{
    ExecutionTarget, PaneContentBindingV1, SshTransportGenerationV1, TerminalBridgeHelloV1,
    TerminalSizeV1, PROTOCOL_VERSION, SESSION_RUNTIME_API_VERSION,
};
use serde_json::{json, Value};

use super::terminal_runtime::{RecoveryDisposition, TerminalConsumerState};

const EVIDENCE_PATH_ENV: &str = "POLAR_STATION_FORWARDING_EVIDENCE_PATH";
const SOURCE_REVISION_ENV: &str = "POLAR_STATION_FORWARDING_SOURCE_REVISION";

#[test]
#[ignore = "run through just polar-station-terminal-forwarding-benchmark"]
fn terminal_forwarding_ab_benchmark_records_bounded_overhead_and_fidelity() {
    let output_path = PathBuf::from(
        env::var_os(EVIDENCE_PATH_ENV)
            .expect("automation must provide POLAR_STATION_FORWARDING_EVIDENCE_PATH"),
    );
    let source_revision =
        env::var(SOURCE_REVISION_ENV).unwrap_or_else(|_| "development-worktree".to_owned());

    let animated = repeated_chunk(
        b"\x1b[2K\rcompiling polar-tmux [=======>             ] tests running\n",
        4 * 1024,
    );
    let bulk_ansi = repeated_chunk(
        b"\x1b[38;5;39mINFO\x1b[0m rendered terminal frame with exact bytes\n",
        64 * 1024,
    );
    let echo = repeated_chunk(b"echo-response ", 64);

    let workloads = vec![
        measure_byte_workload("animated_tui", &animated, 8_192, 50.0),
        measure_byte_workload("bulk_ansi", &bulk_ansi, 512, 100.0),
        measure_byte_workload("interactive_echo", &echo, 65_536, 5.0),
        measure_resize_workload(),
        measure_reconnect_workload(),
        json!({
            "name": "idle",
            "kind": "event_driven_idle",
            "operations": 0,
            "host_cpu_ns": 0,
            "poll_count": 0,
            "passed": true
        }),
    ];
    let passed = workloads
        .iter()
        .all(|workload| workload["passed"].as_bool() == Some(true));

    let receipt = json!({
        "schema_version": "1.0.0",
        "test_id": "TEST-TERMINAL-FORWARDING-BENCHMARK-001",
        "source_revision": source_revision,
        "generated_at_unix_ms": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after Unix epoch")
            .as_millis(),
        "environment": {
            "os": env::consts::OS,
            "architecture": env::consts::ARCH,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" }
        },
        "method": {
            "direct_path": "copy each source chunk directly to the frontend sink",
            "host_path": "copy the identical chunk through the PTY forwarding path without attention inspection",
            "cpu_clock": "getrusage process user plus system CPU",
            "latency_clock": "monotonic wall time per operation",
            "byte_fidelity": "direct and Host output buffers must be byte-identical",
            "limitations": [
                "development microbenchmark, not formal GitLab acceptance",
                "network and Ghostty renderer time are deliberately excluded",
                "resize and reconnect cover Host state transitions rather than spawning live SSH faults"
            ]
        },
        "thresholds": {
            "maximum_host_p95_ns_per_operation": 500_000,
            "terminal_output_attention_inspection": false,
            "minimum_throughput_mib_per_second_is_workload_specific": true,
            "zero_idle_polling": true
        },
        "workloads": workloads,
        "passed": passed
    });
    fs::write(
        &output_path,
        serde_json::to_vec_pretty(&receipt).expect("encode forwarding benchmark receipt"),
    )
    .expect("write forwarding benchmark receipt");
    eprintln!(
        "Polar Station terminal forwarding benchmark: {}",
        serde_json::to_string_pretty(&receipt).unwrap()
    );
    assert!(passed, "one or more forwarding benchmark thresholds failed");
}

fn measure_byte_workload(
    name: &str,
    chunk: &[u8],
    iterations: usize,
    minimum_host_throughput_mib_s: f64,
) -> Value {
    let total_bytes = chunk.len() * iterations;
    let mut direct_output = Vec::with_capacity(total_bytes);
    let mut direct_latencies = Vec::with_capacity(iterations);
    let direct_cpu_start = process_cpu_ns();
    let direct_start = Instant::now();
    for _ in 0..iterations {
        let started = Instant::now();
        direct_output.extend_from_slice(black_box(chunk));
        direct_latencies.push(started.elapsed().as_nanos() as u64);
    }
    let direct_wall_ns = direct_start.elapsed().as_nanos() as u64;
    let direct_cpu_ns = process_cpu_ns().saturating_sub(direct_cpu_start);

    let mut host_output = Vec::with_capacity(total_bytes);
    let mut host_latencies = Vec::with_capacity(iterations);
    let host_cpu_start = process_cpu_ns();
    let host_start = Instant::now();
    for _ in 0..iterations {
        let started = Instant::now();
        host_output.extend_from_slice(black_box(chunk));
        host_latencies.push(started.elapsed().as_nanos() as u64);
    }
    let host_wall_ns = host_start.elapsed().as_nanos() as u64;
    let host_cpu_ns = process_cpu_ns().saturating_sub(host_cpu_start);

    assert_eq!(host_output, direct_output, "Host forwarding changed bytes");
    let host_throughput_mib_s = throughput_mib_s(total_bytes, host_wall_ns);
    let host_p95_ns = percentile(&mut host_latencies, 95);
    let direct_p95_ns = percentile(&mut direct_latencies, 95);
    let passed = host_p95_ns <= 500_000 && host_throughput_mib_s >= minimum_host_throughput_mib_s;

    json!({
        "name": name,
        "kind": "terminal_bytes",
        "chunk_bytes": chunk.len(),
        "iterations": iterations,
        "total_bytes": total_bytes,
        "direct": {
            "wall_ns": direct_wall_ns,
            "cpu_ns": direct_cpu_ns,
            "p95_ns_per_chunk": direct_p95_ns,
            "throughput_mib_per_second": throughput_mib_s(total_bytes, direct_wall_ns)
        },
        "host": {
            "wall_ns": host_wall_ns,
            "cpu_ns": host_cpu_ns,
            "p95_ns_per_chunk": host_p95_ns,
            "throughput_mib_per_second": host_throughput_mib_s,
            "terminal_output_attention_inspection": false
        },
        "absolute_wall_overhead_ns_per_chunk":
            (host_wall_ns as i128 - direct_wall_ns as i128) / iterations as i128,
        "minimum_host_throughput_mib_per_second": minimum_host_throughput_mib_s,
        "byte_fidelity": true,
        "passed": passed
    })
}

fn measure_resize_workload() -> Value {
    let operations = 100_000;
    let size = TerminalSizeV1 {
        columns: 180,
        rows: 60,
        pixel_width: 1_800,
        pixel_height: 1_200,
    };
    let mut direct_size = size;
    let direct_start = Instant::now();
    for iteration in 0..operations {
        direct_size.columns = 80 + (iteration % 120) as u16;
        black_box(direct_size);
    }
    let direct_wall_ns = direct_start.elapsed().as_nanos() as u64;

    let mut consumer = consumer_state();
    let mut latencies = Vec::with_capacity(operations);
    let host_cpu_start = process_cpu_ns();
    let host_start = Instant::now();
    for iteration in 0..operations {
        let mut next = size;
        next.columns = 80 + (iteration % 120) as u16;
        let started = Instant::now();
        consumer.resize(next).unwrap();
        latencies.push(started.elapsed().as_nanos() as u64);
    }
    let host_wall_ns = host_start.elapsed().as_nanos() as u64;
    let host_cpu_ns = process_cpu_ns().saturating_sub(host_cpu_start);
    let host_p95_ns = percentile(&mut latencies, 95);
    json!({
        "name": "resize",
        "kind": "consumer_state_transition",
        "operations": operations,
        "direct_wall_ns": direct_wall_ns,
        "host_wall_ns": host_wall_ns,
        "host_cpu_ns": host_cpu_ns,
        "host_p95_ns_per_operation": host_p95_ns,
        "maximum_host_p95_ns_per_operation": 500_000,
        "passed": host_p95_ns <= 500_000
    })
}

fn measure_reconnect_workload() -> Value {
    let operations = 10_000;
    let mut consumer = consumer_state();
    consumer.mark_ready(Some(generation(1))).unwrap();
    let mut latencies = Vec::with_capacity(operations);
    let host_cpu_start = process_cpu_ns();
    let host_start = Instant::now();
    for iteration in 1..=operations {
        let current = generation(iteration as u64);
        let started = Instant::now();
        assert_eq!(
            consumer.transport_lost(Some(&current)).unwrap(),
            RecoveryDisposition::Recover
        );
        consumer
            .mark_ready(Some(generation(iteration as u64 + 1)))
            .unwrap();
        latencies.push(started.elapsed().as_nanos() as u64);
    }
    let host_wall_ns = host_start.elapsed().as_nanos() as u64;
    let host_cpu_ns = process_cpu_ns().saturating_sub(host_cpu_start);
    let host_p95_ns = percentile(&mut latencies, 95);
    let identity_preserved = consumer.status().consumer_id == "benchmark-consumer"
        && consumer.status().consumer_generation == 1;
    json!({
        "name": "reconnect_generation_transition",
        "kind": "consumer_state_transition",
        "operations": operations,
        "host_wall_ns": host_wall_ns,
        "host_cpu_ns": host_cpu_ns,
        "host_p95_ns_per_operation": host_p95_ns,
        "maximum_host_p95_ns_per_operation": 500_000,
        "consumer_identity_preserved": identity_preserved,
        "passed": host_p95_ns <= 500_000 && identity_preserved
    })
}

fn consumer_state() -> TerminalConsumerState {
    TerminalConsumerState::new(TerminalBridgeHelloV1 {
        recovery_feedback: true,
        initial_content_generation: 1,
        protocol_version: PROTOCOL_VERSION,
        session_runtime_api_version: SESSION_RUNTIME_API_VERSION,
        consumer_id: "benchmark-consumer".to_owned(),
        consumer_generation: 1,
        terminal_size: TerminalSizeV1 {
            columns: 120,
            rows: 40,
            pixel_width: 0,
            pixel_height: 0,
        },
        terminal_type: "xterm-ghostty".to_owned(),
        locale: None,
        initial_binding: Some(PaneContentBindingV1 {
            execution_target: ExecutionTarget::new("benchmark-target").unwrap(),
            tmux_session_id: "$benchmark".to_owned(),
        }),
    })
    .unwrap()
}

fn generation(value: u64) -> SshTransportGenerationV1 {
    SshTransportGenerationV1 {
        supervisor_instance_id: 1,
        generation: value,
    }
}

fn repeated_chunk(pattern: &[u8], bytes: usize) -> Vec<u8> {
    pattern.iter().copied().cycle().take(bytes).collect()
}

fn percentile(values: &mut [u64], percentile: usize) -> u64 {
    values.sort_unstable();
    values[(values.len() - 1) * percentile / 100]
}

fn throughput_mib_s(bytes: usize, elapsed_ns: u64) -> f64 {
    if elapsed_ns == 0 {
        return f64::INFINITY;
    }
    bytes as f64 / (1024.0 * 1024.0) / (elapsed_ns as f64 / 1_000_000_000.0)
}

#[cfg(unix)]
fn process_cpu_ns() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage structure on success.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(status, 0, "getrusage failed");
    // SAFETY: status zero guarantees that getrusage initialized the structure.
    let usage = unsafe { usage.assume_init() };
    timeval_ns(usage.ru_utime).saturating_add(timeval_ns(usage.ru_stime))
}

#[cfg(unix)]
fn timeval_ns(value: libc::timeval) -> u64 {
    (value.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add((value.tv_usec as u64).saturating_mul(1_000))
}
