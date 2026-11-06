//! Writes the config on its own thread. A save waits for the disk, which on a slow or
//! network drive can take long enough to stall the window.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Repaint = Arc<dyn Fn() + Send + Sync>;

struct Job {
    number: u64,
    path: PathBuf,
    text: String,
    repaint: Repaint,
}

/// How a save went, and which one it was.
pub(super) type Finished = (u64, Result<(), String>);

#[derive(Default)]
pub(super) struct Saver {
    jobs: Option<mpsc::Sender<Job>>,
    finished: Arc<Mutex<Option<Finished>>>,
    /// The number of the newest save sent, and of the newest one known to be done.
    sent: u64,
    done: u64,
}

impl Saver {
    fn start(&mut self) -> &mpsc::Sender<Job> {
        self.jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let finished = self.finished.clone();
            std::thread::Builder::new()
                .name("config-saver".into())
                .spawn(move || {
                    while let Ok(mut job) = rx.recv() {
                        // Only the newest config matters: it has every change in it.
                        while let Ok(newer) = rx.try_recv() {
                            job = newer;
                        }
                        let result =
                            linecon_config::write(&job.path, &job.text).map_err(|e| e.to_string());
                        *finished.lock().unwrap() = Some((job.number, result));
                        (job.repaint)();
                    }
                })
                .expect("could not start the config saver");
            tx
        })
    }

    pub(super) fn send(&mut self, path: PathBuf, text: String, repaint: Repaint) {
        self.sent += 1;
        let number = self.sent;
        let job = Job {
            number,
            path,
            text,
            repaint,
        };
        if self.start().send(job).is_err() {
            // The thread is gone, so the next send starts another.
            self.jobs = None;
            *self.finished.lock().unwrap() = Some((number, Err("the saver stopped".into())));
        }
    }

    /// The result of the newest save, once it is done. Older ones are overtaken by it.
    pub(super) fn take_finished(&mut self) -> Option<Result<(), String>> {
        let (number, result) = self.finished.lock().unwrap().take()?;
        self.done = self.done.max(number);
        (number == self.sent).then_some(result)
    }

    pub(super) fn in_flight(&self) -> bool {
        self.done < self.sent
    }

    /// Waits for the newest save, for when the program is about to stop.
    pub(super) fn wait(&mut self, limit: Duration) -> Option<Result<(), String>> {
        let deadline = Instant::now() + limit;
        while self.in_flight() && Instant::now() < deadline {
            if let Some(result) = self.take_finished() {
                return Some(result);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}
