#[cfg(feature = "enable_opcode_metrics")]
use super::dashboard_opcode::*;
#[cfg(feature = "enable_opcode_metrics")]
use revm::OpCode;
#[cfg(any(feature = "enable_opcode_metrics", feature = "enable_cache_record"))]
use revm_utils::time_utils::convert_cycles_to_duration;
#[cfg(feature = "enable_opcode_metrics")]
use revm_utils::{time_utils::convert_cycles_to_ns_f64, types::RevmMetricRecord};
#[cfg(feature = "enable_opcode_metrics")]
use std::collections::BTreeMap;

#[cfg(feature = "enable_cache_record")]
use revm_utils::types::CacheDbRecord;

#[cfg(feature = "enable_tps_gas_record")]
use minstant::Instant;
#[cfg(feature = "enable_tps_gas_record")]
use std::ops::{Div, Mul};

#[cfg(feature = "enable_execution_duration_record")]
use reth_stages::ExecutionDurationRecord;

#[cfg(feature = "enable_db_speed_record")]
use reth_stages::DbSpeedRecord;

#[cfg(feature = "enable_opcode_metrics")]
const MGAS_TO_GAS: u64 = 1_000_000u64;
#[cfg(any(feature = "enable_opcode_metrics", feature = "enable_cache_record"))]
const COL_WIDTH_MIDDLE: usize = 14;
#[cfg(feature = "enable_cache_record")]
const COL_WIDTH_BIG: usize = 20;
#[cfg(feature = "enable_cache_record")]
const COL_WIDTH_LARGE: usize = 30;

#[cfg(feature = "enable_opcode_metrics")]
struct OpcodeMergeRecord {
    count: u64,
    duration: u64,
    count_percent: f64,
    duration_percent: f64,
    ave_cost: f64,
}

#[cfg(feature = "enable_opcode_metrics")]
pub(crate) struct OpcodeStats {
    total_count: u64,
    total_duration: u64,
    total_duration_percent: f64,
    count_percent: [f64; OPCODE_NUMBER],
    duration_percent: [f64; OPCODE_NUMBER],
    ave_cost: [f64; OPCODE_NUMBER],
    opcode_gas: [(f64, f64); OPCODE_NUMBER],
    total_gas: f64,
    merge_records: BTreeMap<&'static str, OpcodeMergeRecord>,
    opcode_record: RevmMetricRecord,
}

#[cfg(feature = "enable_opcode_metrics")]
impl OpcodeStats {
    pub(crate) fn print(&self) {
        self.print_header();
        self.print_opcode();
        self.print_sload_percentile();
        self.print_category();
        println!("\n");
    }

    fn base_gas(&self, opcode: u8) -> Option<u64> {
        Some(MERGE_MAP[opcode as usize]?.1.gas)
    }

    fn print_header(&self) {
        println!("\n===============================Metric of instruction==========================================================\n");
    }

    fn print_opcode_line(
        &self,
        opcode_jump: &str,
        count: u64,
        count_percent: f64,
        time: f64,
        time_percent: f64,
        cost: f64,
        total_gas: f64,
        gas_percent: f64,
        base_gas: u64,
    ) {
        println!(
            "{: <COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$.3}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$.3} \
            {:>COL_WIDTH_MIDDLE$.1}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$}",
            opcode_jump,
            count,
            count_percent,
            time,
            time_percent,
            cost,
            total_gas,
            gas_percent,
            base_gas,
        );
    }

    fn print_opcode(&self) {
        println!(
            "{: <COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$} \
            {:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}", 
            "Opcode", 
            "Count", 
            "Count (%)", 
            "Time (s)", 
            "Time (%)", 
            "Cost (ns)", 
            "Total Mgas",
            "Gas (%)",
            "Base gas");

        let avg_cost = convert_cycles_to_ns_f64(self.total_duration) / self.total_count as f64;
        println!(
            "{: <COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$.3}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$.3} \
            {:>COL_WIDTH_MIDDLE$.1}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$}",
            "Overall",
            self.total_count,
            100f64,
            convert_cycles_to_duration(self.total_duration).as_secs_f64(),
            self.total_duration_percent * 100.0,
            avg_cost,
            self.total_gas,
            100f64,
            "NAN",
        );

        for i in 0..OPCODE_NUMBER {
            let op = i as u8;
            let opcode_jump = OpCode::try_from_u8(op);
            if opcode_jump.is_none() {
                continue
            }

            self.print_opcode_line(
                opcode_jump.unwrap().as_str(),
                self.opcode_record.opcode_record[i].0,
                self.count_percent[i] * 100.0,
                convert_cycles_to_duration(self.opcode_record.opcode_record[i].1).as_secs_f64(),
                self.duration_percent[i] * 100.0,
                self.ave_cost[i],
                self.opcode_gas[i].0,
                self.opcode_gas[i].1 * 100.0,
                self.base_gas(op).unwrap_or(0),
            );
        }
    }
    fn print_category(&self) {
        println!("\n");
        println!("==========================================================================================");
        println!("{: <COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}", 
                "Opcode Cat.", 
                "Count", 
                "Count (%)", 
                "Time (s)", 
                "Time (%)",
                "Cost (ns)", 
        );

        for (k, v) in self.merge_records.iter() {
            println!(
                "{: <COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$.2}{:>COL_WIDTH_MIDDLE$.1}{:>COL_WIDTH_MIDDLE$.3}{:>COL_WIDTH_MIDDLE$.3}",
                *k,
                v.count,
                v.count_percent * 100.0,
                convert_cycles_to_duration(v.duration).as_secs_f64(),
                v.duration_percent * 100.0,
                v.ave_cost,
            );
        }
    }

