# Nibble Storage Analyzer for Reth

This tool analyzes the nibble storage in Reth's database tables (StoragesTrie and AccountsTrie) and calculates potential space savings with different encoding strategies.

## Current Storage Format

### StoragesTrie Table
- Uses `StoredNibblesSubKey` which always stores 65 bytes:
  - 64 bytes: Padded nibbles (right-padded with zeros)
  - 1 byte: Length indicator

### AccountsTrie Table
- Uses `StoredNibbles` which stores variable-length nibbles (one nibble per byte)
- No padding - stores only the actual nibbles needed

## Proposed Encoding Options

### Option 1: Packed + Length Byte
- Pack nibbles (2 nibbles per byte)
- Add 1 byte to store the total number of nibbles
- Formula: `ceil(nibbles/2) + 1` bytes

### Option 2: Packed + Even/Odd Parity Byte  
- Pack nibbles (2 nibbles per byte)
- Add 1 byte to indicate even/odd number of nibbles for disambiguation
- Formula: `ceil(nibbles/2) + 1` bytes

## Building the Analyzer

```bash
cd /Users/dan/projects/claude-workspaces/reth-nibble-storage-analysis
cargo build -p reth-cli-commands --bin nibble-analyzer --release
```

## Running the Analyzer

```bash
# Basic usage
./target/release/nibble-analyzer --datadir /data/mainnet/reth

# With verbose output (shows progress)
./target/release/nibble-analyzer --datadir /data/mainnet/reth --verbose
```

## Example Output

The analyzer will output:
1. Statistics for each table (StoragesTrie and AccountsTrie)
2. Current storage usage broken down by:
   - Nibbles/keys storage
   - Values storage  
   - Total table size
3. Projected storage with each encoding option showing:
   - Nibbles-only savings (reduction in nibble storage)
   - Total table savings (reduction in overall table size)
4. Space savings shown as both:
   - Absolute bytes and MB
   - Percentage reduction relative to current nibbles storage
   - Percentage reduction relative to total table size
5. Distribution of nibble lengths
6. Most common nibble lengths

## Key Insights

The analyzer helps answer critical questions:
- **What percentage of table size is used for nibbles vs values?** This shows whether optimizing nibble storage is worthwhile.
- **How much would packed encodings save?** Shows savings both as a percentage of nibble storage and total table size.
- **Which table benefits more?** StoragesTrie (with padding) vs AccountsTrie (without padding).
- **What's the nibble length distribution?** Helps understand if variable-length or fixed-length encodings would be better.

Important notes:
- StoragesTrie always uses 65 bytes per entry due to padding to 64 nibbles
- AccountsTrie uses variable length with no padding
- Both proposed encoding options provide identical space savings since they both use `ceil(nibbles/2) + 1` bytes
- The actual impact depends on the ratio of nibbles to values in your database

## Implementation Notes

The analyzer:
1. Opens the Reth database in read-only mode
2. Iterates through all entries in StoragesTrie and AccountsTrie tables
3. Calculates current storage usage and projected usage with different encodings
4. Generates a comprehensive report with statistics

The analysis is non-invasive and does not modify the database in any way.