#[cfg(feature = "enable_execution_duration_record")]
use minstant::Instant;
#[cfg(any(feature = "enable_execution_duration_record", feature = "enable_db_speed_record"))]
use std::time::Duration;

#[cfg(feature = "enable_execution_duration_record")]
pub(crate) const COL_WIDTH_MIDDLE: usize = 14;
#[cfg(feature = "enable_execution_duration_record")]
pub(crate) const COL_WIDTH_BIG: usize = 18;

/// excution duration record
#[cfg(feature = "enable_execution_duration_record")]
#[derive(Debug, Clone, Copy)]
pub struct ExecutionDurationRecord {
    /// execute inner time recorder
    inner_recorder: Instant,
    /// time recorder
    time_recorder: Instant,

    /// total time of execute inner.
    pub execute_inner: Duration,
    /// total time of get block td and block_with_senders.
    pub read_block: Duration,
    /// total time of revm execute tx(execute_and_verify_receipt).
    pub execute_tx: Duration,
    /// total time of process state(state.extend)
    pub process_state: Duration,
    /// total time of write to db
    pub write_to_db: Duration,
}

#[cfg(feature = "enable_execution_duration_record")]
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

#[cfg(feature = "enable_execution_duration_record")]
impl ExecutionDurationRecord {
    const SECONDS_ONE_HOUR: f64 = 60.0 * 60.0;

