//! Sikt against the `regex` crate on equivalent searches over a log-like text.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

/// About 1 MB of log lines, deterministic.
fn haystack() -> String {
    let levels = ["INFO", "DEBUG", "WARN", "INFO", "ERROR", "INFO"];
    let users = ["alice", "bob", "carol", "dave", "eve"];
    let mut out = String::new();
    let mut i = 0u32;
    while out.len() < 1 << 20 {
        let level = levels[i as usize % levels.len()];
        let user = users[(i / 3) as usize % users.len()];
        out.push_str(&format!(
            "2026-10-{:02} 12:{:02}:{:02} [{level}] request user={user} ms={} path=/api/v1/items/{}\n",
            i % 28 + 1,
            i % 60,
            (i * 7) % 60,
            (i * 37) % 1000,
            i % 500,
        ));
        i += 1;
    }
    out
}

struct Case {
    name: &'static str,
    sikt: &'static str,
    regex: &'static str,
    capture: Option<&'static str>,
}

const CASES: &[Case] = &[
    Case {
        name: "literal",
        sikt: r#""ERROR""#,
        regex: "ERROR",
        capture: None,
    },
    Case {
        name: "date",
        sikt: r#"4 digit "-" 2 digit "-" 2 digit"#,
        regex: r"[0-9]{4}-[0-9]{2}-[0-9]{2}",
        capture: None,
    },
    Case {
        name: "key_value",
        sikt: r#"key: 1+ wordchar "=" value: 1+ wordchar"#,
        regex: r"(?P<key>\w+)=(?P<value>\w+)",
        capture: Some("value"),
    },
    Case {
        name: "bracketed",
        sikt: r#""[" level: until "]" "]""#,
        regex: r"\[(?P<level>[^\]]*)\]",
        capture: Some("level"),
    },
];

fn bench(c: &mut Criterion) {
    let hay = haystack();
    for case in CASES {
        let mut group = c.benchmark_group(case.name);
        group.throughput(Throughput::Bytes(hay.len() as u64));

        let p = sikt::Pattern::new(case.sikt).unwrap();
        let r = regex::Regex::new(case.regex).unwrap();
        let sikt_count = p.find_iter(&hay).count();
        let regex_count = r.find_iter(&hay).count();
        assert_eq!(
            sikt_count, regex_count,
            "{}: match counts differ",
            case.name
        );

        match case.capture {
            None => {
                group.bench_function("sikt", |b| b.iter(|| p.find_iter(black_box(&hay)).count()));
                group.bench_function("regex", |b| b.iter(|| r.find_iter(black_box(&hay)).count()));
            }
            Some(name) => {
                group.bench_function("sikt", |b| {
                    b.iter(|| {
                        p.find_iter(black_box(&hay))
                            .filter_map(|m| m.unwrap().get(name))
                            .count()
                    })
                });
                group.bench_function("regex", |b| {
                    b.iter(|| {
                        r.captures_iter(black_box(&hay))
                            .filter_map(|c| c.name(name))
                            .count()
                    })
                });
            }
        }
        group.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
