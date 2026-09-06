//! Production SQLite adapter, driven by a deterministic recorded-stroke schedule.
use anyhow::{Result, ensure};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

// Share the exact application model, validation and adapter; only the command error
// boundary and runtime scheduler are replaced. No Tauri, WebView or SDK is involved.
#[derive(Debug)]
pub struct CommandError(String);
impl CommandError {
    fn invalid(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    fn conflict(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}
impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for CommandError {}
type CommandResult<T> = std::result::Result<T, CommandError>;
fn err(e: impl std::fmt::Display) -> CommandError {
    CommandError(e.to_string())
}
#[path = "../../../../src-tauri/src/commands/handwriting/model.rs"]
mod model;
use model::*;
#[allow(dead_code, unused_imports)]
#[path = "../../../../src-tauri/src/commands/handwriting/storage.rs"]
mod storage;
#[cfg(test)]
fn write_draft(path: &Path, draft: InkDraft, revision: Option<String>) -> CommandResult<String> {
    validate(&draft)?;
    storage::patch(
        path,
        InkDraftPatch {
            order: draft.strokes.iter().map(|s| s.id).collect(),
            upserts: draft.strokes,
            background: draft.background,
        },
        revision,
    )
}
#[cfg(test)]
fn write_patch(
    path: &Path,
    patch: InkDraftPatch,
    revision: Option<String>,
) -> CommandResult<String> {
    storage::patch(path, patch, revision)
}
fn cpu_ms() -> f64 {
    unsafe {
        let mut t = std::mem::zeroed();
        assert_eq!(
            libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut t),
            0
        );
        t.tv_sec as f64 * 1000. + t.tv_nsec as f64 / 1e6
    }
}
fn proc_values(path: &str) -> BTreeMap<String, u64> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.into(), v.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}
#[derive(Default, Serialize)]
struct Timing {
    samples: usize,
    total_ms: f64,
    cpu_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}
