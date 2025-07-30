use clap::Parser;
use eyre::{eyre, Result};
use reth_cli_runner::CliContext;
use reth_node_ethereum::EthereumNode;
use reth_provider::providers::ReadOnlyConfig;
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    Nibbles,
};
use reth_trie_db::DatabaseTrieCursorFactory;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::Instant,
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Measures performance of TrieCursor iteration over accounts trie"
)]
pub struct TrieCursorPerfCommand {
    /// Path to datadir (required unless --compare is used)
    #[arg(long, value_name = "PATH", required_unless_present = "compare")]
    pub datadir: Option<PathBuf>,

    /// Path to file containing paths to seek (overrides default iteration behavior)
    #[arg(long, value_name = "PATH")]
    pub seek: Option<PathBuf>,

    /// Randomize the order of paths before seeking (only applies with --seek)
    #[arg(long, requires = "seek")]
    pub seek_randomize: bool,

    /// Compare two output files and display histogram of duration differences
    #[arg(long, value_name = "FILE1,FILE2", value_delimiter = ',')]
    pub compare: Option<Vec<PathBuf>>,
}

impl TrieCursorPerfCommand {
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        // Handle compare mode if specified
        if let Some(ref compare_files) = self.compare {
            if compare_files.len() != 2 {
                return Err(eyre!(
                    "--compare requires exactly 2 files, got {}",
                    compare_files.len()
                ));
            }
            return self.compare_files(&compare_files[0], &compare_files[1]);
        }

        let datadir = self.datadir.ok_or_else(|| eyre!("--datadir is required"))?;

        info!("Opening database at {:?}", datadir);

        // Open the database in read-only mode using EthereumNode
        let factory = EthereumNode::provider_factory_builder()
            .open_read_only(Default::default(), ReadOnlyConfig::from_datadir(datadir.clone()))?;

        // Get a provider
        let provider = factory.provider()?;
        let tx = provider.tx_ref();

        // Create the trie cursor factory
        let trie_cursor_factory = DatabaseTrieCursorFactory::new(tx);

        // Create cursor for the accounts trie
        let mut cursor = trie_cursor_factory.account_trie_cursor()?;