    /// start inner time recorder
    pub(crate) fn start_inner_time_recorder(&mut self) {
        self.inner_recorder = Instant::now();
    }
    /// start time recorder
    pub(crate) fn start_time_recorder(&mut self) {
        self.time_recorder = Instant::now();
    }
    /// add time of execute_inner
    pub(crate) fn add_execute_inner_duration(&mut self) {
        self.execute_inner =
            self.execute_inner.checked_add(self.inner_recorder.elapsed()).expect("overflow");
    }
    /// add time of get block td and block_with_senders
    pub(crate) fn add_read_block_duration(&mut self) {
        self.read_block =
            self.read_block.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of revm execute tx
    pub(crate) fn add_execute_tx_duration(&mut self) {
        self.execute_tx =
            self.execute_tx.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of process state
    pub(crate) fn add_process_state_duration(&mut self) {
        self.process_state =
            self.process_state.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add time of write to db
    pub(crate) fn add_write_to_db_duration(&mut self) {
        self.write_to_db =
            self.write_to_db.checked_add(self.time_recorder.elapsed()).expect("overflow");
    }
    /// add
    pub fn add(&mut self, other: ExecutionDurationRecord) {
        self.execute_inner = self.execute_inner.checked_add(other.execute_inner).expect("overflow");
        self.read_block = self.read_block.checked_add(other.read_block).expect("overflow");
        self.execute_tx = self.execute_tx.checked_add(other.execute_tx).expect("overflow");
        self.process_state = self.process_state.checked_add(other.process_state).expect("overflow");
        self.write_to_db = self.write_to_db.checked_add(other.write_to_db).expect("overflow");
    }

    fn execute_inner_time(&self) -> f64 {
        self.execute_inner.as_secs_f64() / Self::SECONDS_ONE_HOUR
    }

    fn fetching_block_time(&self) -> f64 {
        self.execute_tx.as_secs_f64() / Self::SECONDS_ONE_HOUR
    }

    fn fetching_block_time_percent(&self) -> f64 {
        self.fetching_block_time() / self.execute_inner_time()
    }

    fn execute_tx_time(&self) -> f64 {
        self.execute_tx.as_secs_f64() / Self::SECONDS_ONE_HOUR
    }

    fn execute_tx_time_percent(&self) -> f64 {
        self.execute_tx_time() / self.execute_inner_time()
    }

    fn process_state_time(&self) -> f64 {
        self.process_state.as_secs_f64() / Self::SECONDS_ONE_HOUR
    }

    fn process_state_time_percent(&self) -> f64 {
        self.process_state_time() / self.execute_inner_time()
    }

    fn write_to_db_time(&self) -> f64 {
        self.write_to_db.as_secs_f64() / Self::SECONDS_ONE_HOUR
    }

    fn write_to_db_time_percent(&self) -> f64 {
        self.write_to_db_time() / self.execute_inner_time()
    }

    fn misc_time(&self) -> f64 {
        self.execute_inner_time() -
            self.fetching_block_time() -
            self.execute_tx_time() -
            self.process_state_time() -
            self.write_to_db_time()
    }

    fn misc_time_percent(&self) -> f64 {
        self.misc_time() / self.execute_inner_time()
    }

    fn print_line(&self, cat: &str, time: f64, time_percent: f64) {
        println!(
            "{: <COL_WIDTH_BIG$}{: >COL_WIDTH_MIDDLE$.3}{: >COL_WIDTH_MIDDLE$.2}",
            cat, time, time_percent,
        );
    }

    /// print the information of the execution duration record.
    pub fn print(&self) {
        println!();
        println!("===============================Metric of execution duration==========================================================");
        println!(
            "{: <COL_WIDTH_BIG$}{: >COL_WIDTH_MIDDLE$}{: >COL_WIDTH_MIDDLE$}",
            "Cat.", "Time (h)", "Time (%)",
        );

        self.print_line("total", self.execute_inner_time(), 100.0);
        self.print_line("misc", self.misc_time(), self.misc_time_percent());
        self.print_line(
            "fetching_blocks",
            self.fetching_block_time(),
            self.fetching_block_time_percent(),
        );
        self.print_line("execution", self.execute_tx_time(), self.execute_tx_time_percent());
        self.print_line(
            "process_state",
            self.process_state_time(),
            self.process_state_time_percent(),
        );
        self.print_line("write_to_db", self.write_to_db_time(), self.write_to_db_time_percent());

        println!();
    }
}

/// db speed record
#[cfg(feature = "enable_db_speed_record")]
#[derive(Debug, Clone, Copy)]
pub struct DbSpeedRecord {
    /// time of read header td from db
    pub read_header_td_db_time: (u64, Duration),
    /// data size of read header td from db
    pub read_header_td_db_size: u64,
    /// time of read block with senders from db
    pub read_block_with_senders_db_time: (u64, Duration),
    /// data size of read block with senders from db
    pub read_block_with_senders_db_size: u64,
    /// time of write to db
    pub write_to_db_time: (u64, Duration),
    /// data size of write to db
    pub write_to_db_size: u64,
}

#[cfg(feature = "enable_db_speed_record")]
impl Default for DbSpeedRecord {
    fn default() -> Self {
        Self {
            read_header_td_db_time: (0, Duration::default()),
            read_header_td_db_size: 0,
            read_block_with_senders_db_time: (0, Duration::default()),
            read_block_with_senders_db_size: 0,
            write_to_db_time: (0, Duration::default()),
            write_to_db_size: 0,
        }
    }
}

#[cfg(feature = "enable_db_speed_record")]
impl DbSpeedRecord {
    /// add time of write to db
    pub(crate) fn add_read_header_td_db_time(&mut self, add_time: Duration, get_time_count: u64) {
        self.read_header_td_db_time.0 =
            self.read_header_td_db_time.0.checked_add(get_time_count).expect("overflow");
        self.read_header_td_db_time.1 =
            self.read_header_td_db_time.1.checked_add(add_time).expect("overflow");
    }

    /// add time of write to db
    pub(crate) fn add_read_header_td_db_size(&mut self, add_size: u64) {
        self.read_header_td_db_size =
            self.read_header_td_db_size.checked_add(add_size).expect("overflow");
    }

    /// add time of write to db
    pub(crate) fn add_read_block_with_senders_db_time(
        &mut self,
        add_time: Duration,
        get_time_count: u64,
    ) {
        self.read_block_with_senders_db_time.0 =
            self.read_block_with_senders_db_time.0.checked_add(get_time_count).expect("overflow");
        self.read_block_with_senders_db_time.1 =
            self.read_block_with_senders_db_time.1.checked_add(add_time).expect("overflow");
    }

    /// add time of write to db
    pub(crate) fn add_read_block_with_senders_db_size(&mut self, add_size: u64) {
        self.read_block_with_senders_db_size =
            self.read_block_with_senders_db_size.checked_add(add_size).expect("overflow");
    }

    /// add time of write to db
    pub(crate) fn add_write_to_db_time(&mut self, add_time: Duration, get_time_count: u64) {
        self.write_to_db_time.0 =
            self.write_to_db_time.0.checked_add(get_time_count).expect("overflow");
        self.write_to_db_time.1 = self.write_to_db_time.1.checked_add(add_time).expect("overflow");
    }

    /// add time of write to db
    pub(crate) fn add_write_to_db_size(&mut self, add_size: u64) {
        self.write_to_db_size = self.write_to_db_size.checked_add(add_size).expect("overflow");
    }

    /// add
    pub fn add(&mut self, other: Self) {
        self.read_header_td_db_time = (
            self.read_header_td_db_time
                .0
                .checked_add(other.read_header_td_db_time.0)
                .expect("overflow"),
            self.read_header_td_db_time
                .1
                .checked_add(other.read_header_td_db_time.1)
                .expect("overflow"),
        );
        self.read_header_td_db_size = self
            .read_header_td_db_size
            .checked_add(other.read_header_td_db_size)
            .expect("overflow");

        self.read_block_with_senders_db_time = (
            self.read_block_with_senders_db_time
                .0
                .checked_add(other.read_block_with_senders_db_time.0)
                .expect("overflow"),
            self.read_block_with_senders_db_time
                .1
                .checked_add(other.read_block_with_senders_db_time.1)
                .expect("overflow"),
        );
        self.read_block_with_senders_db_size = self
            .read_block_with_senders_db_size
            .checked_add(other.read_block_with_senders_db_size)
            .expect("overflow");

        self.write_to_db_time = (
            self.write_to_db_time.0.checked_add(other.write_to_db_time.0).expect("overflow"),
            self.write_to_db_time.1.checked_add(other.write_to_db_time.1).expect("overflow"),
        );
        self.write_to_db_size =
            self.write_to_db_size.checked_add(other.write_to_db_size).expect("overflow");
    }

    fn cover_size_bytes_to_m(&self, bytes_size: u64) -> f64 {
        bytes_size as f64 / 1024.0 / 1024.0
    }

    /// print the information of db speed record.
    pub fn print(&self, header: &str) {
        println!();
        println!("{}", header);

        let col_len = 15;

        let read_header_td_time = self.read_header_td_db_time.1.as_secs_f64();
        let read_header_td_size = self.cover_size_bytes_to_m(self.read_header_td_db_size);
        let read_header_td_rate = read_header_td_size / read_header_td_time;

        let read_block_with_senders_time = self.read_block_with_senders_db_time.1.as_secs_f64();
        let read_block_with_senders_size =
            self.cover_size_bytes_to_m(self.read_block_with_senders_db_size);
        let read_block_with_senders_rate =
            read_block_with_senders_size / read_block_with_senders_time;

        let write_to_db_time = self.write_to_db_time.1.as_secs_f64();
        let write_to_db_size = self.cover_size_bytes_to_m(self.write_to_db_size);
        let write_to_db_rate = write_to_db_size / write_to_db_time;

        println!("Cat.                           Size (MBytes)   Time (s)   Rate (MBytes/s)");
        println! {"{:col_len$}{:>col_len$.3}{:>col_len$.3}{:>col_len$.3}", "Read header td         ",
        read_header_td_size, read_header_td_time, read_header_td_rate};
        println! {"{:col_len$}{:>col_len$.3}{:>col_len$.3}{:>col_len$.3}", "Read header with sender",
        read_block_with_senders_size, read_block_with_senders_time, read_block_with_senders_rate};
        println! {"{:col_len$}{:>col_len$.3}{:>col_len$.3}{:>col_len$.3}", "Write to db            ",
        write_to_db_size, write_to_db_time, write_to_db_rate};

        println!();
    }
}
