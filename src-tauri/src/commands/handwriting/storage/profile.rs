//! Test-only phase counters; no instrumentation is compiled into the app.
use std::{cell::RefCell, collections::BTreeMap, time::Instant};
type Counters = BTreeMap<&'static str, (usize, f64)>;
thread_local! { static COUNTERS: RefCell<Option<Counters>> = const { RefCell::new(None) }; }
pub(super) fn start() {
    COUNTERS.with(|c| *c.borrow_mut() = Some(BTreeMap::new()));
}
pub(super) fn finish() -> Counters {
    COUNTERS.with(|c| c.borrow_mut().take().unwrap_or_default())
}
pub(super) struct Span {
    name: &'static str,
    start: Instant,
}
pub(super) fn span(name: &'static str) -> Option<Span> {
    COUNTERS.with(|c| {
        c.borrow().is_some().then(|| Span {
            name,
            start: Instant::now(),
        })
    })
}
impl Drop for Span {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64() * 1000.;
        COUNTERS.with(|c| {
            if let Some(c) = &mut *c.borrow_mut() {
                let entry = c.entry(self.name).or_default();
                entry.0 += 1;
                entry.1 += elapsed;
            }
        });
    }
}

pub(super) fn kill_point(stage: &str) {
    if COUNTERS.with(|c| c.borrow().is_some())
        && std::env::var("INK_PROFILE_KILL").is_ok_and(|v| v == stage)
    {
        eprintln!("INK_KILL {stage}");
        // External SIGKILL performs no Rust cleanup; used only in isolated test
        // processes. A failure to deliver the signal fails the test instead.
        let status = std::process::Command::new("kill")
            .args(["-9", &std::process::id().to_string()])
            .status()
            .expect("run kill");
        panic!("SIGKILL did not terminate test process: {status}");
    }
}
