//! The work-stealing pool one root is walked on.
//!
//! Per-thread `crossbeam_deque` queues, popped LIFO locally and stolen from
//! the oldest end — rayon's and jwalk's shape, and the arm `b2-rustix` was
//! measured on. W0.2 put this against a shared LIFO stack and a shared FIFO
//! queue and could not separate the three: the walk is bound by the kernel,
//! not by the queue in front of it. That is the reason to keep the scheduler
//! the spike measured rather than the one that reads most simply — a
//! difference too small to see in 144 hyperfine rows is also too small to
//! trade for anything.
//!
//! Each worker owns its buffers and its output, so nothing is shared on the
//! per-entry path. Three things do cross threads, all of them cold: the
//! descriptor budget, the in-flight count that decides when the walk is over,
//! and the parking lot idle workers wait on.
//!
//! Idle workers back off with `spin_loop` and then a timed condvar wait
//! rather than `yield_now`, deliberately: `sched_yield` would charge the
//! walk's own idling to the sys time AC-3 measures, which is how a scheduler
//! can look expensive without doing any more work.

use std::os::fd::BorrowedFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crossbeam_deque::{Injector, Steal, Stealer, Worker};

use super::sys::{Ctx, Job, Out, Walker};

/// Upper bound on worker threads.
///
/// `SCAP_LIST_THREADS` is a measurement and tuning knob, not a way to fork a
/// thousand threads at a filesystem: past the core count the walk only adds
/// contention, and the W0.2 matrix stopped at 16 for that reason.
pub(crate) const MAX_THREADS: usize = 64;

/// Local spins before a worker with nothing to do parks.
///
/// Small on purpose: work arrives in bursts as directories are read, so a
/// worker that finds nothing usually finds nothing for a while.
const SPINS_BEFORE_PARK: u32 = 32;

/// How long a parked worker sleeps before looking again.
///
/// The condvar is notified whenever work is pushed, so this bound only has to
/// cover a missed wake-up, not the common case.
const PARK_TIMEOUT: Duration = Duration::from_micros(500);

/// Where idle workers wait.
///
/// The mutex guards no data at all — it exists only to pair with the
/// condvar — so a poisoned lock carries no information and is taken anyway
/// throughout. That also keeps the panic path below from panicking again
/// while unwinding, which would abort the process.
struct Park {
    mutex: Mutex<()>,
    condvar: Condvar,
    sleepers: AtomicUsize,
}

impl Park {
    /// Wakes anyone waiting, without taking the lock when nobody is.
    ///
    /// The check is what keeps the common case — every worker busy — off the
    /// mutex entirely.
    fn wake(&self) {
        if self.sleepers.load(Ordering::SeqCst) > 0 {
            let _guard = self.lock();
            self.condvar.notify_all();
        }
    }

    /// Releases every sleeper, for the worker that retired the last job.
    fn wake_all_final(&self) {
        let _guard = self.lock();
        self.condvar.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Retires one job from the in-flight count on the way out of the iteration
/// that took it — including an unwinding one.
///
/// Without this a panicking worker would leave the count permanently above
/// zero: every other worker would wait for work that can never arrive, and
/// `thread::scope` would hang on the join rather than propagating the panic.
/// A hang is a far worse failure than a panic, and it is the one the reader's
/// `expect`s could otherwise cause.
struct Retire<'a> {
    in_flight: &'a AtomicUsize,
    park: &'a Park,
}

impl Drop for Retire<'_> {
    fn drop(&mut self) {
        if self.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            // The last outstanding directory: nothing else will ever be
            // queued, so every sleeper has to be released rather than left to
            // time out.
            self.park.wake_all_final();
        }
    }
}

