use super::{Event, Update};
use crate::registry::Registry;
use async_channel::{Receiver, Sender, unbounded};
use std::{fs, path::PathBuf, thread};

type Job = Box<dyn FnOnce() + Send>;
type StoreJob = Box<dyn FnOnce(&mut Registry) -> Result<Event, String> + Send>;

#[derive(Clone)]
pub(super) struct Pool {
    jobs: Sender<Job>,
    events: Sender<Update>,
}

impl Pool {
    fn new(name: &str, threads: usize, events: Sender<Update>) -> Self {
        let (jobs, queue) = unbounded::<Job>();
        for i in 0..threads {
            let queue = queue.clone();
            thread::Builder::new()
                .name(format!("githalo-{name}-{i}"))
                .spawn(move || {
                    while let Ok(job) = queue.recv_blocking() {
                        job();
                    }
                })
                .expect("start application worker");
        }
        Self { jobs, events }
    }
    pub(super) fn run(&self, f: impl FnOnce() -> Event + Send + 'static) {
        let events = self.events.clone();
        self.execute(move || {
            let _ = events.send_blocking(Update(f()));
        });
    }
    pub(super) fn execute(&self, job: impl FnOnce() + Send + 'static) {
        let events = self.events.clone();
        let _ = self.jobs.try_send(Box::new(move || {
            if !events.is_closed() {
                job();
            }
        }));
    }
}

pub(super) struct Runtime {
    pub(super) status: Pool,
    pub(super) detail: Pool,
    pub(super) fetch: Pool,
    pub(super) write: Pool,
    pub(super) scan: Pool,
    pub(super) db_path: PathBuf,
    store_queue: Sender<StoreJob>,
    events: Sender<Update>,
}

impl Runtime {
    pub(super) fn new(path: PathBuf) -> (Self, Receiver<Update>) {
        let db_path = path.clone();
        let (events, receiver) = unbounded();
        let (store_queue, queue) = unbounded::<StoreJob>();
        let results = events.clone();
        thread::Builder::new()
            .name("githalo-registry".into())
            .spawn(move || {
                let open = || -> Result<Registry, String> {
                    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    Registry::open(&path).map_err(|e| e.to_string())
                };
                let mut db = match open() {
                    Ok(db) => db,
                    Err(e) => {
                        let _ = results.send_blocking(Update(Event::Error(format!(
                            "无法打开仓库登记（{e}）"
                        ))));
                        return;
                    }
                };
                let mut send = |op: StoreJob| {
                    let event = op(&mut db).unwrap_or_else(Event::Error);
                    results.send_blocking(Update(event)).is_ok()
                };
                if !send(Box::new(|db| {
                    Ok(Event::Board {
                        board: db.board().map_err(|e| e.to_string())?,
                        reload: false,
                        notice: None,
                    })
                })) {
                    return;
                }
                while let Ok(op) = queue.recv_blocking() {
                    if !send(op) {
                        break;
                    }
                }
            })
            .expect("start registry worker");
        (
            Self {
                status: Pool::new("status", 3, events.clone()),
                detail: Pool::new("details", 2, events.clone()),
                fetch: Pool::new("fetch", 3, events.clone()),
                write: Pool::new("write", 1, events.clone()),
                scan: Pool::new("scan", 1, events.clone()),
                db_path,
                store_queue,
                events,
            },
            receiver,
        )
    }
    pub(super) fn events_sender(&self) -> Sender<Update> {
        self.events.clone()
    }

    pub(super) fn store(
        &self,
        op: impl FnOnce(&mut Registry) -> Result<Event, String> + Send + 'static,
    ) {
        if self.store_queue.try_send(Box::new(op)).is_err() {
            let _ = self
                .events
                .try_send(Update(Event::Error("仓库登记不可用".into())));
        }
    }
}
