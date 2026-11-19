//! Many movies corrected one after another, as the tilt series of cryo-ET
//! (`series::run`) and the micrographs of single particles
//! (`spa::project::motion`) are: movies are read and decoded ahead
//! (processor and disk: `prepare`) by a few reader threads while the one
//! before is measured and summed on this thread (GPU: `finish`), and what
//! follows a movie (writing it) is left to the caller's `post`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc::SyncSender;

use anyhow::{Context, Result};

use super::{check_cancel, finish, prepare, Corrected, Params, Prepared};

/// Starts `readers` threads in `scope` that prepare the movies of `jobs`
/// in order (a job of None is skipped), each at most `ahead` movies beyond
/// the one `taken` says is being finished, and hand each to `send` as
/// `wrap(prepared)`. They stop when `stopped` is set, on cancel, or when the
/// receiving end is gone.
#[allow(clippy::too_many_arguments)]
pub fn spawn_readers<'scope, 'env, T: Send + 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    jobs: &'env [Option<(&'env Path, &'env Params)>],
    scratch: &'env Path,
    readers: usize,
    ahead: usize,
    next: &'env AtomicUsize,
    taken: &'env AtomicUsize,
    stopped: &'env AtomicBool,
    send: SyncSender<(usize, Result<T>)>,
    wrap: &'env (dyn Fn(Prepared) -> T + Sync),
) {
    for _ in 0..readers.max(1) {
        let send = send.clone();
        scope.spawn(move || loop {
            let i = next.fetch_add(1, SeqCst);
            if i >= jobs.len() {
                break;
            }
            let Some((path, p)) = jobs[i] else { continue };
            // Not further ahead than the readers can hold.
            while i > taken.load(SeqCst) + ahead && check_cancel().is_ok() && !stopped.load(SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if check_cancel().is_err() || stopped.load(SeqCst) {
                break;
            }
            let prepared = path.to_str().context("电影路径编码无效").and_then(|path| prepare(path, None, scratch, 0, 0, 0, p, vec![], &|_, _, _| {}));
            if send.send((i, prepared.map(wrap))).is_err() {
                break;
            }
        });
    }
}

/// Corrects every movie of `jobs` (None: skipped) with two readers ahead and
/// hands each result to `post` in order, on this thread. `label` names the
/// progress of movie i.
pub fn each(
    jobs: &[Option<(&Path, &Params)>],
    scratch: &Path,
    label: &(dyn Fn(usize) -> String + Sync),
    progress: &(dyn Fn(&str, usize, usize) + Sync),
    mut post: impl FnMut(usize, Result<Corrected>) -> Result<()>,
) -> Result<()> {
    let (next, taken, stopped) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicBool::new(false));
    let wrap = |p: Prepared| p;
    std::thread::scope(|scope| -> Result<()> {
        // Release readers waiting on the window when this ends early.
        struct Stop<'a>(&'a AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(true, SeqCst);
            }
        }
        let _stop = Stop(&stopped);
        let (send, read) = std::sync::mpsc::sync_channel::<(usize, Result<Prepared>)>(0);
        spawn_readers(scope, jobs, scratch, 2, 2, &next, &taken, &stopped, send, &wrap);
        let mut ready = std::collections::BTreeMap::new();
        for (i, job) in jobs.iter().enumerate() {
            check_cancel()?;
            let Some((_, p)) = job else { continue };
            let label = label(i);
            progress(&label, i, jobs.len());
            taken.store(i, SeqCst);
            let prepared = loop {
                if let Some(p) = ready.remove(&i) {
                    break p;
                }
                let (at, p) = read.recv().map_err(|_| anyhow::anyhow!("已取消"))?;
                ready.insert(at, p);
            };
            let corrected = prepared.and_then(|m| finish(m, None, p, &|stage, a, b| progress(&format!("{label} · {stage}"), a, b)));
            post(i, corrected)?;
        }
        Ok(())
    })
}
