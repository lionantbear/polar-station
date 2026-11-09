//! Fixed search-only batch/concurrency matrix. Thread/connection/statement setup,
//! warm-up, barriers after completion and teardown are outside the measured span.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{path::Path, sync::Barrier};

pub fn sqlite(path: &Path, rows: &[LocatedContent]) {
    let mut conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE locations(k BLOB PRIMARY KEY, size INTEGER NOT NULL, kind INTEGER NOT NULL) WITHOUT ROWID;").unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx
            .prepare("INSERT INTO locations VALUES(?1,?2,?3)")
            .unwrap();
        for row in rows {
            insert
                .execute(rusqlite::params![
                    row.content.id.digest_bytes().as_slice(),
                    row.content.size_bytes as i64,
                    if matches!(row.location, PhysicalCandidate::Loose) {
                        1
                    } else {
                        2
                    }
                ])
                .unwrap();
        }
    }
    tx.commit().unwrap();
}

#[inline]
pub(super) fn chunks<const PROFILE: bool, F>(
    queries: &[ObjectId],
    batch: usize,
    output: &mut [Option<LocatedContent>],
    latencies: &mut Vec<u128>,
    mut f: F,
) -> u64
where
    F: FnMut(&[ObjectId], &mut [Option<LocatedContent>]) -> u64,
{
    let mut sum = 0u64;
    for chunk in queries.chunks(batch) {
        let start = if PROFILE { Some(Instant::now()) } else { None };
        sum = sum.wrapping_add(f(chunk, &mut output[..chunk.len()]));
        if let Some(start) = start {
            latencies.push(start.elapsed().as_nanos());
        }
    }
    black_box(sum)
}
struct Runner<'a, 'db> {
    method: &'a str,
    queries: &'a [ObjectId],
    batch: usize,
    index: &'a SearchIndex,
    native: &'a SearchIndex,
    pack: &'a ContentRef,
    statement: Option<rusqlite::Statement<'db>>,
    output: Vec<Option<LocatedContent>>,
    latencies: Vec<u128>,
}
impl Runner<'_, '_> {
    fn execute<const PROFILE: bool>(&mut self) -> u64 {
        let index = self.index;
        let native = self.native;
        let pack = self.pack;
        let stmt = &mut self.statement;
        let (qs, batch, out, times) = (
            self.queries,
            self.batch,
            &mut self.output,
            &mut self.latencies,
        );
        // Select the implementation once per whole trace, not once per request.
        match self.method {
            "binary" => chunks::<PROFILE, _>(qs, batch, out, times, |chunk, _| {
                chunk.iter().fold(0u64, |a, q| {
                    a.wrapping_add(token(&index.locate_binary(q).unwrap()))
                })
            }),
            "native_scalar" => chunks::<PROFILE, _>(qs, batch, out, times, |chunk, _| {
                chunk.iter().fold(0u64, |a, q| {
                    a.wrapping_add(token(&native.locate(q).unwrap()))
                })
            }),
            "native_batch" => chunks::<PROFILE, _>(qs, batch, out, times, |chunk, out| {
                native.locate_batch_into::<32>(chunk, out).unwrap();
                out.iter().fold(0u64, |a, v| a.wrapping_add(token(v)))
            }),
            "sqlite_scalar" => chunks::<PROFILE, _>(qs, batch, out, times, |chunk, _| {
                chunk.iter().fold(0u64, |a, q| {
                    let hit = stmt
                        .as_mut()
                        .unwrap()
                        .query_row([q.digest_bytes().as_slice()], |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                        })
                        .optional()
                        .unwrap();
                    let hit = hit.map(|(size, kind)| LocatedContent {
                        content: ContentRef {
                            id: q.clone(),
                            size_bytes: size as u64,
                        },
                        location: if kind == 1 {
                            PhysicalCandidate::Loose
                        } else {
                            PhysicalCandidate::PackMember { pack: pack.clone() }
                        },
                    });
                    a.wrapping_add(token(&hit))
                })
            }),
            "sqlite_batch" => chunks::<PROFILE, _>(qs, batch, out, times, |chunk, out| {
                let mut found = stmt
                    .as_mut()
                    .unwrap()
                    .query(rusqlite::params_from_iter(
                        chunk.iter().map(|q| q.digest_bytes().as_slice()),
                    ))
                    .unwrap();
                out.fill(None);
                while let Some(row) = found.next().unwrap() {
                    let i = row.get::<_, i64>(0).unwrap() as usize;
                    if let Some(size) = row.get::<_, Option<i64>>(1).unwrap() {
                        let kind: i64 = row.get(2).unwrap();
                        out[i] = Some(LocatedContent {
                            content: ContentRef {
                                id: chunk[i].clone(),
                                size_bytes: size as u64,
                            },
                            location: if kind == 1 {
                                PhysicalCandidate::Loose
                            } else {
                                PhysicalCandidate::PackMember { pack: pack.clone() }
                            },
                        });
                    }
                }
                out.iter().fold(0u64, |a, v| a.wrapping_add(token(v)))
            }),
            _ => unreachable!(),
        }
    }
}

