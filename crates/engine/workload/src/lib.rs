//! This crate includes thread pool primitives for mixed I/O and CPU-bound workloads.
//!
//! This is intended to be used for computing the state root during payload validation.


/// Workflow
///
/// Main thread:
///  - executes block sequentially
///  - awaits stateroot
///
/// Stateroot task:
///   - spawns multiproof calculations
///     * fetches multiproofs from disk
///        * this spawns more tasks Starting proof calculation
///        * joins all
///     * (emits proof calculated message)
/// Prewarming task:
///  - schedules transactions (this should be a simple loop over a channel that spawns tx execution and receives result until cancelled)
///  - should be tokio blocking task, limited to
///
/// 1. spawn stateroot task
/// 2. spawn transaction prewarming task
///

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]


use crossbeam_channel::{bounded, unbounded, Receiver, Sender, select};
use rayon::ThreadPool as RayonPool;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use std::time::Duration;
use std::collections::BinaryHeap;
use std::cmp::Ordering;

/// An executor for mixed I/O and CPU workloads.
pub struct WorkloadExecutor<T> {
    runtime: Arc<Runtime>,
    cpu_pool: Arc<RayonPool>,
    result_receiver: Receiver<T>,
    result_sender: Sender<T>,
    cancel_sender: broadcast::Sender<()>,
    scheduler: Arc<dyn SchedulingStrategy>,
    task_queue: Arc<parking_lot::Mutex<BinaryHeap<PrioritizedTask>>>,
}

// // Task priority and scheduling
// #[derive(PartialEq, Eq)]
// pub enum Priority {
//     Low = 0,
//     Medium = 1,
//     High = 2,
//     Critical = 3,
// }
//
// // Scheduling strategy trait
// pub trait SchedulingStrategy: Send + Sync + 'static {
//     fn schedule(&self, tasks: &mut BinaryHeap<PrioritizedTask>);
//     fn should_yield(&self) -> bool;
// }
//
// // Example FIFO strategy
// pub struct FifoStrategy;
// impl SchedulingStrategy for FifoStrategy {
//     fn schedule(&self, _tasks: &mut BinaryHeap<PrioritizedTask>) {
//         // FIFO doesn't reorder tasks
//     }
//
//     fn should_yield(&self) -> bool {
//         false
//     }
// }
//
// // Example Fair scheduling strategy
// pub struct FairSchedulingStrategy {
//     quantum: Duration,
//     last_yield: std::sync::atomic::AtomicU64,
// }
//
// impl SchedulingStrategy for FairSchedulingStrategy {
//     fn schedule(&self, tasks: &mut BinaryHeap<PrioritizedTask>) {
//         // Reorder tasks to ensure fairness between different priorities
//         let mut temp = Vec::new();
//         let mut last_priority = None;
//
//         while let Some(task) = tasks.pop() {
//             if let Some(last) = last_priority {
//                 if task.priority == last {
//                     // Move to back of queue
//                     temp.push(task);
//                     continue;
//                 }
//             }
//             last_priority = Some(task.priority);
//             tasks.push(task);
//         }
//
//         // Restore temporary tasks
//         for task in temp {
//             tasks.push(task);
//         }
//     }
//
//     fn should_yield(&self) -> bool {
//         let now = std::time::SystemTime::now()
//             .duration_since(std::time::UNIX_EPOCH)
//             .unwrap()
//             .as_millis() as u64;
//
//         let last = self.last_yield.load(std::sync::atomic::Ordering::Relaxed);
//         if now - last > self.quantum.as_millis() as u64 {
//             self.last_yield.store(now, std::sync::atomic::Ordering::Relaxed);
//             true
//         } else {
//             false
//         }
//     }
// }
//
// // Prioritized task wrapper
// #[derive(Clone)]
// struct PrioritizedTask {
//     priority: Priority,
//     task_id: u64,
//     timestamp: std::time::SystemTime,
// }
//
// impl PartialEq for PrioritizedTask {
//     fn eq(&self, other: &Self) -> bool {
//         self.priority == other.priority && self.task_id == other.task_id
//     }
// }
//
// impl Eq for PrioritizedTask {}
//
// impl PartialOrd for PrioritizedTask {
//     fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
//         Some(self.cmp(other))
//     }
// }
//
// impl Ord for PrioritizedTask {
//     fn cmp(&self, other: &Self) -> Ordering {
//         self.priority.cmp(&other.priority)
//             .then_with(|| self.timestamp.cmp(&other.timestamp))
//     }
// }

