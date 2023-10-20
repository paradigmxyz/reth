use minstant::Instant;
use std::time::Duration;

#[derive(Debug)]
pub(crate) struct ExecutionDurationRecord {
    /// execute inner time recorder
    inner_recorder: Instant,
    /// time recorder
    time_recorder: Instant,

    /// total time of execute_inner
    pub(crate) execute_inner: Duration,
    /// total time of  get block td and block_with_senders
    pub(crate) read_block: Duration,
    /// time of revm execute tx(execute_and_verify_receipt)
    pub(crate) execute_tx: Duration,
    /// time of process state(state.extend)
    pub(crate) process_state: Duration,
    /// time of write to db
    pub(crate) write_to_db: Duration,
}

impl Default for ExecutionDurationRecord {
    fn default() -> Self {
        Self {
            inner_recorder: Instant::now(),
            time_recorder: Instant::now(),
            execute_inner: Duration::default(),
            read_block: Duration::default(),
            execute_tx: Duration::default(),
            process_state: Duration::default(),
            write_to_db: Duration::default(),
        }
    }
}

impl ExecutionDurationRecord {
    /// start inner time recorder
    pub(crate) fn start_inner_time_recorder(&mut self) {
        self.inner_recorder = Instant::now();
    }
    /// start time recorder
    pub(crate) fn start_time_recorder(&mut self) {
        self.time_recorder = Instant::now();
    }
    /// add time of execute_inner
    pub(crate) fn add_execute_inner(&mut self) {
        self.execute_inner =
            self.execute_inner.checked_add(self.inner_recorder.elapsed()).expect("overflow");
    }
    /// add time of get block td and block_with_senders
    pub(crate) fn add_read_block(&mut self) {
        self.read_block =
            self.read_block.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of revm execute tx
    pub(crate) fn add_execute_tx(&mut self) {
        self.execute_tx =
            self.execute_tx.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of process state
    pub(crate) fn add_process_state(&mut self) {
        self.process_state =
            self.process_state.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of write to db
    pub(crate) fn add_write_to_db(&mut self) {
        self.write_to_db =
            self.write_to_db.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
}
