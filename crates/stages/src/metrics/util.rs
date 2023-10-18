#[derive(Debug, Default)]
pub(crate) struct ExecutionDurationRecord {
    /// execute inner time recorder
    inner_recorder: revm_utils::time::TimeRecorder,
    /// time recorder
    time_recorder: revm_utils::time::TimeRecorder,

    /// total time of execute_inner
    pub(crate) execute_inner: u64,
    /// total time of  get block td and block_with_senders
    pub(crate) read_block: u64,
    /// time of revm execute tx(execute_and_verify_receipt)
    pub(crate) execute_tx: u64,
    /// time of process state(state.extend)
    pub(crate) process_state: u64,
    /// time of write to db
    pub(crate) write_to_db: u64,
}

impl ExecutionDurationRecord {
    /// start inner time recorder
    pub(crate) fn start_inner_time_recorder(&mut self) {
        self.inner_recorder = revm_utils::time::TimeRecorder::now();
    }
    /// start time recorder
    pub(crate) fn start_time_recorder(&mut self) {
        self.time_recorder = revm_utils::time::TimeRecorder::now();
    }
    /// add time of execute_inner
    pub(crate) fn add_execute_inner(&mut self) {
        self.execute_inner = self
            .execute_inner
            .checked_add(self.inner_recorder.elapsed().to_cycles())
            .expect("overflow");
    }
    /// add time of get block td and block_with_senders
    pub(crate) fn add_read_block(&mut self) {
        self.read_block = self
            .read_block
            .checked_add(self.time_recorder.elapsed().to_cycles())
            .expect("overflow");
    }
    /// add time of revm execute tx
    pub(crate) fn add_execute_tx(&mut self) {
        self.execute_tx = self
            .execute_tx
            .checked_add(self.time_recorder.elapsed().to_cycles())
            .expect("overflow");
    }
    /// add time of process state
    pub(crate) fn add_process_state(&mut self) {
        self.process_state = self
            .process_state
            .checked_add(self.time_recorder.elapsed().to_cycles())
            .expect("overflow");
    }
    /// add time of write to db
    pub(crate) fn add_write_to_db(&mut self) {
        self.write_to_db = self
            .write_to_db
            .checked_add(self.time_recorder.elapsed().to_cycles())
            .expect("overflow");
    }
}