//
// impl<T: Send + 'static> WorkloadExecutor<T> {
//     pub async fn new(
//         cpu_threads: usize,
//         database_url: &str,
//         scheduler: impl SchedulingStrategy,
//     ) -> Result<Self, sqlx::Error> {
//         let db_pool = PgPool::connect(database_url).await?;
//         let db_pool = Arc::new(db_pool);
//         let runtime = Arc::new(Runtime::new().unwrap());
//         let cpu_pool = Arc::new(RayonPool::new(cpu_threads).unwrap());
//         let (result_sender, result_receiver) = unbounded();
//         let (cancel_sender, _) = broadcast::channel(100);
//
//         Ok(WorkloadExecutor {
//             runtime,
//             db_pool,
//             cpu_pool,
//             result_receiver,
//             result_sender,
//             cancel_sender,
//             scheduler: Arc::new(scheduler),
//             task_queue: Arc::new(parking_lot::Mutex::new(BinaryHeap::new())),
//         })
//     }
//
//     pub async fn execute<F>(
//         &self,
//         initial_ids: Vec<i64>,
//         process_fn: F,
//         priority: Priority,
//     ) -> Result<ExecutionHandle<T>, Box<dyn std::error::Error>>
//     where
//         F: Fn(Vec<Row>) -> TaskResult<T> + Send + Sync + 'static,
//         T: Send + 'static,
//     {
//         let db_pool = self.db_pool.clone();
//         let cpu_pool = self.cpu_pool.clone();
//         let result_sender = self.result_sender.clone();
//         let process_fn = Arc::new(process_fn);
//         let mut cancel_rx = self.cancel_sender.subscribe();
//         let scheduler = self.scheduler.clone();
//         let task_queue = self.task_queue.clone();
//
//         // Create task metadata
//         let task = PrioritizedTask {
//             priority,
//             task_id: rand::random(),
//             timestamp: std::time::SystemTime::now(),
//         };
//
//         // Add to scheduling queue
//         task_queue.lock().push(task.clone());
//
//         let handle = ExecutionHandle {
//             cancel_sender: self.cancel_sender.clone(),
//             task_id: task.task_id,
//         };
//
//         self.runtime.spawn(async move {
//             let mut current_ids = initial_ids;
//
//             loop {
//                 // Check cancellation
//                 if let Ok(()) = cancel_rx.try_recv() {
//                     break;
//                 }
//
//                 // Apply scheduling strategy
//                 {
//                     let mut queue = task_queue.lock();
//                     scheduler.schedule(&mut queue);
//                 }
//
//                 // Fetch data with spawn_blocking
//                 let rows = match tokio::task::spawn_blocking(move || {
//                     fetch_data(&db_pool, &current_ids)
//                 }).await {
//                     Ok(data) => data,
//                     Err(_) => break,
//                 };
//
//                 // Process with Rayon, checking for yields
//                 let process_fn = process_fn.clone();
//                 let scheduler = scheduler.clone();
//
//                 let result = cpu_pool.install(move || {
//                     let mut result = None;
//                     rayon::scope(|s| {
//                         s.spawn(|_| {
//                             result = Some(process_fn(rows));
//
//                             // Check if we should yield to other tasks
//                             if scheduler.should_yield() {
//                                 std::thread::yield_now();
//                             }
//                         });
//                     });
//                     result.unwrap()
//                 });
//
//                 match result {
//                     TaskResult::NeedsMore(ids) => {
//                         current_ids = ids;
//                     }
//                     TaskResult::Done(final_result) => {
//                         result_sender.send(final_result).unwrap();
//                         break;
//                     }
//                 }
//             }
//
//             // Remove task from queue when done
//             task_queue.lock().retain(|t| t.task_id != task.task_id);
//         });
//
//         Ok(handle)
//     }
// }
//
// // Handle for controlling task execution
// pub struct ExecutionHandle<T> {
//     cancel_sender: broadcast::Sender<()>,
//     task_id: u64,
// }
//
// impl<T> ExecutionHandle<T> {
//     pub fn cancel(&self) -> Result<(), broadcast::error::SendError<()>> {
//         self.cancel_sender.send(())
//     }
//
//     pub fn task_id(&self) -> u64 {
//         self.task_id
//     }
// }
//
// // Example usage
// async fn main() -> Result<(), Box<dyn std::error::Error>> {
//     // Create executor with fair scheduling
//     let fair_scheduler = FairSchedulingStrategy {
//         quantum: Duration::from_millis(100),
//         last_yield: std::sync::atomic::AtomicU64::new(0),
//     };
//
//     let executor = WorkloadExecutor::new(
//         num_cpus::get(),
//         "postgresql://localhost/database",
//         fair_scheduler,
//     ).await?;
//
//     // Define processing function
//     let process = |rows: Vec<Row>| {
//         // CPU-intensive work
//         let result: Vec<_> = rows.par_iter()
//             .map(|row| {
//                 // Heavy computation...
//                 row.data.clone()
//             })
//             .collect();
//
//         TaskResult::Done(result)
//     };
//
//     // Execute high-priority task
//     let handle1 = executor.execute(
//         vec![1, 2, 3],
//         process.clone(),
//         Priority::High,
//     ).await?;
//
//     // Execute low-priority task
//     let handle2 = executor.execute(
//         vec![4, 5, 6],
//         process.clone(),
//         Priority::Low,
//     ).await?;
//
//     // Cancel first task after some time
//     tokio::time::sleep(Duration::from_secs(1)).await;
//     handle1.cancel()?;
//
//     Ok(())
}