# From Raiko
# https://github.com/taikoxyz/raiko/blob/6a8421d5c17a7d8846c22f8f986d9584a6885bf1/script/install.sh

#!/usr/bin/env bash

# Any error will result in failure
set -e

# Check if the RISC-V GCC prebuilt binary archive already exists
if [ -f /tmp/riscv32-unknown-elf.gcc-13.2.0.tar.gz ]; then
    echo "riscv-gcc-prebuilt existed, please check the file manually"
else
    # Download the file using wget
    wget -O /tmp/riscv32-unknown-elf.gcc-13.2.0.tar.gz https://github.com/stnolting/riscv-gcc-prebuilt/releases/download/rv32i-131023/riscv32-unknown-elf.gcc-13.2.0.tar.gz
    # Check if wget succeeded
    if [ $? -ne 0 ]; then
        echo "Failed to download riscv-gcc-prebuilt"
        exit 1
    fi
    # Create the directory if it doesn't exist
    if [ ! -d /opt/riscv ]; then
        mkdir /opt/riscv
    fi
    # Extract the downloaded archive
    tar -xzf /tmp/riscv32-unknown-elf.gcc-13.2.0.tar.gz -C /opt/riscv/
    # Check if tar succeeded
    if [ $? -ne 0 ]; then
        echo "Failed to extract riscv-gcc-prebuilt"
        exit 1
    fi
fi
