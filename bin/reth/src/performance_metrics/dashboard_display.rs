#[cfg(feature = "enable_opcode_metrics")]
use super::dashboard_opcode::*;
#[cfg(feature = "enable_opcode_metrics")]
use lazy_static::lazy_static;
#[cfg(feature = "enable_tps_gas_record")]
use minstant::Instant;
#[cfg(feature = "enable_opcode_metrics")]
use revm::OpCode;
#[cfg(feature = "enable_opcode_metrics")]
use revm_utils::types::RevmMetricRecord;
#[cfg(feature = "enable_opcode_metrics")]
use std::collections::HashMap;
#[cfg(feature = "enable_opcode_metrics")]
use std::time::Duration;

#[cfg(feature = "enable_execution_duration_record")]
use reth_stages::ExecutionDurationRecord;

#[cfg(feature = "enable_db_speed_record")]
use reth_stages::DbSpeedRecord;

#[cfg(feature = "enable_cache_record")]
use revm_utils::types::CacheDbRecord;

#[cfg(feature = "enable_tps_gas_record")]
use std::ops::{Div, Mul};

#[cfg(feature = "enable_opcode_metrics")]
lazy_static! {
    static ref OPCODE_DES_MAP: HashMap<u8, OpcodeInfo> = MERGE_MAP.iter().copied().collect();
}

#[cfg(feature = "enable_opcode_metrics")]
pub(crate) const MGAS_TO_GAS: u64 = 1_000_000u64;

#[cfg(feature = "enable_opcode_metrics")]
#[derive(Debug)]
pub(crate) struct OpcodeMergeRecord {
    count: u64,
    duration: Duration,
    count_percent: f64,
    duration_percent: f64,
    ave_cost: f64,
}

#[cfg(feature = "enable_opcode_metrics")]
#[derive(Debug)]
pub(crate) struct OpcodeStats {
    total_count: u64,
    total_duration: Duration,
    total_duration_percent: f64,
    count_percent: [f64; OPCODE_NUMBER],
    duration_percent: [f64; OPCODE_NUMBER],
    ave_cost: [f64; OPCODE_NUMBER],
    opcode_gas: [(f64, f64); OPCODE_NUMBER],
    total_gas: f64,
    merge_records: HashMap<&'static str, OpcodeMergeRecord>,
    opcode_record: RevmMetricRecord,
}

#[cfg(feature = "enable_opcode_metrics")]
impl OpcodeStats {
    pub(crate) fn print(&self, header: &str) {
        self.print_summay(header);
        self.print_opcode();
        self.print_sload_percentile();
        self.print_category();
    }

    fn base_gas(&self, opcode: u8) -> u64 {
        if !OPCODE_DES_MAP.contains_key(&opcode) {
            return 0u64
        }

        OPCODE_DES_MAP.get(&opcode).unwrap().gas
    }

    fn print_summay(&self, header: &str) {
        println!("\n");
        println!("{}", header);
        // println!("Total Count     : {:>20}", self.total_count);
        // println!("Total Time (s)  : {:>20.2}", self.total_duration.as_secs_f64());
        // println!(
        //     "Avg   Cost (ns) : {:>20.1}",
        //     self.total_duration.as_nanos() as f64 / self.total_count as f64
        // );
        println!("");
    }

