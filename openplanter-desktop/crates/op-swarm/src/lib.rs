//! op-swarm: sub-second fan-out/fan-in swarm over tokio.

use std::time::Duration;
use tokio::task::JoinSet;

/// Result of one swarm run.
#[derive(Debug)]
pub struct SwarmReport {
    pub completed: Vec<u64>,
    pub elapsed_ms: u128,
    pub timed_out: bool,
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
    let res = tokio::time::timeout(Duration::from_millis(deadline_ms), async {
        let mut out = Vec::with_capacity(n);
        while let Some(r) = set.join_next().await {
            if let Ok(v) = r {
                out.push(v);
            }
        }
        out
    })
    .await;
    match res {
        Ok(mut out) => {
            out.sort_unstable();
            SwarmReport {
                completed: out,
                elapsed_ms: start.elapsed().as_millis(),
                timed_out: false,
            }
        }
        Err(_) => {
            set.abort_all();
            SwarmReport {
                completed: Vec::new(),
                elapsed_ms: start.elapsed().as_millis(),
                timed_out: true,
            }
        }
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
}