/// Walks everything reachable from `initial`, returning one output per
/// worker for the caller to merge.
///
/// `threads` is clamped to `1..=MAX_THREADS`, so a caller that passes zero or
/// a wild override still gets a walk.
pub(crate) fn run(
    ctx: &Ctx<'_>,
    root_fd: BorrowedFd<'_>,
    initial: Vec<Job>,
    threads: usize,
) -> Vec<Out> {
    let threads = threads.clamp(1, MAX_THREADS);

    let workers: Vec<Worker<Job>> = (0..threads).map(|_| Worker::new_lifo()).collect();
    let stealers: Vec<Stealer<Job>> = workers.iter().map(Worker::stealer).collect();
    let injector: Injector<Job> = Injector::new();
    // Counted before the first worker starts, so the termination test below
    // can never see a transient zero.
    let in_flight = AtomicUsize::new(initial.len());
    for job in initial {
        injector.push(job);
    }
    let park =
        Park { mutex: Mutex::new(()), condvar: Condvar::new(), sleepers: AtomicUsize::new(0) };

    std::thread::scope(|scope| {
        let handles: Vec<_> = workers
            .into_iter()
            .enumerate()
            .map(|(me, local)| {
                let (stealers, injector, in_flight, park) =
                    (&stealers, &injector, &in_flight, &park);
                scope.spawn(move || {
                    work(ctx, root_fd, me, local, stealers, injector, in_flight, park)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("walk worker panicked")).collect()
    })
}

/// One worker's whole life: take a directory, read it, hand its children to
/// the queue, and stop when nothing is left anywhere.
#[expect(
    clippy::too_many_arguments,
    reason = "the pool's shared state is passed as the separate borrows it \
              actually is; bundling it into a struct would add a level of \
              indirection to the steal loop and hide which parts are shared"
)]
fn work(
    ctx: &Ctx<'_>,
    root_fd: BorrowedFd<'_>,
    me: usize,
    local: Worker<Job>,
    stealers: &[Stealer<Job>],
    injector: &Injector<Job>,
    in_flight: &AtomicUsize,
    park: &Park,
) -> Out {
    let mut walker = Walker::new(ctx);
    let mut children = Vec::new();
    let mut spins = 0u32;

    loop {
        if let Some(job) = find_job(&local, injector, stealers, me) {
            spins = 0;
            // Dropped at the end of this iteration, after the children below
            // have been counted. The children have to be counted before the
            // job is retired, or a worker landing between the two would see
            // the walk as finished and stop every other worker with the queue
            // still full.
            let _retire = Retire { in_flight, park };
            walker.run(job, root_fd, &mut children);

            if !children.is_empty() {
                in_flight.fetch_add(children.len(), Ordering::SeqCst);
                for child in children.drain(..) {
                    local.push(child);
                }
                park.wake();
            }
            continue;
        }

        if in_flight.load(Ordering::SeqCst) == 0 {
            return walker.into_out();
        }

        spins += 1;
        if spins < SPINS_BEFORE_PARK {
            // User-space only. `yield_now` here would charge `sched_yield` to
            // the sys time the walk is measured on.
            std::hint::spin_loop();
            continue;
        }

        let guard = park.lock();
        park.sleepers.fetch_add(1, Ordering::SeqCst);
        // Re-checked under the lock: the last job may have been retired
        // between the check above and this one, and its notification would
        // then already have been sent.
        if in_flight.load(Ordering::SeqCst) == 0 {
            park.sleepers.fetch_sub(1, Ordering::SeqCst);
            return walker.into_out();
        }
        let _unused = park.condvar.wait_timeout(guard, PARK_TIMEOUT);
        park.sleepers.fetch_sub(1, Ordering::SeqCst);
        spins = 0;
    }
}

/// Finds one directory to read: this worker's own queue first, then the seed
/// queue, then somebody else's.
///
/// Locally the deque is LIFO, so a worker keeps descending the subtree it
/// just opened and its buffers stay warm. Stealers take from the other end,
/// the oldest jobs, which are the ones furthest from any other worker's hot
/// path.
fn find_job(
    local: &Worker<Job>,
    injector: &Injector<Job>,
    stealers: &[Stealer<Job>],
    me: usize,
) -> Option<Job> {
    if let Some(job) = local.pop() {
        return Some(job);
    }
    loop {
        match injector.steal_batch_and_pop(local) {
            Steal::Success(job) => return Some(job),
            Steal::Empty => break,
            // Another thread was mid-steal; the queue may still hold work.
            Steal::Retry => {}
        }
    }
    for (i, stealer) in stealers.iter().enumerate() {
        if i == me {
            continue;
        }
        loop {
            match stealer.steal_batch_and_pop(local) {
                Steal::Success(job) => return Some(job),
                Steal::Empty => break,
                Steal::Retry => {}
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "pool_tests.rs"]
mod tests;
