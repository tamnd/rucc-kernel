//! Boots run side by side.
//!
//! A boot is one QEMU with two CPUs, so on a runner with four, one boot at a time leaves half the
//! machine idle. On the A64 row every guest runs under TCG, where one kselftest collection or the
//! KUnit boot can take twenty five minutes, and one boot after another took longer than the job is
//! allowed. The boots of one suite do not read each other's files, so they can run at once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One piece of work for [`run`].
pub type Task<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

/// How many boots run at once when nobody says: one for every two host CPUs, since every guest
/// has two.
#[must_use]
pub fn default_jobs() -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1))
}

/// Run every task, at most `jobs` of them at once, and answer what each returned, in the order of
/// the tasks.
pub fn run<T: Send>(jobs: usize, tasks: Vec<Task<'_, T>>) -> Vec<T> {
    let count = tasks.len();
    let next = AtomicUsize::new(0);
    let tasks: Vec<Mutex<Option<Task<'_, T>>>> =
        tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let results: Vec<Mutex<Option<T>>> = (0..count).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, count.max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = tasks.get(i) else { break };
                    let task = slot.lock().ok().and_then(|mut t| t.take());
                    if let (Some(task), Ok(mut result)) = (task, results[i].lock()) {
                        *result = Some(task());
                    }
                }
            });
        }
    });
    results
        .into_iter()
        .map(|r| {
            r.into_inner()
                .ok()
                .flatten()
                .expect("every task ran to the end")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_results_come_back_in_the_order_of_the_tasks() {
        let tasks: Vec<Task<'_, usize>> = (0..7_usize)
            .map(|i| -> Task<'_, usize> {
                Box::new(move || {
                    std::thread::sleep(Duration::from_millis(((7 - i) * 5) as u64));
                    i * 10
                })
            })
            .collect();
        assert_eq!(run(3, tasks), vec![0, 10, 20, 30, 40, 50, 60]);
        assert_eq!(run::<u8>(2, Vec::new()), Vec::<u8>::new());
    }

    #[test]
    fn two_jobs_run_two_tasks_at_once() {
        let start = Instant::now();
        let tasks: Vec<Task<'_, ()>> = (0..2)
            .map(|_| -> Task<'_, ()> {
                Box::new(|| std::thread::sleep(Duration::from_millis(300)))
            })
            .collect();
        run(2, tasks);
        assert!(
            start.elapsed() < Duration::from_millis(550),
            "{:?}",
            start.elapsed()
        );
    }
}