    fn print_opcode(&self) {
        let opcode_width = 15;
        let count_width = 12;
        let count_percent_width = 12;
        let time_width = 12;
        let time_percent_width = 12;
        let total_gas_width = 12;
        let total_gas_percent_width = 12;
        let cost_width = 12;
        let base_gas_width = 12;

        println!(
            "{:opcode_width$}{:>count_width$}{:>count_percent_width$}{:>time_width$}{:>time_percent_width$} \
            {:>total_gas_width$}{:>total_gas_percent_width$}{:>cost_width$}{:>base_gas_width$}", 
            "Opcode", 
            "Count", 
            "Count (%)", 
            "Time (s)", 
            "Time (%)", 
            "Total Mgas",
            "Gas (%)",
            "Cost (ns)", 
            "Base gas");

        let avg_cost = self.total_duration.as_nanos() as f64 / self.total_count as f64;
        println!(
            "{:opcode_width$}{:>count_width$}{:>count_percent_width$.3}{:>time_width$.2}{:>time_percent_width$.3} \
            {:>total_gas_width$.2}{:>total_gas_percent_width$.2}{:>cost_width$.1}{:>base_gas_width$}",
            "Overall",
            self.total_count,
            100f64,
            self.total_duration.as_secs_f64(),
            self.total_duration_percent * 100.0,
            self.total_gas,
            100f64,
            avg_cost,
            "NAN",
        );

        for i in 0..OPCODE_NUMBER {
            let op = i as u8;
            if !OPCODE_DES_MAP.contains_key(&op) {
                continue
            }
            let opcode_jump = OpCode::try_from_u8(op);
            if opcode_jump.is_none() {
                continue
            }

            println!(
                "{:opcode_width$}{:>count_width$}{:>count_percent_width$.3}{:>time_width$.2}{:>time_percent_width$.3} \
                {:>total_gas_width$.2}{:>total_gas_percent_width$.2}{:>cost_width$.1}{:>base_gas_width$}",
                opcode_jump.unwrap().as_str(),
                self.opcode_record.opcode_record[i].0,
                self.count_percent[i] * 100.0,
                self.opcode_record.opcode_record[i].1.as_secs_f64(),
                self.duration_percent[i] * 100.0,
                self.opcode_gas[i].0,
                self.opcode_gas[i].1*100.0,
                self.ave_cost[i],
                self.base_gas(op),
            );
        }
    }
    fn print_category(&self) {
        let cat_width = 13;
        let count_width = 18;
        let time_width = 13;
        let cost_width = 13;
        let count_percent_width = 13;
        let time_percent_width = 13;

        println!("\n");
        println!("==========================================================================================");
        println!("{:cat_width$}{:>count_width$}{:>time_width$}{:>cost_width$}{:>count_percent_width$}{:>time_percent_width$}", 
                "Opcode Cat.", 
                "Count", 
                "Time (s)", 
                "Cost (ns)", 
                "Count (%)", 
                "Time (%)");
        for (k, v) in self.merge_records.iter() {
            println!(
                "{:cat_width$}{:count_width$}{:>time_width$.2}{:>cost_width$.1}{:>count_percent_width$.3}{:>time_percent_width$.3}",
                *k,
                v.count,
                v.duration.as_secs_f64(),
                v.ave_cost,
                v.count_percent * 100.0,
                v.duration_percent * 100.0,
            );
        }
    }

    fn print_sload_percentile(&self) {
        let total_cnt: u128 = self.opcode_record.sload_opcode_record.iter().map(|&v| v.1).sum();
        println!("\n");
        println!("================================sload time percentile=====================================");
        println!("Time (us)    Percentile (%)");
        let mut max_per = 0.0;
        for value in self.opcode_record.sload_opcode_record.iter() {
            let p = value.1 as f64 / total_cnt as f64;
            if value.0 == u128::MAX {
                max_per = p;
                break
            }
            println!("{:5} {:15.3}", value.0, p * 100.0);
        }
        println!("{:>5} {:15.3}", "MAX", max_per * 100.0);
    }
}

#[cfg(feature = "enable_opcode_metrics")]
#[derive(Default, Debug)]
pub(crate) struct RevmMetricTimeDisplayer {
    /// revm metric recoder
    revm_metric_record: RevmMetricRecord,
}

#[cfg(feature = "enable_opcode_metrics")]
impl RevmMetricTimeDisplayer {
    pub(crate) fn update_metric_record(&mut self, record: &mut RevmMetricRecord) {
        self.revm_metric_record.update(record);
    }

    fn category_name(&self, opcode: u8) -> &'static str {
        if OPCODE_DES_MAP.contains_key(&opcode) {
            return OPCODE_DES_MAP.get(&opcode).unwrap().category
        }

