use std::cell::RefCell;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use tokio::sync::oneshot;

use super::{ExecutionContext, MapEntries, Value, VariantTag};

pub(super) const MAX_WORKERS_PER_SCOPE: usize = 1_024;
pub(super) const MAX_WORKER_SOURCE_BYTES: usize = 1_048_576;
const MAX_WORKER_MESSAGE_BYTES: usize = 4 * 1_048_576;
const MAX_WORKER_MESSAGE_DEPTH: usize = 128;
const MAX_WORKER_THREADS: usize = 32;
const WORKER_QUEUE_CAPACITY: usize = 1_024;
const STATUS_QUEUED: u8 = 0;
const STATUS_RUNNING: u8 = 1;
const STATUS_SUCCEEDED: u8 = 2;
const STATUS_FAILED: u8 = 3;
const STATUS_CANCEL_REQUESTED: u8 = 4;
const STATUS_CANCELLED: u8 = 5;

#[derive(Clone, Debug)]
pub(super) enum TransferValue {
    Void,
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Vec<TransferValue>),
    Map(Vec<(String, TransferValue)>),
    Variant(VariantTag, Option<Box<TransferValue>>),
}

pub(super) type WorkerResult = Result<TransferValue, String>;

pub struct WorkerTaskValue {
    pub(super) receiver: Option<oneshot::Receiver<WorkerResult>>,
    pub(super) result: Option<WorkerResult>,
    pub(super) cancellation: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
}

pub(super) struct WorkerJob {
    pub(super) source: String,
    pub(super) input: TransferValue,
    pub(super) context: ExecutionContext,
    pub(super) cancellation: Arc<AtomicBool>,
    pub(super) result: oneshot::Sender<WorkerResult>,
    status: Arc<AtomicU8>,
}

#[derive(Default)]
struct WorkerPoolCounters {
    queued: AtomicUsize,
    running: AtomicUsize,
    completed: AtomicU64,
    failed: AtomicU64,
    panicked: AtomicU64,
    rejected: AtomicU64,
}

pub(super) struct WorkerPoolStats {
    pub(super) threads: usize,
    pub(super) queue_capacity: usize,
    pub(super) queued: usize,
    pub(super) running: usize,
    pub(super) completed: u64,
    pub(super) failed: u64,
    pub(super) panicked: u64,
    pub(super) rejected: u64,
}

pub(super) struct WorkerPool {
    sender: Option<mpsc::SyncSender<WorkerJob>>,
    threads: Vec<JoinHandle<()>>,
    counters: Arc<WorkerPoolCounters>,
}

