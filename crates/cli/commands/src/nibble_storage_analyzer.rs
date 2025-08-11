use reth_db::{init_db, mdbx::{DatabaseArguments, MaxReadTransactionDuration}};
use reth_db_api::{cursor::DbCursorRO, database::Database, transaction::DbTx};
use reth_trie_common::{StoredNibbles, StoredNibblesSubKey};
use reth_codecs::Compact;
use std::path::Path;
use clap::Parser;

/// Command-line arguments for the nibble storage analyzer
#[derive(Debug, Parser)]
#[command(name = "nibble-analyzer")]
#[command(about = "Analyzes nibble storage in Reth database and calculates potential space savings")]
pub struct Args {
    /// Path to the database directory
    #[arg(long, value_name = "PATH")]
    pub datadir: String,
    
    /// Show detailed statistics
    #[arg(long)]
    pub verbose: bool,
    
    /// Disable read transaction timeout (useful for large databases)
    #[arg(long)]
    pub no_timeout: bool,
}

/// Statistics for nibble storage analysis
#[derive(Default, Debug)]
struct NibbleStats {
    /// Total number of entries
    total_entries: usize,
    /// Current total bytes used for nibbles/keys
    current_nibbles_bytes: usize,
    /// Total table size (keys + values)
    total_table_bytes: usize,
    /// Bytes for values only
    value_bytes: usize,
    /// Bytes if using packed + length encoding
    packed_with_length_bytes: usize,
    /// Bytes if using packed + even/odd byte encoding
    packed_with_parity_bytes: usize,
    /// Distribution of nibble lengths
    length_distribution: Vec<usize>,
}

impl NibbleStats {
    fn new() -> Self {
        Self {
            length_distribution: vec![0; 65], // 0 to 64 nibbles
            ..Default::default()
        }
    }
    
    fn add_subkey_entry(&mut self, nibbles: &StoredNibblesSubKey, value_size: usize) {
        let nibble_count = nibbles.len();
        
        // Update statistics
        self.total_entries += 1;
        
        // Current storage: always 65 bytes (64 padded nibbles + 1 length byte)
        let nibbles_storage = 65;
        self.current_nibbles_bytes += nibbles_storage;
        self.value_bytes += value_size;
        self.total_table_bytes += nibbles_storage + value_size;
        
        // Update length distribution
        if nibble_count <= 64 {
            self.length_distribution[nibble_count] += 1;
        }
        
        // Calculate packed representations
        
        // Option 1: Packed + length byte
        // Formula: ceil(nibbles/2) + 1 (for length byte)
        let packed_bytes = (nibble_count + 1) / 2;
        self.packed_with_length_bytes += packed_bytes + 1;
        
        // Option 2: Packed + even/odd parity byte
        // Formula: ceil(nibbles/2) + 1 (for parity byte)
        // The parity byte would indicate if we have even or odd number of nibbles
        self.packed_with_parity_bytes += packed_bytes + 1;
    }
    
    fn add_nibbles_entry(&mut self, nibbles: &StoredNibbles, value_size: usize) {
        let nibble_count = nibbles.0.len();
        
        // Update statistics
        self.total_entries += 1;
        
        // For StoredNibbles, current storage is just the nibble count (no padding)
        self.current_nibbles_bytes += nibble_count;
        self.value_bytes += value_size;
        self.total_table_bytes += nibble_count + value_size;
        
        // Update length distribution
        if nibble_count <= 64 {
            self.length_distribution[nibble_count] += 1;
        }
        
        // Calculate packed representations
        
        // Option 1: Packed + length byte
        let packed_bytes = (nibble_count + 1) / 2;
        self.packed_with_length_bytes += packed_bytes + 1;
        
        // Option 2: Packed + even/odd parity byte
        self.packed_with_parity_bytes += packed_bytes + 1;
    }
    