        if let Some(seek_file) = self.seek {
            // Seek mode: read paths from file and seek to each one
            info!("Reading paths from {:?}", seek_file);

            let file = File::open(&seek_file)?;
            let reader = BufReader::new(file);
            let mut paths = Vec::new();

            // Read paths from file
            for line in reader.lines() {
                let line = line?;
                // Parse the line to extract the path (format: "Nibbles(0x...) <nanos>")
                if let Some(path_str) = line.split_whitespace().next() {
                    // Remove "Nibbles(" prefix and ")" suffix
                    if path_str.starts_with("Nibbles(0x") && path_str.ends_with(")") {
                        let hex_str = &path_str[10..path_str.len() - 1];
                        // Parse hex string to nibbles directly (each hex char is one nibble)
                        let mut nibble_vec = Vec::with_capacity(hex_str.len());
                        for ch in hex_str.chars() {
                            let nibble = ch
                                .to_digit(16)
                                .ok_or_else(|| eyre!("Invalid hex character: {}", ch))?
                                as u8;
                            nibble_vec.push(nibble);
                        }
                        let nibbles = Nibbles::from_nibbles(nibble_vec);
                        paths.push(nibbles);
                    }
                }
            }

            // Randomize paths if requested
            if self.seek_randomize {
                use rand::{rng, seq::SliceRandom};

                info!("Randomizing path order...");
                let mut rng = rng();
                paths.shuffle(&mut rng);
            }

            info!("Starting seek operations for {} paths...", paths.len());

            // Seek to each path and measure time
            for (idx, path) in paths.iter().enumerate() {
                let start = Instant::now();
                let result = cursor.seek(*path);
                let duration = start.elapsed();

                result?.ok_or(eyre!("path {path:?} not found!"))?;

                println!("{:?} {}", path, duration.as_nanos());

                if (idx + 1) % 1000 == 0 {
                    info!("Processed {} seeks", idx + 1);
                }
            }

            info!("Seek operations complete. Total paths: {}", paths.len());
        } else {
            // Default mode: iterate through trie
            info!("Starting iteration over accounts trie...");

            let mut count = 0;

            loop {
                let start = Instant::now();
                let result = cursor.next();
                let duration = start.elapsed();

                match result? {
                    Some((key, _value)) => {
                        println!("{:?} {}", key, duration.as_nanos());
                        count += 1;
                    }
                    None => {
                        info!("Iteration complete. Total nodes: {}", count);
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Compare two output files and display histogram of duration differences
    fn compare_files(&self, file1: &PathBuf, file2: &PathBuf) -> Result<()> {
        info!("Comparing {:?} and {:?}", file1, file2);

        let reader1 = BufReader::new(File::open(file1)?);
        let reader2 = BufReader::new(File::open(file2)?);

        let mut lines1 = reader1.lines();
        let mut lines2 = reader2.lines();

        // Histogram with 1 microsecond buckets (stored as microseconds)
        let mut histogram: BTreeMap<i64, u64> = BTreeMap::new();
        let mut line_count = 0;

        loop {
            let line1 = lines1.next();
            let line2 = lines2.next();

            match (line1, line2) {
                (Some(Ok(l1)), Some(Ok(l2))) => {
                    line_count += 1;

                    // Parse both lines
                    let parts1: Vec<&str> = l1.split_whitespace().collect();
                    let parts2: Vec<&str> = l2.split_whitespace().collect();

                    if parts1.len() < 2 || parts2.len() < 2 {
                        return Err(eyre!("Invalid line format at line {}", line_count));
                    }

                    // Compare paths
                    if parts1[0] != parts2[0] {
                        return Err(eyre!(
                            "Path mismatch at line {}: '{}' vs '{}'",
                            line_count,
                            parts1[0],
                            parts2[0]
                        ));
                    }

                    // Parse durations (in nanoseconds)
                    let duration1: u64 = parts1[1].parse()?;
                    let duration2: u64 = parts2[1].parse()?;

                    // Calculate difference in microseconds (file2 - file1)
                    let diff_nanos = duration2 as i64 - duration1 as i64;
                    let diff_micros = diff_nanos / 1000; // Convert to microseconds

                    // Add to histogram
                    *histogram.entry(diff_micros).or_insert(0) += 1;
                }
                (None, None) => break,
                (Some(_), None) => return Err(eyre!("File 1 has more lines than file 2")),
                (None, Some(_)) => return Err(eyre!("File 2 has more lines than file 1")),
                (Some(Err(e)), _) | (_, Some(Err(e))) => {
                    return Err(eyre!("Error reading file: {}", e))
                }
            }
        }

        info!("Processed {} lines", line_count);

        // Display histogram
        self.display_histogram(&histogram);

        Ok(())
    }

    /// Display histogram with horizontal bars
    fn display_histogram(&self, histogram: &BTreeMap<i64, u64>) {
        if histogram.is_empty() {
            println!("No data to display");
            return;
        }

        // Find max count for scaling
        let max_count = *histogram.values().max().unwrap();
        let bar_width = 50; // Maximum bar width in characters

        println!("\nHistogram of duration differences (file2 - file1):");
        println!("Bucket (μs) | Count | Bar");
        println!("{:-<60}", "");

        for (bucket, count) in histogram {
            let bar_length = ((*count as f64 / max_count as f64) * bar_width as f64) as usize;
            let bar = "█".repeat(bar_length);

            println!("{:>11} | {:>5} | {}", bucket, count, bar);
        }

        // Print summary statistics
        let total: u64 = histogram.values().sum();
        let mean: f64 =
            histogram.iter().map(|(bucket, count)| *bucket as f64 * *count as f64).sum::<f64>() /
                total as f64;

        println!("\nSummary:");
        println!("Total entries: {}", total);
        println!("Mean difference: {:.2} μs", mean);

        // Find median
        let mut cumulative = 0;
        let median_pos = total / 2;
        let mut median = 0;
        for (bucket, count) in histogram {
            cumulative += count;
            if cumulative >= median_pos {
                median = *bucket;
                break;
            }
        }
        println!("Median difference: {} μs", median);
    }
}