        return &"unknow"
    }

    fn caculate_gas(&self, opcode: u8, record: &(u64, Duration, i128)) -> f64 {
        if !OPCODE_DES_MAP.contains_key(&opcode) {
            return 0.0
        }

        let opcode_info = OPCODE_DES_MAP.get(&opcode).unwrap();
        if opcode_info.static_gas {
            return opcode_info.gas.checked_mul(record.0).unwrap_or(0) as f64
        }

        return record.2 as f64
    }

    fn pure_metric_record(&self) -> RevmMetricRecord {
        const RDTSC_OVERHEAD: u64 = 7;
        let mut pure_record = self.revm_metric_record.clone();
        for (i, v) in self.revm_metric_record.opcode_record.iter().enumerate() {
            let rdtsc_overhead: u64 = v.0 * RDTSC_OVERHEAD;
            if v.1.as_nanos() < rdtsc_overhead as u128 {
                panic!("rdtsc overhead too larget");
            }
            pure_record.opcode_record[i].1 = v.1 - Duration::from_nanos(rdtsc_overhead);
        }

        pure_record
    }

    pub(crate) fn stats(&self, metric_record: &RevmMetricRecord) -> OpcodeStats {
        let mut merge_records: HashMap<&'static str, OpcodeMergeRecord> = HashMap::new();
        let mut total_count: u64 = 0;
        let total_duration: Duration = metric_record.total_time;
        let mut total_duration_percent: f64 = 0.0;

        for (i, v) in metric_record.opcode_record.iter().enumerate() {
            total_count = total_count.checked_add(v.0).expect("overflow");
            let cat = self.category_name(i as u8);

            merge_records
                .entry(cat)
                .and_modify(|r| {
                    r.count += v.0;
                    r.duration += v.1;
                })
                .or_insert(OpcodeMergeRecord {
                    count: v.0,
                    duration: v.1,
                    count_percent: 0.0,
                    duration_percent: 0.0,
                    ave_cost: 0.0,
                });
        }

        let mut opcode_gas: [(f64, f64); 256] = [(0.0, 0.0); 256];
        let mut total_gas: f64 = 0.0;
        for (i, v) in metric_record.opcode_record.iter().enumerate() {
            let op = i as u8;
            let op_gas = self.caculate_gas(op, v);
            opcode_gas[i].0 = op_gas / MGAS_TO_GAS as f64;
            if opcode_gas[i].0 > 0.0 {
                total_gas += opcode_gas[i].0;
            } else {
                total_gas -= opcode_gas[i].0;
            }
        }

        let mut count_percent: [f64; OPCODE_NUMBER] = [0.0; OPCODE_NUMBER];
        let mut duration_percent: [f64; OPCODE_NUMBER] = [0.0; OPCODE_NUMBER];
        let mut ave_cost: [f64; OPCODE_NUMBER] = [0.0; OPCODE_NUMBER];
        for (i, v) in self.revm_metric_record.opcode_record.iter().enumerate() {
            count_percent[i] = v.0 as f64 / total_count as f64;
            duration_percent[i] = v.1.as_nanos() as f64 / total_duration.as_nanos() as f64;

            total_duration_percent += duration_percent[i];
            ave_cost[i] = v.1.as_nanos() as f64 / v.0 as f64;
            opcode_gas[i].1 = opcode_gas[i].0 / total_gas;
        }

        for (_, value) in merge_records.iter_mut() {
            value.count_percent = value.count as f64 / total_count as f64;
            value.duration_percent =
                value.duration.as_nanos() as f64 / total_duration.as_nanos() as f64;
            value.ave_cost = value.duration.as_nanos() as f64 / value.count as f64;
        }

        OpcodeStats {
            total_count,
            total_duration,
            total_duration_percent,
            count_percent,
            duration_percent,
            ave_cost,
            opcode_gas,
            total_gas,
            merge_records,
            opcode_record: metric_record.clone(),
        }
    }

    pub(crate) fn print(&self) {
        let stat = self.stats(&self.revm_metric_record);
        stat.print("===============================Metric of instruction==========================================================");

        let pure_metric_record = self.pure_metric_record();
        let pure_stat = self.stats(&pure_metric_record);
        pure_stat.print("===============================Metric of pure instruction==========================================================");
        println!("\n");
    }
}

#[cfg(feature = "enable_execution_duration_record")]
#[derive(Default, Debug)]
pub(crate) struct ExecutionDurationDisplayer {
    excution_duration_record: ExecutionDurationRecord,
}

#[cfg(feature = "enable_execution_duration_record")]
impl ExecutionDurationDisplayer {
    pub(crate) fn update_excution_duration_record(&mut self, record: ExecutionDurationRecord) {
        self.excution_duration_record.add(record);
    }

    pub(crate) fn print(&self) {
        self.excution_duration_record.print("===============================Metric of execution duration==========================================================");

        // let pure_record = self.excution_duration_record.pure_record();
        // pure_record.print("===============================Metric of pure execution
        // duration==========================================================");
    }
}