fn timing(v: &[(f64, f64)]) -> Timing {
    if v.is_empty() {
        return Timing::default();
    }
    let mut w: Vec<_> = v.iter().map(|x| x.0).collect();
    w.sort_by(f64::total_cmp);
    Timing {
        samples: v.len(),
        total_ms: w.iter().sum(),
        cpu_ms: v.iter().map(|x| x.1).sum(),
        p95_ms: w[(w.len() * 95).div_ceil(100) - 1],
        max_ms: *w.last().unwrap(),
    }
}
fn measure<T>(f: impl FnOnce() -> CommandResult<T>, values: &mut Vec<(f64, f64)>) -> Result<T> {
    let t = Instant::now();
    let c = cpu_ms();
    let result = f()?;
    values.push((t.elapsed().as_secs_f64() * 1000., cpu_ms() - c));
    Ok(result)
}
fn equal(actual: &InkDraft, expected: &InkDraft, count: usize) -> Result<()> {
    ensure!(actual.strokes.len() == count);
    ensure!(actual.width == expected.width && actual.height == expected.height);
    ensure!(
        serde_json::to_value(&actual.background)?
            == serde_json::to_value(if count == 0 {
                &InkBackground::Plain
            } else {
                &expected.background
            })?
    );
    for (a, b) in actual.strokes.iter().zip(&expected.strokes) {
        ensure!(
            a.id == b.id
                && a.width.to_bits() == b.width.to_bits()
                && a.points.len() == b.points.len()
        );
        for (a, b) in a.points.iter().zip(&b.points) {
            for (x, y) in [a.x, a.y, a.pressure, a.tilt_x, a.tilt_y, a.time]
                .into_iter()
                .zip([b.x, b.y, b.pressure, b.tilt_x, b.tilt_y, b.time])
            {
                ensure!(x.to_bits() == y.to_bits());
            }
        }
    }
    Ok(())
}
fn sizes(path: &Path) -> Result<serde_json::Value> {
    let db = rusqlite::Connection::open(path)?;
    let (root, revision): (Vec<u8>, String) = db.query_row(
        "SELECT r.data,h.revision FROM ink_head h JOIN ink_records r ON r.id=h.root_id",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let record = ink_format::record::Record::decode(&root)?;
    use ink_format::model::Body;
    let doc = ink_format::model::Document::read(&record)?;
    let mut sums = BTreeMap::new();
    for table in ["ink_records", "ink_chunks"] {
        let row: (i64, i64) = db.query_row(
            &format!("SELECT count(*),coalesce(sum(length(data)),0) FROM {table}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        sums.insert(table, row);
    }
    Ok(
        serde_json::json!({"head_geometry_bytes":doc.chunks.values().map(|r|r.length).sum::<u64>(),"head_metadata_bytes":root.len() as u64+doc.records.values().map(|r|r.length).sum::<u64>(),"head_chunks":doc.chunks.len(),"retained":sums,"db_bytes":fs::metadata(path)?.len(),"revision":revision}),
    )
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    ensure!(
        a.len() >= 4,
        "compaction page.json NEW.db none|exit|SECONDS [speed:0=unpaced,1=realtime] [max_chunks=16] [decoded_MiB=128]"
    );
    let path = Path::new(&a[2]);
    ensure!(
        !path.exists()
            && !Path::new(&format!("{}-wal", a[2])).exists()
            && !Path::new(&format!("{}-shm", a[2])).exists(),
        "refusing existing database"
    );
    let page: InkDraft = serde_json::from_slice(&fs::read(&a[1])?)?;
    validate(&page)?;
    ensure!(!page.strokes.is_empty());
    let interval = match a[3].as_str() {
        "none" | "exit" => None,
        s => Some(s.parse::<f64>()? * 1000.),
    };
    if let Some(n) = interval {
        ensure!(n.is_finite() && n >= 1000.);
    }
    let speed = a
        .get(4)
        .map(|s| s.parse::<f64>())
        .transpose()?
        .unwrap_or(0.);
    ensure!(speed.is_finite() && speed >= 0.);
    let max_chunks = a
        .get(5)
        .map(|s| s.parse::<usize>())
        .transpose()?
        .unwrap_or(16);
    let decoded_mib = a
        .get(6)
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(128);
    ensure!((2..=512).contains(&max_chunks) && (16..=128).contains(&decoded_mib));
    let compact = || storage::compact_with_limits(path, max_chunks, decoded_mib * 1024 * 1024);
    storage::read(path)?; // schema creation excluded
    let first = page.strokes[0].points[0].time;
    let mut strokes = vec![];
    let mut packs = vec![];
    let mut exit_packs = vec![];
    let mut order = vec![];
    let mut revision = None;
    let mut due: Option<f64> = None;
    let mut packed_count = 0;
    let mut max_lag: f64 = 0.;
    let io = proc_values("/proc/self/io");
    let cpu = cpu_ms();
    let start = Instant::now();
    let wait = |when: f64| {
        if speed > 0. {
            let deadline = Duration::from_secs_f64(when.max(0.) / 1000. / speed);
            if deadline > start.elapsed() {
                std::thread::sleep(deadline - start.elapsed());
            }
        }
    };
    for s in &page.strokes {
        let when = s.points.last().unwrap().time - first;
        ensure!(when.is_finite() && when >= 0.);
        while let Some(t) = due.filter(|t| *t <= when) {
            wait(t);
            let packed = measure(compact, &mut packs)?;
            packed_count += usize::from(packed);
            due = if packed {
                interval.map(|n| t + n)
            } else {
                None
            };
        }
        wait(when);
        if speed > 0. {
            max_lag = max_lag.max(start.elapsed().as_secs_f64() * 1000. - when / speed);
        }
        order.push(s.id);
        revision = Some(measure(
            || {
                storage::patch(
                    path,
                    InkDraftPatch {
                        order: order.clone(),
                        upserts: vec![s.clone()],
                        background: page.background.clone(),
                    },
                    revision.take(),
                )
            },
            &mut strokes,
        )?);
        if due.is_none() {
            due = interval.map(|n| when + n);
        }
    }
    let before_exit_cpu = cpu_ms() - cpu;
    let before_exit_io = proc_values("/proc/self/io");
    let rss_before_exit = proc_values("/proc/self/status").get("VmHWM").copied();
    if a[3] != "none" {
        loop {
            let packed = measure(compact, &mut exit_packs)?;
            packed_count += usize::from(packed);
            if !packed {
                break;
            }
        }
    }
    let total_cpu = cpu_ms() - cpu;
    let wall = start.elapsed().as_secs_f64() * 1000.;
    let after_io = proc_values("/proc/self/io");
    let rss = proc_values("/proc/self/status").get("VmHWM").copied();
    let snapshot = storage::read(path)?;
    ensure!(snapshot.revision == revision);
    equal(&snapshot.draft, &page, page.strokes.len())?;
    let size = sizes(path)?;
    let db = rusqlite::Connection::open(path)?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(integrity == "ok");
    ensure!(!db.prepare("PRAGMA foreign_key_check")?.exists([])?);
    drop(db);
    let mut history_states = 0;
    loop {
        let state = storage::navigate_update(path, None, None)?;
        let InkHistoryUpdate::Snapshot { history } = state else {
            unreachable!()
        };
        equal(
            &history.snapshot.draft,
            &page,
            page.strokes.len() - history_states,
        )?;
        history_states += 1;
        if !history.can_undo {
            break;
        }
        storage::navigate_update(path, Some(false), history.snapshot.revision)?;
    }
    ensure!(history_states == 51.min(page.strokes.len() + 1));
    let delta = |to: &BTreeMap<String, u64>, key: &str| {
        to.get(key)
            .zip(io.get(key))
            .map(|(a, b)| a.saturating_sub(*b))
    };
    println!(
        "{}",
        serde_json::json!({"policy":a[3],"speed":speed,"max_chunks":max_chunks,"decoded_budget_mib":decoded_mib,"peak_rss_before_exit_kib":rss_before_exit,"strokes":page.strokes.len(),"points":page.strokes.iter().map(|s|s.points.len()).sum::<usize>(),"stroke":timing(&strokes),"periodic":timing(&packs),"exit":timing(&exit_packs),"successful_packs":packed_count,"cpu_ms":total_cpu,"writing_cpu_ms":before_exit_cpu,"wall_ms":wall,"write_bytes":delta(&after_io,"write_bytes"),"writing_write_bytes":delta(&before_exit_io,"write_bytes"),"wchar":delta(&after_io,"wchar"),"peak_rss_kib":rss,"max_serial_event_lag_ms":max_lag,"size":size,"verified_history_states":history_states,"bitwise_verified":true})
    );
    Ok(())
}
