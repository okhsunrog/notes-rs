//! Storage-only benchmark: production codec, experimental SQLite layout, no UI or sync.
use anyhow::{Result, ensure};
use ink_format::{
    DType, Encoding, cbor,
    chunk::{Chunk, Column, f64_column},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Point {
    x: f64,
    y: f64,
    pressure: f64,
    tilt_x: f64,
    tilt_y: f64,
    time: f64,
}
impl Point {
    fn values(&self) -> [f64; 6] {
        [
            self.x,
            self.y,
            self.pressure,
            self.tilt_x,
            self.tilt_y,
            self.time,
        ]
    }
}
#[derive(Clone, Deserialize)]
struct Stroke {
    width: f64,
    points: Vec<Point>,
}
#[derive(Deserialize)]
struct Page {
    strokes: Vec<Stroke>,
}
fn id(n: usize) -> [u8; 16] {
    let mut b = [0; 16];
    b[..8].copy_from_slice(&(n as u64 + 1).to_le_bytes());
    b
}
fn chunk(n: usize, s: &Stroke) -> Chunk {
    let mut columns: Vec<_> = [1, 2, 3, 5, 6, 11]
        .into_iter()
        .enumerate()
        .map(|(axis, semantic)| f64_column(semantic, s.points.iter().map(|p| p.values()[axis])))
        .collect();
    columns.push(Column {
        semantic: 12,
        dtype: DType::U8,
        required: true,
        validity: None,
        values: vec![2; s.points.len()],
    });
    Chunk {
        id: id(n),
        profile: id(usize::MAX / 2),
        segments: vec![(id(n), s.points.len() as u32)],
        columns,
    }
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
            Some((k.to_string(), v.split_whitespace().next()?.parse().ok()?))
        })
        .collect()
}
#[derive(Default, Serialize)]
struct Times {
    samples: usize,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    total_ms: f64,
}
fn times(v: &[f64]) -> Times {
    if v.is_empty() {
        return Times::default();
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    Times {
        samples: s.len(),
        median_ms: s[s.len() / 2],
        p95_ms: s[((s.len() as f64 * 0.95).ceil() as usize - 1).min(s.len() - 1)],
        max_ms: *s.last().unwrap(),
        total_ms: s.iter().sum(),
    }
}
#[derive(Default, Serialize)]
struct Metrics {
    mode: String,
    speed: f64,
    strokes: usize,
    points: usize,
    save_interval_ms: f64,
    stroke: Times,
    autosave: Times,
    initial_encode_ms: f64,
    repack_decode_ms: f64,
    repack_encode_ms: f64,
    initial_payload_bytes: usize,
    packed_payload_bytes: usize,
    cpu_ms: f64,
    wall_ms: f64,
    write_bytes: Option<u64>,
    wchar: Option<u64>,
    peak_rss_kib: Option<u64>,
    max_wal_bytes: u64,
    db_bytes: u64,
    geometry_bytes: u64,
    metadata_bytes: u64,
    max_event_lag_ms: f64,
    verified: bool,
}
struct Store {
    conn: Connection,
    path: String,
    pending: Vec<i64>,
    next_chunk: i64,
    seq: i64,
    state: Vec<usize>,
    m: Metrics,
    stroke_times: Vec<f64>,
    save_times: Vec<f64>,
    crash: String,
}
impl Store {
    fn open(path: &str, mode: &str, speed: f64, crash: String) -> Result<Self> {
        ensure!(
            !Path::new(path).exists(),
            "refusing to overwrite existing database"
        );
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
    CREATE TABLE chunks(id INTEGER PRIMARY KEY,data BLOB NOT NULL);
    CREATE TABLE geometry(id INTEGER PRIMARY KEY,chunk INTEGER NOT NULL REFERENCES chunks(id),segment INTEGER NOT NULL,width REAL NOT NULL);
    CREATE TABLE history(seq INTEGER PRIMARY KEY,state BLOB NOT NULL);
    CREATE TABLE head(singleton INTEGER PRIMARY KEY,seq INTEGER NOT NULL REFERENCES history(seq));
    CREATE TABLE checkpoint(singleton INTEGER PRIMARY KEY,seq INTEGER NOT NULL);")?;
        Ok(Self {
            conn,
            path: path.into(),
            pending: vec![],
            next_chunk: 0,
            seq: 0,
            state: vec![],
            m: Metrics {
                mode: mode.into(),
                speed,
                save_interval_ms: 5000.,
                ..Default::default()
            },
            stroke_times: vec![],
            save_times: vec![],
            crash,
        })
    }
    fn wal(&mut self) {
        self.m.max_wal_bytes = self.m.max_wal_bytes.max(
            fs::metadata(format!("{}-wal", self.path))
                .map(|x| x.len())
                .unwrap_or(0),
        );
    }
    fn kill_at(&self, phase: &str) {
        if self.crash == phase {
            eprintln!("crash_expected_strokes={}", self.state.len());
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            }
        }
    }
    fn append(&mut self, n: usize, s: &Stroke) -> Result<()> {
        let start = Instant::now();
        let t = Instant::now();
        let bytes = chunk(n, s).encode(if self.m.mode == "raw" {
            Encoding::Raw
        } else {
            Encoding::Pco8
        })?;
        self.m.initial_encode_ms += t.elapsed().as_secs_f64() * 1000.;
        self.m.initial_payload_bytes += bytes.len();
        self.next_chunk += 1;
        let cid = self.next_chunk;
        self.state.push(n);
        self.seq += 1;
        let state = cbor::encode(&cbor::array(
            self.state.iter().map(|n| cbor::num(*n as u64)),
        ))?;
        let tx = self.conn.transaction()?;
        tx.execute("INSERT INTO chunks VALUES(?1,?2)", params![cid, bytes])?;
        tx.execute(
            "INSERT INTO geometry VALUES(?1,?2,0,?3)",
            params![n as i64, cid, s.width],
        )?;
        tx.execute(
            "INSERT INTO history VALUES(?1,?2)",
            params![self.seq, state],
        )?;
        tx.execute("INSERT OR REPLACE INTO head VALUES(1,?1)", [self.seq])?;
        tx.execute("DELETE FROM history WHERE seq<?1", [self.seq - 50])?;
        tx.commit()?;
        self.pending.push(cid);
        self.wal();
        self.stroke_times
            .push(start.elapsed().as_secs_f64() * 1000.);
        Ok(())
    }
    fn save(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        self.kill_at("before_pack");
        let t = Instant::now();
        let mut merged: Option<Chunk> = None;
        let mut remap = vec![];
        for cid in &self.pending {
            let bytes: Vec<u8> =
                self.conn
                    .query_row("SELECT data FROM chunks WHERE id=?1", [cid], |r| r.get(0))?;
            let c = Chunk::decode(&bytes)?;
            if let Some(m) = &mut merged {
                for (out, col) in m.columns.iter_mut().zip(c.columns) {
                    out.values.extend(col.values)
                }
                m.segments.extend(c.segments)
            } else {
                merged = Some(c)
            };
            let gid: i64 =
                self.conn
                    .query_row("SELECT id FROM geometry WHERE chunk=?1", [cid], |r| {
                        r.get(0)
                    })?;
            remap.push(gid);
        }
        self.m.repack_decode_ms += t.elapsed().as_secs_f64() * 1000.;
        let mut merged = merged.unwrap();
        self.next_chunk += 1;
        merged.id = id(self.next_chunk as usize + 1_000_000);
        let t = Instant::now();
        let bytes = merged.encode(Encoding::Pco8)?;
        self.m.repack_encode_ms += t.elapsed().as_secs_f64() * 1000.;
        self.m.packed_payload_bytes += bytes.len();
        self.kill_at("after_encode");
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO chunks VALUES(?1,?2)",
            params![self.next_chunk, bytes],
        )?;
        for (segment, gid) in remap.iter().enumerate() {
            tx.execute(
                "UPDATE geometry SET chunk=?1,segment=?2 WHERE id=?3",
                params![self.next_chunk, segment as i64, gid],
            )?;
        }
        for cid in &self.pending {
            tx.execute("DELETE FROM chunks WHERE id=?1", [cid])?;
        }
        tx.execute("INSERT OR REPLACE INTO checkpoint VALUES(1,?1)", [self.seq])?;
        if self.crash == "before_commit" {
            eprintln!("crash_expected_strokes={}", self.state.len());
            unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            }
        }
        tx.commit()?;
        self.kill_at("after_commit");
        self.pending.clear();
        self.wal();
        self.save_times.push(start.elapsed().as_secs_f64() * 1000.);
        Ok(())
    }
}
fn verify(path: &str, page: &Page) -> Result<usize> {
    let conn = Connection::open(path)?;
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    ensure!(integrity == "ok");
    let mut decoded = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT id,data FROM chunks")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))? {
        let (id, b) = row?;
        decoded.insert(id, Chunk::decode(&b)?);
    }
    let mut count = 0;
    let mut stmt = conn.prepare("SELECT id,chunk,segment,width FROM geometry ORDER BY id")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)? as usize,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)? as usize,
            r.get::<_, f64>(3)?,
        ))
    })? {
        let (n, cid, segment, width) = row?;
        let c = &decoded[&cid];
        let s = &page.strokes[n];
        ensure!(width.to_bits() == s.width.to_bits());
        ensure!(c.segments[segment] == (id(n), s.points.len() as u32));
        let offset: usize = c.segments[..segment].iter().map(|(_, n)| *n as usize).sum();
        for (i, p) in s.points.iter().enumerate() {
            for (axis, value) in p.values().into_iter().enumerate() {
                ensure!(
                    c.columns[axis].values[(offset + i) * 8..(offset + i + 1) * 8]
                        == value.to_le_bytes(),
                    "point bits mismatch"
                );
            }
        }
        count += 1;
    }
    let mut stmt = conn.prepare("SELECT state FROM history")?;
    for row in stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
        let bytes = row?;
        let state = cbor::decode(&bytes)?;
        for n in cbor::items(&state)? {
            let n = cbor::u64_value(n)? as usize;
            ensure!(n < count, "history references missing geometry");
        }
    }
    Ok(count)
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() >= 4,
        "usage: ink-storage-bench page.json db mode[pco|raw|verify] [speed:0=unpaced,1=realtime] [crash-phase]"
    );
    let page: Page = serde_json::from_slice(&fs::read(&args[1])?)?;
    ensure!(!page.strokes.is_empty());
    if args[3] == "verify" {
        println!(
            "{}",
            serde_json::json!({"verified_strokes":verify(&args[2],&page)?})
        );
        return Ok(());
    }
    ensure!(args[3] == "pco" || args[3] == "raw");
    let speed = args
        .get(4)
        .map(|s| s.parse::<f64>())
        .transpose()?
        .unwrap_or(0.);
    ensure!(speed.is_finite() && speed >= 0.);
    let mut store = Store::open(
        &args[2],
        &args[3],
        speed,
        args.get(5).cloned().unwrap_or_default(),
    )?;
    let first = page.strokes[0].points[0].time;
    let mut events = vec![];
    let last = page.strokes.last().unwrap().points.last().unwrap().time - first;
    for n in 1..=(last / 5000.).floor() as usize {
        events.push((n as f64 * 5000., None));
    }
    for (n, s) in page.strokes.iter().enumerate() {
        ensure!(!s.points.is_empty());
        events.push((s.points.last().unwrap().time - first, Some(n)));
    }
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    let start = Instant::now();
    let cpu = cpu_ms();
    let io = proc_values("/proc/self/io");
    for (when, event) in events {
        if speed > 0. {
            let deadline = Duration::from_secs_f64(when / 1000. / speed);
            if deadline > start.elapsed() {
                std::thread::sleep(deadline - start.elapsed())
            }
            store.m.max_event_lag_ms = store
                .m
                .max_event_lag_ms
                .max(start.elapsed().as_secs_f64() * 1000. - when / speed);
        }
        match event {
            Some(n) => store.append(n, &page.strokes[n])?,
            None => store.save()?,
        }
    }
    store.save()?;
    store.m.cpu_ms = cpu_ms() - cpu;
    store.m.wall_ms = start.elapsed().as_secs_f64() * 1000.;
    let after = proc_values("/proc/self/io");
    store.m.write_bytes = after
        .get("write_bytes")
        .zip(io.get("write_bytes"))
        .map(|(a, b)| a - b);
    store.m.wchar = after.get("wchar").zip(io.get("wchar")).map(|(a, b)| a - b);
    store.m.peak_rss_kib = proc_values("/proc/self/status").get("VmHWM").copied();
    store.m.geometry_bytes = store.conn.query_row(
        "SELECT coalesce(sum(length(data)),0) FROM chunks",
        [],
        |r| r.get::<_, i64>(0),
    )? as u64;
    store.m.metadata_bytes = store.conn.query_row(
        "SELECT coalesce(sum(length(state)),0) FROM history",
        [],
        |r| r.get::<_, i64>(0),
    )? as u64;
    store
        .conn
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    store.m.db_bytes = fs::metadata(&args[2])?.len();
    store.m.strokes = page.strokes.len();
    store.m.points = page.strokes.iter().map(|s| s.points.len()).sum();
    store.m.stroke = times(&store.stroke_times);
    store.m.autosave = times(&store.save_times);
    ensure!(verify(&args[2], &page)? == page.strokes.len());
    store.m.verified = true;
    println!("{}", serde_json::to_string(&store.m)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page() -> Page {
        Page {
            strokes: (0..55)
                .map(|n| Stroke {
                    width: 3.,
                    points: vec![Point {
                        x: -0.0,
                        y: n as f64 + 0.123,
                        pressure: 0.123456789,
                        tilt_x: -12.,
                        tilt_y: 45.,
                        time: 1788656480932.125,
                    }],
                })
                .collect(),
        }
    }
    #[test]
    fn both_modes_preserve_bits_and_retained_history_with_identical_final_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let p = page();
        let mut outputs = vec![];
        for mode in ["raw", "pco"] {
            let path = dir.path().join(format!("{mode}.db"));
            let mut s = Store::open(path.to_str().unwrap(), mode, 0., String::new()).unwrap();
            for (n, stroke) in p.strokes.iter().enumerate() {
                s.append(n, stroke).unwrap();
                if n % 5 == 4 {
                    s.save().unwrap();
                }
            }
            assert_eq!(verify(path.to_str().unwrap(), &p).unwrap(), 55); // mixed packed and pending
            s.save().unwrap();
            assert_eq!(verify(path.to_str().unwrap(), &p).unwrap(), 55);
            assert_eq!(
                s.conn
                    .query_row("SELECT count(*) FROM history", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                51
            );
            let mut stmt = s
                .conn
                .prepare("SELECT data FROM chunks ORDER BY id")
                .unwrap();
            outputs.push(
                stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
            );
        }
        assert_eq!(outputs[0], outputs[1]);
    }
    #[test]
    fn unchanged_saves_do_no_work_and_existing_database_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let path = path.to_str().unwrap();
        let mut s = Store::open(path, "raw", 0., String::new()).unwrap();
        s.save().unwrap();
        assert!(s.save_times.is_empty());
        assert!(Store::open(path, "raw", 0., String::new()).is_err());
    }
}