#[cfg(feature = "enable_db_speed_record")]
#[derive(Default, Debug)]
pub(crate) struct DBSpeedDisplayer {
    db_speed_record: DbSpeedRecord,
}

#[cfg(feature = "enable_db_speed_record")]
impl DBSpeedDisplayer {
    pub(crate) fn update_db_speed_record(&mut self, record: DbSpeedRecord) {
        self.db_speed_record.add(record);
    }

    pub(crate) fn print(&self) {
        self.db_speed_record.print("===============================Metric of db speed==========================================================");

        // let pure_record = self.db_speed_record.pure_record();
        // pure_record.print("===============================Metric of pure db
        // speed==========================================================");
    }
}

#[cfg(feature = "enable_cache_record")]
#[derive(Default, Debug)]
pub(crate) struct CacheDBRecordDisplayer {
    cache_db_record: CacheDbRecord,
}

#[cfg(feature = "enable_cache_record")]
impl CacheDBRecordDisplayer {
    pub(crate) fn update_cache_db_record(&mut self, record: CacheDbRecord) {
        self.cache_db_record.update(&record);
    }

    fn print_header(&self, times_name: &str, percentiles_name: &str) {
        let col_funciotns_len = 20;
        let col_times_len = 24;
        let col_percentage_len = 20;

        println! {"{:col_funciotns_len$}{:>col_times_len$}{:>col_percentage_len$}", "CacheDb functions", times_name, percentiles_name};
    }

    fn print_line(&self, function: &str, times: u64, percentiles: f64) {
        let col_funciotns_len = 10;
        let col_times_len = 15;
        let col_percentage_len = 20;

        println!(
            "{:col_funciotns_len$}{:>col_times_len$}{:>col_percentage_len$.3}",
            function, times, percentiles
        );
    }

    pub(crate) fn print(&self) {
        let cache_db_record = &self.cache_db_record;

        println!("===============================Metric of CacheDb========================================================");

        let total_in_basic = cache_db_record.total_in_basic();
        let total_in_code_by_hash = cache_db_record.total_in_code_by_hash();
        let total_in_storage = cache_db_record.total_in_storage();
        let total_in_block_hash = cache_db_record.total_in_block_hash();

        let total_hits = cache_db_record.total_hits();
        let total_miss = cache_db_record.total_miss();
        let total_count = total_hits + total_miss;

        let hits_in_basic_pencentage =
            cache_db_record.hits.hits_in_basic as f64 / total_in_basic as f64;
        let hits_in_code_by_hash_pencentage =
            cache_db_record.hits.hits_in_code_by_hash as f64 / total_in_code_by_hash as f64;
        let hits_in_storage_pencentage =
            cache_db_record.hits.hits_in_storage as f64 / total_in_storage as f64;
        let hits_in_block_hash_pencentage =
            cache_db_record.hits.hits_in_block_hash as f64 / total_in_block_hash as f64;
        let total_hits_pencentage = total_hits as f64 / total_count as f64;

        println!("===============================Hit in CacheDb===========================================================");
        self.print_header("Hits", "Hit ratio (%)");
        self.print_line(
            "hit_in_basic                 ",
            cache_db_record.hits.hits_in_basic,
            hits_in_basic_pencentage * 100.0,
        );
        self.print_line(
            "hit_in_code_by_hash          ",
            cache_db_record.hits.hits_in_code_by_hash,
            hits_in_code_by_hash_pencentage * 100.0,
        );
        self.print_line(
            "hit_in_storage               ",
            cache_db_record.hits.hits_in_storage,
            hits_in_storage_pencentage * 100.0,
        );
        self.print_line(
            "hit_in_block_hash            ",
            cache_db_record.hits.hits_in_block_hash,
            hits_in_block_hash_pencentage * 100.0,
        );
        self.print_line("total hits                   ", total_hits, total_hits_pencentage * 100.0);

        let misses_in_basic_pencentage =
            cache_db_record.misses.misses_in_basic as f64 / total_in_basic as f64;
        let misses_in_code_by_hash_pencentage =
            cache_db_record.misses.misses_in_code_by_hash as f64 / total_in_code_by_hash as f64;
        let misses_in_storage_pencentage =
            cache_db_record.misses.misses_in_storage as f64 / total_in_storage as f64;
        let misses_in_block_hash_pencentage =
            cache_db_record.misses.misses_in_block_hash as f64 / total_in_block_hash as f64;
        let total_misses_pencentage = total_miss as f64 / total_count as f64;

        println!("===============================Miss in CacheDb===========================================================");
        self.print_header("Misses", "Miss ratio (%)");
        self.print_line(
            "miss_in_basic                ",
            cache_db_record.misses.misses_in_basic,
            misses_in_basic_pencentage * 100.0,
        );
        self.print_line(
            "miss_in_code_by_hash         ",
            cache_db_record.misses.misses_in_code_by_hash,
            misses_in_code_by_hash_pencentage * 100.0,
        );
        self.print_line(
            "miss_in_storage              ",
            cache_db_record.misses.misses_in_storage,
            misses_in_storage_pencentage * 100.0,
        );
        self.print_line(
            "miss_in_block_hash           ",
            cache_db_record.misses.misses_in_block_hash,
            misses_in_block_hash_pencentage * 100.0,
        );
        self.print_line(
            "total misses                 ",
            total_miss,
            total_misses_pencentage * 100.0,
        );

        let col_len = 20;
        let total_penalty_times = cache_db_record.total_penalty_times();
        println!("===============================Misses penalty in CacheDb=================================================");
        println! {"CacheDb functions                     Penalty time(min)"};
        println! {"{:col_len$}{:col_len$.3}", "miss_penalty_in_basic       ", cache_db_record.penalty.penalty_in_basic.as_secs_f64() / 60.0};
        println! {"{:col_len$}{:col_len$.3}", "miss_penalty_in_code_by_hash", cache_db_record.penalty.penalty_in_code_by_hash.as_secs_f64() / 60.0};
        println! {"{:col_len$}{:col_len$.3}", "miss_penalty_in_storage     ", cache_db_record.penalty.penalty_in_storage.as_secs_f64() / 60.0};
        println! {"{:col_len$}{:col_len$.3}", "miss_penalty_in_block_hash  ", cache_db_record.penalty.penalty_in_block_hash.as_secs_f64() / 60.0};
        println! {"{:col_len$}{:col_len$.3}", "total penalty time          ", total_penalty_times / 60.0};

        println!();
    }
}