    fn print_sload_percentile(&self) {
        let total_cnt: u64 = self.opcode_record.sload_opcode_record.iter().map(|&v| v.1).sum();
        println!("\n");
        println!("================================sload time percentile=====================================");
        println!("Time (us)    Percentile (%)");
        let mut max_per = 0.0;
        for value in self.opcode_record.sload_opcode_record.iter() {
            let p = value.1 as f64 / total_cnt as f64;
            if value.0 == u64::MAX {
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

    fn category_name(&self, opcode: u8) -> Option<&'static str> {
        Some(MERGE_MAP[opcode as usize]?.1.category)
    }

    fn caculate_gas(&self, opcode: u8, record: &(u64, u64, i128)) -> f64 {
        let (base_gas, static_gas) = match MERGE_MAP[opcode as usize] {
            Some(opcode_info) => (opcode_info.1.gas, opcode_info.1.static_gas),
            None => return 0.0,
        };

        if static_gas {
            return base_gas.checked_mul(record.0).unwrap_or(0) as f64
        }

        record.2 as f64
    }

    pub(crate) fn stats(&self, metric_record: &RevmMetricRecord) -> OpcodeStats {
        let mut merge_records: BTreeMap<&'static str, OpcodeMergeRecord> = BTreeMap::new();
        let mut total_count: u64 = 0;
        let total_duration = metric_record.total_time;
        let mut total_duration_percent: f64 = 0.0;

        for (i, v) in metric_record.opcode_record.iter().enumerate() {
            total_count = total_count.checked_add(v.0).expect("overflow");
            let cat = match self.category_name(i as u8) {
                Some(name) => name,
                None => continue,
            };

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
            duration_percent[i] = v.1 as f64 / total_duration as f64;

            total_duration_percent += duration_percent[i];
            ave_cost[i] = convert_cycles_to_ns_f64(v.1) / v.0 as f64;
            opcode_gas[i].1 = opcode_gas[i].0 / total_gas;
        }

        for (_, value) in merge_records.iter_mut() {
            value.count_percent = value.count as f64 / total_count as f64;
            value.duration_percent = value.duration as f64 / total_duration as f64;
            value.ave_cost = convert_cycles_to_ns_f64(value.duration) / value.count as f64;
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
        stat.print();
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
        self.excution_duration_record.print();
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
        self.db_speed_record = record;
    }

    pub(crate) fn print(&self) {
        let header = "===============================Metric of db speed==========================================================";
        self.db_speed_record.print(header);
    }
}

#[cfg(feature = "enable_cache_record")]
#[derive(Default, Debug)]
pub(crate) struct CacheDBRecordDisplayer {
    cache_db_record: CacheDbRecord,
    pre_db_size: usize,
}

#[cfg(feature = "enable_cache_record")]
impl CacheDBRecordDisplayer {
    pub(crate) fn update_cache_db_record(&mut self, record: CacheDbRecord) {
        self.cache_db_record.update(&record);
    }

    pub(crate) fn update_size(&mut self, block_number: u64, cachedb_size: usize) {
        if self.pre_db_size != cachedb_size {
            println!("Block Number: {:?}, Cachedb Size(bytes): {:?}", block_number, cachedb_size);
            self.pre_db_size = cachedb_size;
        }
    }

    // fn print_header(&self, times_name: &str, percentiles_name: &str) {
    //     let col_funciotns_len = 20;
    //     let col_times_len = 24;
    //     let col_percentage_len = 20;

    //     println! {"{:col_funciotns_len$}{:>col_times_len$}{:>col_percentage_len$}", "CacheDb
    // functions", times_name, percentiles_name}; }

    fn print_line(&self, function: &str, hits: u64, misses: u64, misses_pct: f64) {
        println!(
            "{: <COL_WIDTH_BIG$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_BIG$.3}",
            function, hits, misses, misses_pct
        );
    }

    fn misses_in_basic_pencentage(&self) -> f64 {
        self.cache_db_record.misses.basic as f64 / self.cache_db_record.total_in_basic() as f64
    }

    fn misses_in_code_by_hash_pencentage(&self) -> f64 {
        self.cache_db_record.misses.code_by_hash as f64 /
            self.cache_db_record.total_in_code_by_hash() as f64
    }

    fn misses_in_storage_pencentage(&self) -> f64 {
        self.cache_db_record.misses.storage as f64 / self.cache_db_record.total_in_storage() as f64
    }

    fn misses_in_block_hash_pencentage(&self) -> f64 {
        self.cache_db_record.misses.block_hash as f64 /
            self.cache_db_record.total_in_block_hash() as f64
    }

    fn misses_in_load_account_pencentage(&self) -> f64 {
        self.cache_db_record.misses.load_account as f64 /
            self.cache_db_record.total_in_load_account() as f64
    }

    fn total_misses_pencentage(&self) -> f64 {
        self.cache_db_record.total_miss() as f64 /
            (self.cache_db_record.total_hits() + self.cache_db_record.total_miss()) as f64
    }

    pub(crate) fn print(&self) {
        println!("===============================Metric of CacheDb========================================================");
        println!("===============================Hit in CacheDb===========================================================");
        println!(
            "{: <COL_WIDTH_BIG$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_MIDDLE$}{:>COL_WIDTH_BIG$}",
            "CacheDb functions", "Hits", "Misses", "Miss ratio (%)",
        );
        self.print_line(
            "in_basic",
            self.cache_db_record.hits.basic,
            self.cache_db_record.misses.basic,
            self.misses_in_basic_pencentage(),
        );

        self.print_line(
            "in_code_by_hash",
            self.cache_db_record.hits.code_by_hash,
            self.cache_db_record.misses.code_by_hash,
            self.misses_in_code_by_hash_pencentage(),
        );

        self.print_line(
            "in_storage",
            self.cache_db_record.hits.storage,
            self.cache_db_record.misses.storage,
            self.misses_in_storage_pencentage(),
        );

        self.print_line(
            "in_block_hash",
            self.cache_db_record.hits.block_hash,
            self.cache_db_record.misses.block_hash,
            self.misses_in_block_hash_pencentage(),
        );

        self.print_line(
            "load_account",
            self.cache_db_record.hits.load_account,
            self.cache_db_record.misses.load_account,
            self.misses_in_load_account_pencentage(),
        );

        self.print_line(
            "total",
            self.cache_db_record.total_hits(),
            self.cache_db_record.total_miss(),
            self.total_misses_pencentage(),
        );

        let total_penalty_times = self.cache_db_record.total_penalty_times();
        println!("===============================Misses penalty in CacheDb=================================================");
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$}", "CacheDb functions", "Penalty time(min)"};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "miss_penalty_in_basic       ", convert_cycles_to_duration(self.cache_db_record.penalty.basic).as_secs_f64() / 60.0};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "miss_penalty_in_code_by_hash", convert_cycles_to_duration(self.cache_db_record.penalty.code_by_hash).as_secs_f64() / 60.0};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "miss_penalty_in_storage     ", convert_cycles_to_duration(self.cache_db_record.penalty.storage).as_secs_f64() / 60.0};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "miss_penalty_in_block_hash  ", convert_cycles_to_duration(self.cache_db_record.penalty.block_hash).as_secs_f64() / 60.0};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "miss_penalty_in_load_account", convert_cycles_to_duration(self.cache_db_record.penalty.load_account).as_secs_f64() / 60.0};
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.3}", "total penalty time          ", convert_cycles_to_duration(total_penalty_times).as_secs_f64() / 60.0};

        println!();
        println!();
        let total_us_count = self.cache_db_record.total_miss();
        println!("========================================Penalty percentile ===============================================");
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$}", "time (us)", "Count (%)"};
        for (i, v) in self.cache_db_record.penalty.us_percentile.iter().enumerate() {
            println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.6}", i+1, (*v * 100) as f64 / total_us_count as f64};
        }
        println!();
        println!();
        let total_ns_count = self.cache_db_record.penalty.us_percentile[0];
        println!("========================================Penalty percentile ===============================================");
        println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$}", "time (ns)", "Count (%)"};
        for (i, v) in self.cache_db_record.penalty.ns_percentile.iter().enumerate() {
            println! {"{: <COL_WIDTH_LARGE$}{:>COL_WIDTH_MIDDLE$.6}", i+1, (*v * 100) as f64 / total_ns_count as f64};
        }
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
