#!/usr/bin/env bash

# Usage: ./fetch_superchain_config.sh
# Switch to the same directory as the script and run it.
# This will checkout the latest superchain registry commit and copy
# the superchain configs to the script directory.

# Requires:
# - MacOS: brew install qpdf zstd yq

SCRIPT_DIR=$(pwd)
TEMP_DIR=$(mktemp -d)

# Clone the repository and go to the directory
git clone --depth 1 https://github.com/ethereum-optimism/superchain-registry.git "$TEMP_DIR"
# shellcheck disable=SC2164
cd "$TEMP_DIR"


DICT_FILE="$TEMP_DIR/superchain/extra/dictionary"
TARGET_PATH="$TEMP_DIR/result"
GENESIS_TARGET_PATH="$TARGET_PATH/genesis"
CONFIGS_TARGET_PATH="$TARGET_PATH/configs"

GENESIS_SRC_DIR="$TEMP_DIR/superchain/extra/genesis"
CONFIGS_SRC_DIR="$TEMP_DIR/superchain/configs"
mkdir -p "$GENESIS_TARGET_PATH"
mkdir -p "$CONFIGS_TARGET_PATH"

# Convert TOML files to JSON
# JSON makes the handling in no-std environments easier
find "$CONFIGS_SRC_DIR" -type f -name "*.toml" | while read -r file; do

    # Compute destination file path
    REL_PATH="${file#"$CONFIGS_SRC_DIR"/}"
    DEST_PATH="$CONFIGS_TARGET_PATH/${REL_PATH%.toml}"

    # Ensure destination directory exists
    mkdir -p "$(dirname "$DEST_PATH")"

    # Convert the toml file to json
    yq -p toml -o json "$file" > "$DEST_PATH.json"
done


# Extract and compress the genesis files
# Again, we compress the genesis files with zlib-flate to make
# them easier to handle in no-std environments
find "$GENESIS_SRC_DIR" -type f -name "*.json.zst" | while read -r file; do

    # Compute destination file path
    REL_PATH="${file#"$GENESIS_SRC_DIR"/}"
    DEST_PATH="$GENESIS_TARGET_PATH/${REL_PATH%.zst}"

    # Ensure destination directory exists
    mkdir -p "$(dirname "$DEST_PATH")"

    # Extract the file
    zstd -q -d -D="$DICT_FILE" "$file" -o "$DEST_PATH"

    # Remove "config" field from genesis files, because it is not consistent populated.
    # We will add it back in from the chain config file during runtime.
    # See: https://github.com/ethereum-optimism/superchain-registry/issues/901
    jq 'del(.config)' "$DEST_PATH" > "$DEST_PATH.tmp"
    mv "$DEST_PATH.tmp" "$DEST_PATH"

    # Compress with zlib-flate and remove the original file
    zlib-flate -compress < "$DEST_PATH" > "$DEST_PATH.deflate"
    rm "$DEST_PATH"

done

# Save revision
git rev-parse HEAD > "$TARGET_PATH/COMMIT"

# Set the modification time of all files to 1980-01-01 to ensure the archive is deterministic
find "$TARGET_PATH" -exec touch -t 198001010000.00 {} +

# shellcheck disable=SC2164
cd "$TARGET_PATH"
# Create a tar archive excluding files that are not relevant superchain directory
# shellcheck disable=SC2035
COPYFILE_DISABLE=1 tar --no-acls --no-xattrs -cf superchain-configs.tar --exclude "README.*" --exclude "._COMMIT" *

# Move result to the script directory
mv superchain-configs.tar "$SCRIPT_DIR"

# Clean up
# shellcheck disable=SC2164
cd "$TEMP_DIR/../"
rm -rf "$TEMP_DIR"