pub fn run(index: &SearchIndex, native: &SearchIndex, path: &Path, n: usize, pack: &ContentRef) {
    for case in ["mixed", "half", "repeat"] {
        let source: Vec<_> = (0..131072usize)
            .map(|q| {
                let at = if case == "repeat" {
                    (q % 16) * 7919 % n
                } else {
                    q * 7919 % n
                };
                if (case == "mixed" && q % 5 == 4) || (case == "half" && q.is_multiple_of(2)) {
                    n + at
                } else {
                    at
                }
            })
            .collect();
        let queries: Vec<_> = source.iter().map(|k| id(*k)).collect();
        let trace = ObjectId::for_bytes(
            &queries
                .iter()
                .flat_map(|q| q.digest_bytes().iter().copied())
                .collect::<Vec<_>>(),
        );
        let expected = source.iter().fold(0u64, |a, k| {
            a.wrapping_add(if *k < n {
                ((*k + 100) as u64) ^ if k.is_multiple_of(2) { 1 } else { 2 }
            } else {
                0
            })
        });
        for workers in [1, 8, 32] {
            for batch in [1, 8, 64, 1024] {
                let methods = [
                    "binary",
                    "native_scalar",
                    "native_batch",
                    "sqlite_scalar",
                    "sqlite_batch",
                ];
                for sample in 0..8 {
                    for step in 0..methods.len() {
                        let method = methods[if sample % 2 == 0 {
                            step
                        } else {
                            methods.len() - 1 - step
                        }];
                        let ready = Barrier::new(workers + 1);
                        let start = Barrier::new(workers + 1);
                        let finish = Barrier::new(workers + 1);
                        let release = Barrier::new(workers + 1);
                        let (ns, checksum) = std::thread::scope(|scope| {
                            let jobs: Vec<_> = queries.chunks(queries.len()/workers).map(|qs| {
                                let (ready,start,finish,release) = (&ready,&start,&finish,&release);
                                scope.spawn(move || {
                                    let conn = if method.starts_with("sqlite") {
                                        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX).unwrap();
                                        conn.execute_batch("PRAGMA mmap_size=2147483648; PRAGMA cache_size=-65536; BEGIN;").unwrap();
                                        Some(conn)
                                    } else {None};
                                    let sql = if method=="sqlite_batch" {
                                        let values = (0..batch).map(|i|format!("({i},?{})",i+1)).collect::<Vec<_>>().join(",");
                                        format!("WITH q(i,k) AS (VALUES {values}) SELECT q.i,l.size,l.kind FROM q LEFT JOIN locations l ON l.k=q.k")
                                    } else { "SELECT size,kind FROM locations WHERE k=?1".into() };
                                    let stmt = conn.as_ref().map(|c|c.prepare(&sql).unwrap());
                                    let mut runner=Runner{method,queries:qs,batch,index,native,pack,statement:stmt,output:vec![None;batch],latencies:Vec::with_capacity(qs.len()/batch)};
                                    let warm=runner.execute::<false>();
                                    ready.wait(); start.wait();
                                    let sum=runner.execute::<false>();
                                    finish.wait(); release.wait();
                                    assert_eq!(sum,warm);
                                    if sample==0 && workers==1 {
                                        assert_eq!(runner.execute::<true>(),sum);
                                        let latencies=&mut runner.latencies;
                                        latencies.sort_unstable();
                                        println!("{}",json!({"kind":"latency","rows":n,"case":case,"method":method,"batch":batch,"workers":workers,"samples":latencies.len(),"p50_ns":latencies[latencies.len()/2],"p95_ns":latencies[latencies.len()*95/100],"p99_ns":latencies[latencies.len()*99/100],"timing":"separate_instrumented_run"}));
                                    }
                                    sum
                                })
                            }).collect();
                            ready.wait();
                            let now = Instant::now();
                            start.wait();
                            finish.wait();
                            let ns = now.elapsed().as_nanos();
                            release.wait();
                            let sum = jobs
                                .into_iter()
                                .fold(0u64, |a, j| a.wrapping_add(j.join().unwrap()));
                            (ns, sum)
                        });
                        assert_eq!(checksum, expected, "{method}");
                        println!(
                            "{}",
                            json!({"kind":"matrix","rows":n,"layout":"aligned64","case":case,"method":method,"sample":sample,"queries":queries.len(),"batch":batch,"workers":workers,"wall_ns":ns,"trace":trace.to_string(),"checksum":checksum,"cache":"warm"})
                        );
                    }
                }
            }
        }
    }
}
