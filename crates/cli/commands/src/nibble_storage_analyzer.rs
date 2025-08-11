use reth_db::init_db;
use reth_db_api::{cursor::{DbCursorRO, DbDupCursorRO}, database::Database, transaction::DbTx};
use reth_trie_common::{StoredNibbles, StoredNibblesSubKey};
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
}

/// Statistics for nibble storage analysis
#[derive(Default, Debug)]
struct NibbleStats {
    /// Total number of entries
    total_entries: usize,
    /// Current total bytes used (with padding)
    current_total_bytes: usize,
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
    
    fn add_subkey_entry(&mut self, nibbles: &StoredNibblesSubKey) {
        let nibble_count = nibbles.len();
        
        // Update statistics
        self.total_entries += 1;
        
        // Current storage: always 65 bytes (64 padded nibbles + 1 length byte)
        self.current_total_bytes += 65;
        
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
    
    fn add_nibbles_entry(&mut self, nibbles: &StoredNibbles) {
        let nibble_count = nibbles.0.len();
        
        // Update statistics
        self.total_entries += 1;
        
        // For StoredNibbles, current storage is just the nibble count (no padding)
        self.current_total_bytes += nibble_count;
        
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
    
    fn print_summary(&self) {
        println!("\n=== Nibble Storage Analysis Report ===\n");
        println!("Total entries analyzed: {}", self.total_entries);
        println!("\n--- Current Storage Format ---");
        println!("Format: Padded to 64 nibbles + length byte");
        println!("Total bytes: {} ({:.2} MB)", 
            self.current_total_bytes, 
            self.current_total_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Average bytes per entry: {:.2}", 
            self.current_total_bytes as f64 / self.total_entries as f64
        );
        
        println!("\n--- Option 1: Packed + Length Byte ---");
        println!("Format: Pack nibbles (2 per byte) + 1 byte for length");
        println!("Total bytes: {} ({:.2} MB)", 
            self.packed_with_length_bytes, 
            self.packed_with_length_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Average bytes per entry: {:.2}", 
            self.packed_with_length_bytes as f64 / self.total_entries as f64
        );
        let savings1 = self.current_total_bytes - self.packed_with_length_bytes;
        let savings1_pct = (savings1 as f64 / self.current_total_bytes as f64) * 100.0;
        println!("Space saved: {} bytes ({:.2} MB) - {:.2}% reduction", 
            savings1, 
            savings1 as f64 / (1024.0 * 1024.0),
            savings1_pct
        );
        
        println!("\n--- Option 2: Packed + Even/Odd Parity Byte ---");
        println!("Format: Pack nibbles (2 per byte) + 1 byte for even/odd disambiguation");
        println!("Total bytes: {} ({:.2} MB)", 
            self.packed_with_parity_bytes, 
            self.packed_with_parity_bytes as f64 / (1024.0 * 1024.0)
        );
        println!("Average bytes per entry: {:.2}", 
            self.packed_with_parity_bytes as f64 / self.total_entries as f64
        );
        let savings2 = self.current_total_bytes - self.packed_with_parity_bytes;
        let savings2_pct = (savings2 as f64 / self.current_total_bytes as f64) * 100.0;
        println!("Space saved: {} bytes ({:.2} MB) - {:.2}% reduction", 
            savings2, 
            savings2 as f64 / (1024.0 * 1024.0),
            savings2_pct
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

pub fn analyze_storage_tries(datadir: &Path, verbose: bool) -> eyre::Result<()> {
    println!("Opening database at: {:?}", datadir);
    
    // Open the database
    let db = init_db(datadir, Default::default())?;
    
    // Analyze StoragesTrie table
    println!("\nAnalyzing StoragesTrie table...");
    let mut storage_stats = NibbleStats::new();
    
    {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_dup_read::<reth_db::tables::StoragesTrie>()?;
        
        let mut count = 0;
        // Iterate through all entries in the dup-sorted table
        while let Some((_key, entry)) = cursor.next()? {
            storage_stats.add_subkey_entry(&entry.nibbles);
            count += 1;
            
            if verbose && count % 100_000 == 0 {
                println!("Processed {} entries...", count);
            }
        }
    }
    
    println!("\n=== StoragesTrie Table ===");
    storage_stats.print_summary();
    
    // Analyze AccountsTrie table
    println!("\n\nAnalyzing AccountsTrie table...");
    let mut account_stats = NibbleStats::new();
    
    {
        let tx = db.tx()?;
        let mut cursor = tx.cursor_read::<reth_db::tables::AccountsTrie>()?;
        
        let mut count = 0;
        while let Some((key, _value)) = cursor.next()? {
            account_stats.add_nibbles_entry(&key);
            count += 1;
            
            if verbose && count % 100_000 == 0 {
                println!("Processed {} entries...", count);
            }
        }
    }
    
    println!("\n=== AccountsTrie Table ===");
    account_stats.print_summary();
    
    // Combined summary
    println!("\n\n=== COMBINED SUMMARY ===");
    let total_current = storage_stats.current_total_bytes + account_stats.current_total_bytes;
    let total_packed_length = storage_stats.packed_with_length_bytes + account_stats.packed_with_length_bytes;
    let total_packed_parity = storage_stats.packed_with_parity_bytes + account_stats.packed_with_parity_bytes;
    
    println!("Total current storage: {} bytes ({:.2} MB)", 
        total_current, 
        total_current as f64 / (1024.0 * 1024.0)
    );
    
    println!("\nWith packed + length encoding:");
    let total_savings1 = total_current - total_packed_length;
    let total_savings1_pct = (total_savings1 as f64 / total_current as f64) * 100.0;
    println!("  Total: {} bytes ({:.2} MB)", 
        total_packed_length, 
        total_packed_length as f64 / (1024.0 * 1024.0)
    );
    println!("  Savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        total_savings1, 
        total_savings1 as f64 / (1024.0 * 1024.0),
        total_savings1_pct
    );
    
    println!("\nWith packed + even/odd parity encoding:");
    let total_savings2 = total_current - total_packed_parity;
    let total_savings2_pct = (total_savings2 as f64 / total_current as f64) * 100.0;
    println!("  Total: {} bytes ({:.2} MB)", 
        total_packed_parity, 
        total_packed_parity as f64 / (1024.0 * 1024.0)
    );
    println!("  Savings: {} bytes ({:.2} MB) - {:.2}% reduction", 
        total_savings2, 
        total_savings2 as f64 / (1024.0 * 1024.0),
        total_savings2_pct
    );
    
    Ok(())
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    let datadir = Path::new(&args.datadir);
    
    if !datadir.exists() {
        return Err(eyre::eyre!("Database directory does not exist: {:?}", datadir));
    }
    
    analyze_storage_tries(datadir, args.verbose)?;
    
    Ok(())
}