#[cfg(feature = "enable_tps_gas_record")]
#[derive(Debug)]
pub(crate) struct TpsAndGasRecordDisplayer {
    pub(crate) delta_txs: u128,
    pub(crate) delta_gas: u128,

    pub(crate) last_instant: minstant::Instant,
}

#[cfg(feature = "enable_tps_gas_record")]
impl TpsAndGasRecordDisplayer {
    const N: u64 = 1000;

    pub(crate) fn update_tps_and_gas(&mut self, block_number: u64, txs: u64, gas: u64) {
        self.delta_txs = self.delta_txs.checked_add(txs as u128).expect("overflow");
        self.delta_gas = self.delta_gas.checked_add(gas as u128).expect("overflow");

        if 0 == block_number % Self::N {
            self.print();
        }
    }

    pub(crate) fn print(&mut self) {
        let elapsed_ns = self.last_instant.elapsed().as_nanos();
        let tps = self.delta_txs.mul(1000_000_000).div(elapsed_ns);
        let mgas_ps = (self.delta_gas as f64).mul(1000_000_000 as f64).div(elapsed_ns as f64);

        self.delta_txs = 0;
        self.delta_gas = 0;
        self.last_instant = Instant::now();

        println!("\n===============================Metric of tps and gas==========================================================");
        println!("elapsed(ns) : {:?}", elapsed_ns);
        println!("TPS : {:?}", tps);
        println!("MGas: {:.3}\n", mgas_ps);
    }

    pub(crate) fn start_record(&mut self) {
        self.delta_txs = 0;
        self.delta_gas = 0;
        self.last_instant = Instant::now();
    }

    pub(crate) fn stop_record(&mut self) {
        self.print();
    }
}

#[cfg(feature = "enable_tps_gas_record")]
impl Default for TpsAndGasRecordDisplayer {
    fn default() -> Self {
        Self { delta_txs: 0, delta_gas: 0, last_instant: Instant::now() }
    }
}
