//! op-swarm: sub-second fan-out/fan-in swarm over tokio.
//!
//! The reusable primitive is [`join_with_deadline`]: drain a [`JoinSet`]
//! under an overall deadline, aborting stragglers. [`run_swarm`] is the
//! demo workload built on it (N sleeps running concurrently).

use std::time::Duration;
use tokio::task::JoinSet;

/// Result of one swarm run.
#[derive(Debug)]
pub struct SwarmReport {
    pub completed: Vec<u64>,
    pub elapsed_ms: u128,
    pub timed_out: bool,
}

/// Drain every task in `set`, waiting at most `deadline_ms` total.
///
/// Returns `(outputs, timed_out)`. On timeout the remaining tasks are
/// aborted and partial results are discarded, so callers never ingest a
/// half-finished swarm.
pub async fn join_with_deadline<T: Send + 'static>(
    set: &mut JoinSet<T>,
    deadline_ms: u64,
) -> (Vec<T>, bool) {
    let res = tokio::time::timeout(Duration::from_millis(deadline_ms), async {
        let mut out = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok(v) = r {
                out.push(v);
            }
        }
        out
    })
    .await;
    match res {
        Ok(out) => (out, false),
        Err(_) => {
            set.abort_all();
            (Vec::new(), true)
        }
    }
}

/// Run `n` tasks concurrently. Each sleeps `work_ms` then returns `i*i`.
/// Whole swarm must finish within `deadline_ms` or stragglers are aborted.
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
    SwarmReport {
        completed,
        elapsed_ms: start.elapsed().as_millis(),
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
}