impl WorkerPool {
    pub(super) fn new() -> Arc<Self> {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        let configured = std::env::var("ORVISCRIPT_WORKER_THREADS").ok();
        let thread_count = configured_thread_count(configured.as_deref(), available);
        let (sender, receiver) = mpsc::sync_channel::<WorkerJob>(WORKER_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(receiver));
        let counters = Arc::new(WorkerPoolCounters::default());
        let mut threads = Vec::with_capacity(thread_count);
        for index in 0..thread_count {
            let receiver = receiver.clone();
            let counters = counters.clone();
            let builder = std::thread::Builder::new().name(format!("orvi-worker-{index}"));
            if let Ok(thread) = builder.spawn(move || {
                loop {
                    let job = {
                        let Ok(receiver) = receiver.lock() else {
                            return;
                        };
                        receiver.recv()
                    };
                    let Ok(job) = job else {
                        return;
                    };
                    counters.queued.fetch_sub(1, Ordering::AcqRel);
                    counters.running.fetch_add(1, Ordering::AcqRel);
                    let _ = job.status.compare_exchange(
                        STATUS_QUEUED,
                        STATUS_RUNNING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    let cancellation = job.cancellation.clone();
                    let (result, panicked) = catch_worker_panic(|| {
                        super::execute_worker_job(
                            &job.source,
                            job.input,
                            job.context,
                            job.cancellation,
                        )
                    });
                    counters.running.fetch_sub(1, Ordering::AcqRel);
                    counters.completed.fetch_add(1, Ordering::Relaxed);
                    if result.is_err() {
                        counters.failed.fetch_add(1, Ordering::Relaxed);
                    }
                    if panicked {
                        counters.panicked.fetch_add(1, Ordering::Relaxed);
                    }
                    let status = if result.is_ok() {
                        STATUS_SUCCEEDED
                    } else if cancellation.load(Ordering::Acquire) {
                        STATUS_CANCELLED
                    } else {
                        STATUS_FAILED
                    };
                    job.status.store(status, Ordering::Release);
                    let _ = job.result.send(result);
                }
            }) {
                threads.push(thread);
            }
        }
        Arc::new(Self {
            sender: Some(sender),
            threads,
            counters,
        })
    }

    pub(super) fn thread_count(&self) -> usize {
        self.threads.len()
    }

    pub(super) fn stats(&self) -> WorkerPoolStats {
        WorkerPoolStats {
            threads: self.threads.len(),
            queue_capacity: WORKER_QUEUE_CAPACITY,
            queued: self.counters.queued.load(Ordering::Acquire),
            running: self.counters.running.load(Ordering::Acquire),
            completed: self.counters.completed.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
            panicked: self.counters.panicked.load(Ordering::Relaxed),
            rejected: self.counters.rejected.load(Ordering::Relaxed),
        }
    }

    pub(super) fn spawn(
        &self,
        source: String,
        input: TransferValue,
        context: ExecutionContext,
    ) -> Result<Rc<RefCell<WorkerTaskValue>>, String> {
        if self.threads.is_empty() {
            return Err("worker pool could not start any threads".into());
        }
        if source.len() > MAX_WORKER_SOURCE_BYTES {
            return Err(format!(
                "worker source exceeds the {MAX_WORKER_SOURCE_BYTES} byte limit"
            ));
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        let status = Arc::new(AtomicU8::new(STATUS_QUEUED));
        let (result, receiver) = oneshot::channel();
        let job = WorkerJob {
            source,
            input,
            context,
            cancellation: cancellation.clone(),
            result,
            status: status.clone(),
        };
        self.counters.queued.fetch_add(1, Ordering::AcqRel);
        let send_result = self
            .sender
            .as_ref()
            .ok_or_else(|| "worker pool is unavailable".to_owned())?
            .try_send(job)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "worker queue is full".to_owned(),
                mpsc::TrySendError::Disconnected(_) => "worker pool is unavailable".to_owned(),
            });
        if let Err(error) = send_result {
            self.counters.queued.fetch_sub(1, Ordering::AcqRel);
            self.counters.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(error);
        }
        Ok(Rc::new(RefCell::new(WorkerTaskValue {
            receiver: Some(receiver),
            result: None,
            cancellation,
            status,
        })))
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.sender.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

pub(super) fn transfer_from_value(value: &Value) -> Result<TransferValue, String> {
    let mut visited = HashSet::new();
    let mut bytes = 0;
    transfer_from_value_inner(value, &mut visited, &mut bytes, 0)
}

fn transfer_from_value_inner(
    value: &Value,
    visited: &mut HashSet<usize>,
    bytes: &mut usize,
    depth: usize,
) -> Result<TransferValue, String> {
    if depth > MAX_WORKER_MESSAGE_DEPTH {
        return Err(format!(
            "worker message exceeds the maximum depth of {MAX_WORKER_MESSAGE_DEPTH}"
        ));
    }
    let transferred = match value {
        Value::Void => TransferValue::Void,
        Value::Null => TransferValue::Null,
        Value::Bool(value) => TransferValue::Bool(*value),
        Value::Integer(value) => TransferValue::Integer(*value),
        Value::Float(value) if value.is_finite() => TransferValue::Float(*value),
        Value::Float(_) => return Err("worker messages cannot contain NaN or infinity".into()),
        Value::String(value) => {
            add_bytes(bytes, value.len())?;
            TransferValue::String(value.clone())
        }
        Value::List(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return Err("worker messages cannot contain cyclic lists".into());
            }
            let transferred = values
                .borrow()
                .iter()
                .map(|value| transfer_from_value_inner(value, visited, bytes, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            visited.remove(&identity);
            TransferValue::List(transferred)
        }
        Value::Map(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return Err("worker messages cannot contain cyclic maps".into());
            }
            let transferred = values
                .borrow()
                .iter()
                .map(|(key, value)| {
                    add_bytes(bytes, key.len())?;
                    Ok((
                        key.clone(),
                        transfer_from_value_inner(value, visited, bytes, depth + 1)?,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            visited.remove(&identity);
            TransferValue::Map(transferred)
        }
        Value::Variant { tag, payload } => TransferValue::Variant(
            *tag,
            payload
                .as_deref()
                .map(|payload| {
                    transfer_from_value_inner(payload, visited, bytes, depth + 1).map(Box::new)
                })
                .transpose()?,
        ),
        _ => {
            return Err(format!(
                "worker messages cannot contain {} values",
                value.type_name()
            ));
        }
    };
    add_bytes(bytes, 8)?;
    Ok(transferred)
}

fn add_bytes(total: &mut usize, amount: usize) -> Result<(), String> {
    *total = total.saturating_add(amount);
    if *total > MAX_WORKER_MESSAGE_BYTES {
        Err(format!(
            "worker message exceeds the {MAX_WORKER_MESSAGE_BYTES} byte limit"
        ))
    } else {
        Ok(())
    }
}

pub(super) fn value_from_transfer(value: TransferValue) -> Value {
    match value {
        TransferValue::Void => Value::Void,
        TransferValue::Null => Value::Null,
        TransferValue::Bool(value) => Value::Bool(value),
        TransferValue::Integer(value) => Value::Integer(value),
        TransferValue::Float(value) => Value::Float(value),
        TransferValue::String(value) => Value::String(value),
        TransferValue::List(values) => Value::List(Rc::new(RefCell::new(
            values.into_iter().map(value_from_transfer).collect(),
        ))),
        TransferValue::Map(values) => Value::Map(Rc::new(RefCell::new(
            values
                .into_iter()
                .map(|(key, value)| (key, value_from_transfer(value)))
                .collect::<MapEntries>(),
        ))),
        TransferValue::Variant(tag, payload) => Value::Variant {
            tag,
            payload: payload.map(|payload| Box::new(value_from_transfer(*payload))),
        },
    }
}

pub(super) fn cancel(task: &WorkerTaskValue) -> bool {
    loop {
        let current = task.status.load(Ordering::Acquire);
        if matches!(
            current,
            STATUS_SUCCEEDED | STATUS_FAILED | STATUS_CANCELLED | STATUS_CANCEL_REQUESTED
        ) {
            return false;
        }
        if task
            .status
            .compare_exchange(
                current,
                STATUS_CANCEL_REQUESTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            task.cancellation.store(true, Ordering::Release);
            return true;
        }
    }
}

pub(super) fn is_cancelled(cancellation: &AtomicBool) -> bool {
    cancellation.load(Ordering::Acquire)
}

pub(super) fn status(task: &WorkerTaskValue) -> &'static str {
    match task.status.load(Ordering::Acquire) {
        STATUS_QUEUED => "queued",
        STATUS_RUNNING => "running",
        STATUS_SUCCEEDED => "succeeded",
        STATUS_FAILED => "failed",
        STATUS_CANCEL_REQUESTED => "cancel_requested",
        STATUS_CANCELLED => "cancelled",
        _ => "unknown",
    }
}

fn configured_thread_count(configured: Option<&str>, available: usize) -> usize {
    configured
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=MAX_WORKER_THREADS).contains(value))
        .unwrap_or(available)
        .clamp(1, MAX_WORKER_THREADS)
}

fn catch_worker_panic(operation: impl FnOnce() -> WorkerResult) -> (WorkerResult, bool) {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => (result, false),
        Err(_) => (
            Err("worker panicked while executing isolated code".into()),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{catch_worker_panic, configured_thread_count};

    #[test]
    fn validates_configured_thread_counts() {
        assert_eq!(configured_thread_count(Some("4"), 8), 4);
        assert_eq!(configured_thread_count(Some("0"), 8), 8);
        assert_eq!(configured_thread_count(Some("33"), 8), 8);
        assert_eq!(configured_thread_count(Some("invalid"), 8), 8);
        assert_eq!(configured_thread_count(None, 128), 32);
    }

    #[test]
    fn turns_panics_into_worker_errors() {
        let (result, panicked) = catch_worker_panic(|| panic!("test panic"));
        assert!(panicked);
        assert_eq!(
            result.expect_err("panic becomes an error"),
            "worker panicked while executing isolated code"
        );
    }
}
