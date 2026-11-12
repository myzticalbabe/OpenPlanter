//! op-swarm: sub-second fan-out/fan-in swarm over tokio.
//!
//! The reusable primitive is [`join_with_deadline`]: drain a [`JoinSet`]
//! under an overall deadline, aborting stragglers. On timeout it returns
//! whatever finished in time plus `timed_out = true`, so callers keep
//! partial results instead of losing them. [`run_swarm`] is the benchmark
//! workload built on it (N concurrent sleeps vs serial).

use std::time::Duration;
use tokio::task::JoinSet;

/// Result of one swarm run.
#[derive(Debug)]
pub struct SwarmReport {
    pub completed: Vec<u64>,
    pub elapsed_ms: u128,
    /// Wall time the same work would take serially (n * work_ms).
    pub serial_ms: u128,
    /// serial_ms / elapsed_ms (>= 1.0 when the swarm beats serial).
    pub speedup: f64,
    pub timed_out: bool,
}

/// Drain every task in `set`, waiting at most `deadline_ms` total.
///
/// Returns `(outputs, timed_out)`. On timeout the remaining tasks are
/// aborted, but results that finished before the deadline tripped are
/// still returned — partial progress is never discarded.
pub async fn join_with_deadline<T: Send + 'static>(
    set: &mut JoinSet<T>,
    deadline_ms: u64,
) -> (Vec<T>, bool) {
    // `out` lives outside the timeout so results reaped before the deadline
    // trips survive it — a timed-out inner vec would be dropped with its future.
    let mut out = Vec::new();
    let finished = tokio::time::timeout(Duration::from_millis(deadline_ms), async {
        while let Some(r) = set.join_next().await {
            if let Ok(v) = r {
                out.push(v);
            }
        }
    })
    .await
    .is_ok();
    if !finished {
        // Top up with anything that finished but wasn't reaped yet.
        while let Some(r) = set.try_join_next() {
            if let Ok(v) = r {
                out.push(v);
            }
        }
        set.abort_all();
    }
    (out, !finished)
}

/// Run `n` tasks concurrently. Each sleeps `work_ms` then returns `i*i`.
/// Whole swarm must finish within `deadline_ms` or stragglers are aborted
/// (partials kept). `speedup` proves the fan-out beat serial execution.
pub async fn run_swarm(n: usize, work_ms: u64, deadline_ms: u64) -> SwarmReport {
    let start = std::time::Instant::now();
    let mut set: JoinSet<u64> = JoinSet::new();
    for i in 0..n as u64 {
        set.spawn(async move {
            tokio::time::sleep(Duration::from_millis(work_ms)).await;
            i * i
        });
    }
    let (mut completed, timed_out) = join_with_deadline(&mut set, deadline_ms).await;
    completed.sort_unstable();
    let elapsed_ms = start.elapsed().as_millis();
    let serial_ms = n as u128 * work_ms as u128;
    let speedup = serial_ms as f64 / elapsed_ms.max(1) as f64;
    SwarmReport {
        completed,
        elapsed_ms,
        serial_ms,
        speedup,
        timed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn swarm_64x50ms_finishes_subsecond() {
        let r = run_swarm(64, 50, 900).await;
        assert!(!r.timed_out, "swarm hit deadline");
        assert_eq!(r.completed.len(), 64);
        assert!(r.elapsed_ms < 1000, "took {}ms", r.elapsed_ms);
        assert_eq!(r.completed[3], 9);
        // 3200ms of serial work in <1000ms wall: the fan-out proved itself.
        assert!(r.speedup > 4.0, "speedup too low: {}", r.speedup);
    }

    #[tokio::test]
    async fn swarm_aborts_on_tight_deadline() {
        let r = run_swarm(8, 500, 100).await;
        assert!(r.timed_out);
        assert!(r.completed.is_empty());
    }

    #[tokio::test]
    async fn join_with_deadline_collects_all() {
        let mut set: JoinSet<u32> = JoinSet::new();
        for i in 0..16u32 {
            set.spawn(async move { i + 1 });
        }
        let (mut out, timed_out) = join_with_deadline(&mut set, 5_000).await;
        out.sort_unstable();
        assert!(!timed_out);
        assert_eq!(out, (1..=16).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn join_with_deadline_keeps_partials() {
        let mut set: JoinSet<u32> = JoinSet::new();
        // Two fast tasks finish well inside the deadline, two slow ones don't.
        for (i, sleep) in [5u64, 10, 500, 500].into_iter().enumerate() {
            set.spawn(async move {
                tokio::time::sleep(Duration::from_millis(sleep)).await;
                i as u32
            });
        }
        let (mut out, timed_out) = join_with_deadline(&mut set, 150).await;
        out.sort_unstable();
        assert!(timed_out);
        assert_eq!(out, vec![0, 1], "partials lost: {out:?}");
    }
}
