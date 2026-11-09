//! Requests eviction only for these private fixture index files; never drops a
//! host cache. Report faults and startup separately: this is not full-host cold.
use super::*;
use std::fs::File;
fn faults() -> (u64, u64) {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    (fields[7].parse().unwrap(), fields[9].parse().unwrap())
}
pub fn run(grouped: &File, native: &File, n: usize) {
    let source: Vec<_> = (0..100000)
        .map(|q| {
            let at = q * 7919 % n;
            if q % 5 == 4 {
                n + at
            } else {
                at
            }
        })
        .collect();
    let queries: Vec<_> = source.iter().map(|k| id(*k)).collect();
    let expected = source.iter().fold(0u64, |sum, k| {
        sum.wrapping_add(if *k < n {
            ((*k + 100) as u64) ^ if k.is_multiple_of(2) { 1 } else { 2 }
        } else {
            0
        })
    });
    for sample in 0..8 {
        let methods = ["binary", "binary_batch", "native_scalar", "native_batch"];
        for step in 0..methods.len() {
            let method = methods[if sample % 2 == 0 {
                step
            } else {
                methods.len() - 1 - step
            }];
            let file = if method.starts_with("native") {
                native
            } else {
                grouped
            };
            rustix::fs::fadvise(file, 0, None, rustix::fs::Advice::DontNeed).unwrap();
            let faults0 = faults();
            let start = Instant::now();
            let index = SearchIndex::open(file, None).unwrap();
            let open_ns = start.elapsed().as_nanos();
            let mut output = vec![None; 1024];
            let mut sum = 0u64;
            let faults1 = faults();
            let start = Instant::now();
            for chunk in queries.chunks(1024) {
                match method {
                    "binary" | "native_scalar" => {
                        for q in chunk {
                            sum = sum.wrapping_add(token(
                                &if method == "binary" {
                                    index.locate_binary(q)
                                } else {
                                    index.locate(q)
                                }
                                .unwrap(),
                            ));
                        }
                    }
                    _ => {
                        let out = &mut output[..chunk.len()];
                        if method == "binary_batch" {
                            index.locate_binary_batch_into(chunk, out)
                        } else {
                            index.locate_batch_into::<32>(chunk, out)
                        }
                        .unwrap();
                        for hit in out {
                            sum = sum.wrapping_add(token(hit));
                        }
                    }
                }
            }
            let ns = start.elapsed().as_nanos();
            let faults2 = faults();
            assert_eq!(sum, expected);
            println!(
                "{}",
                json!({"kind":"cold","rows":n,"case":"mixed","sample":sample,"method":method,"queries":queries.len(),"batch":1024,"open_ns":open_ns,"wall_ns":ns,"checksum":sum,"startup_minor_faults":faults1.0-faults0.0,"startup_major_faults":faults1.1-faults0.1,"query_minor_faults":faults2.0-faults1.0,"query_major_faults":faults2.1-faults1.1,"cache":"private_file_fadvise_dontneed_fresh_mapping"})
            );
        }
    }
}