    fn print_summary(&self, table_name: &str) {
        println!("\n=== {} Analysis Report ===", table_name);
        println!("Total entries analyzed: {}", self.total_entries);
        
        println!("\n--- Current Storage ---");
        println!("Nibbles/keys only: {} bytes ({:.2} MB)", 
            self.current_nibbles_bytes, 
            self.current_nibbles_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Values only: {} bytes ({:.2} MB)", 
            self.value_bytes, 
            self.value_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Total table size: {} bytes ({:.2} MB)", 
            self.total_table_bytes, 
            self.total_table_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Nibbles as % of table: {:.2}%",
            (self.current_nibbles_bytes as f64 / self.total_table_bytes as f64) * 100.0
        );
        println!("Average nibbles bytes per entry: {:.2}", 
            self.current_nibbles_bytes as f64 / self.total_entries as f64
        );
        
        println!("\n--- Option 1: Packed + Length Byte ---");
        println!("Format: Pack nibbles (2 per byte) + 1 byte for length");
        println!("Nibbles bytes: {} ({:.2} MB)", 
            self.packed_with_length_bytes, 
            self.packed_with_length_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Average nibbles bytes per entry: {:.2}", 
            self.packed_with_length_bytes as f64 / self.total_entries as f64
        );
        
        let nibbles_savings1 = self.current_nibbles_bytes.saturating_sub(self.packed_with_length_bytes);
        let nibbles_savings1_pct = (nibbles_savings1 as f64 / self.current_nibbles_bytes as f64) * 100.0;
        println!("Nibbles space saved: {} bytes ({:.2} MB) - {:.2}% reduction", 
            nibbles_savings1, 
            nibbles_savings1 as f64 / (1024.0 * 1024.0),
            nibbles_savings1_pct
        );
        
        let new_table_size1 = self.packed_with_length_bytes + self.value_bytes;
        let table_savings1 = self.total_table_bytes.saturating_sub(new_table_size1);
        let table_savings1_pct = (table_savings1 as f64 / self.total_table_bytes as f64) * 100.0;
        println!("Total table reduction: {} bytes ({:.2} MB) - {:.2}% of table", 
            table_savings1, 
            table_savings1 as f64 / (1024.0 * 1024.0),
            table_savings1_pct
        );
        
        println!("\n--- Option 2: Packed + Even/Odd Parity Byte ---");
        println!("Format: Pack nibbles (2 per byte) + 1 byte for even/odd disambiguation");
        println!("Nibbles bytes: {} ({:.2} MB)", 
            self.packed_with_parity_bytes, 
            self.packed_with_parity_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Average nibbles bytes per entry: {:.2}", 
            self.packed_with_parity_bytes as f64 / self.total_entries as f64
        );
        
        let nibbles_savings2 = self.current_nibbles_bytes.saturating_sub(self.packed_with_parity_bytes);
        let nibbles_savings2_pct = (nibbles_savings2 as f64 / self.current_nibbles_bytes as f64) * 100.0;
        println!("Nibbles space saved: {} bytes ({:.2} MB) - {:.2}% reduction", 
            nibbles_savings2, 
            nibbles_savings2 as f64 / (1024.0 * 1024.0),
            nibbles_savings2_pct
        );
        
        let new_table_size2 = self.packed_with_parity_bytes + self.value_bytes;
        let table_savings2 = self.total_table_bytes.saturating_sub(new_table_size2);
        let table_savings2_pct = (table_savings2 as f64 / self.total_table_bytes as f64) * 100.0;
        println!("Total table reduction: {} bytes ({:.2} MB) - {:.2}% of table", 
            table_savings2, 
            table_savings2 as f64 / (1024.0 * 1024.0),
            table_savings2_pct
        );
        
        println!("\n--- Nibble Length Distribution ---");
        println!("Length | Count | Percentage");
        println!("-------|-------|------------");
        for (length, count) in self.length_distribution.iter().enumerate() {
            if *count > 0 {
                let percentage = (*count as f64 / self.total_entries as f64) * 100.0;
                println!("{:6} | {:6} | {:10.2}%", length, count, percentage);
            }
        }
        
        // Calculate most common lengths
        let mut length_counts: Vec<(usize, usize)> = self.length_distribution
            .iter()
            .enumerate()
            .filter(|(_, &count)| count > 0)
            .map(|(length, &count)| (length, count))
            .collect();
        length_counts.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
        
        println!("\n--- Most Common Nibble Lengths ---");
        for (i, &(length, count)) in length_counts.iter().take(5).enumerate() {
            let percentage = (count as f64 / self.total_entries as f64) * 100.0;
            println!("{}. Length {} nibbles: {} entries ({:.2}%)", 
                i + 1, length, count, percentage);
        }
    }
}

pub fn analyze_storage_tries(datadir: &Path, verbose: bool, no_timeout: bool) -> eyre::Result<()> {
    println!("Opening database at: {:?}", datadir);
    
    // Open the database with optional timeout disabled
    let mut db_args = DatabaseArguments::default();
    if no_timeout {
        db_args = db_args.with_max_read_transaction_duration(
            Some(MaxReadTransactionDuration::Unbounded)
        );
        println!("Read transaction timeout disabled");
    }
    let db = init_db(datadir, db_args)?;
    
    // Analyze StoragesTrie table
    println!("\nAnalyzing StoragesTrie table...");
    let mut storage_stats = NibbleStats::new();
    
    {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_dup_read::<reth_db::tables::StoragesTrie>()?;
        
        let mut count = 0;
        // Iterate through all entries in the dup-sorted table
        while let Some((_key, entry)) = cursor.next()? {
            // Calculate size of the encoded value
            let value_size = entry.node.to_compact(&mut Vec::new());
            storage_stats.add_subkey_entry(&entry.nibbles, value_size);
            count += 1;
            
            if verbose && count % 100_000 == 0 {
                println!("Processed {} entries...", count);
            }
        }
    }
    
    storage_stats.print_summary("StoragesTrie Table");
    
    // Analyze AccountsTrie table
    println!("\n\nAnalyzing AccountsTrie table...");
    let mut account_stats = NibbleStats::new();
    
    {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_read::<reth_db::tables::AccountsTrie>()?;
        
        let mut count = 0;
        while let Some((key, value)) = cursor.next()? {
            // Calculate size of the encoded value
            let value_size = value.to_compact(&mut Vec::new());
            account_stats.add_nibbles_entry(&key, value_size);
            count += 1;
            
            if verbose && count % 100_000 == 0 {
                println!("Processed {} entries...", count);
            }
        }
    }
    
    account_stats.print_summary("AccountsTrie Table");
    
    // Combined summary
    println!("\n\n=== COMBINED SUMMARY ===");
    let total_current_nibbles = storage_stats.current_nibbles_bytes + account_stats.current_nibbles_bytes;
    let total_current_values = storage_stats.value_bytes + account_stats.value_bytes;
    let total_current_tables = storage_stats.total_table_bytes + account_stats.total_table_bytes;
    let total_packed_length = storage_stats.packed_with_length_bytes + account_stats.packed_with_length_bytes;
    let total_packed_parity = storage_stats.packed_with_parity_bytes + account_stats.packed_with_parity_bytes;
    
    println!("Current storage:");
    println!("  Nibbles/keys: {} bytes ({:.2} MB)", 
        total_current_nibbles, 
        total_current_nibbles as f64 / (1024.0 * 1024.0)
    );
    println!("  Values: {} bytes ({:.2} MB)", 
        total_current_values, 
        total_current_values as f64 / (1024.0 * 1024.0)
    );
    println!("  Total tables: {} bytes ({:.2} MB)", 
        total_current_tables, 
        total_current_tables as f64 / (1024.0 * 1024.0)
    );
    println!("  Nibbles as % of tables: {:.2}%",
        (total_current_nibbles as f64 / total_current_tables as f64) * 100.0
    );
    
    println!("\nWith packed + length encoding:");
    let nibbles_savings1 = total_current_nibbles.saturating_sub(total_packed_length);
    let nibbles_savings1_pct = (nibbles_savings1 as f64 / total_current_nibbles as f64) * 100.0;
    println!("  New nibbles size: {} bytes ({:.2} MB)", 
        total_packed_length, 
        total_packed_length as f64 / (1024.0 * 1024.0)
    );
    println!("  Nibbles savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        nibbles_savings1, 
        nibbles_savings1 as f64 / (1024.0 * 1024.0),
        nibbles_savings1_pct
    );
    let new_total_tables1 = total_packed_length + total_current_values;
    let table_savings1 = total_current_tables.saturating_sub(new_total_tables1);
    let table_savings1_pct = (table_savings1 as f64 / total_current_tables as f64) * 100.0;
    println!("  New total tables: {} bytes ({:.2} MB)", 
        new_total_tables1, 
        new_total_tables1 as f64 / (1024.0 * 1024.0)
    );
    println!("  Total table savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        table_savings1, 
        table_savings1 as f64 / (1024.0 * 1024.0),
        table_savings1_pct
    );
    
    println!("\nWith packed + even/odd parity encoding:");
    let nibbles_savings2 = total_current_nibbles.saturating_sub(total_packed_parity);
    let nibbles_savings2_pct = (nibbles_savings2 as f64 / total_current_nibbles as f64) * 100.0;
    println!("  New nibbles size: {} bytes ({:.2} MB)", 
        total_packed_parity, 
        total_packed_parity as f64 / (1024.0 * 1024.0)
    );
    println!("  Nibbles savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        nibbles_savings2, 
        nibbles_savings2 as f64 / (1024.0 * 1024.0),
        nibbles_savings2_pct
    );
    let new_total_tables2 = total_packed_parity + total_current_values;
    let table_savings2 = total_current_tables.saturating_sub(new_total_tables2);
    let table_savings2_pct = (table_savings2 as f64 / total_current_tables as f64) * 100.0;
    println!("  New total tables: {} bytes ({:.2} MB)", 
        new_total_tables2, 
        new_total_tables2 as f64 / (1024.0 * 1024.0)
    );
    println!("  Total table savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        table_savings2, 
        table_savings2 as f64 / (1024.0 * 1024.0),
        table_savings2_pct
    );
    
    Ok(())
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    let datadir = Path::new(&args.datadir);
    
    if !datadir.exists() {
        return Err(eyre::eyre!("Database directory does not exist: {:?}", datadir));
    }
    
    analyze_storage_tries(datadir, args.verbose, args.no_timeout)?;
    
    Ok(())
}