# Building for ARM devices

Reth can be build for and run on ARM devices, but there are a few things to take into considerations before.

## CPU Architecture

First, you must have a 64-bit CPU and Operating System, otherwise some of the project dependencies will not be able to compile or be executed.

## Memory Layout on AArch64

Then, you must setup the virtual memory layout in such a way that the user space is sufficiently large.
From [the Linux Kernel documentation](https://www.kernel.org/doc/html/v5.3/arm64/memory.html#:~:text=AArch64%20Linux%20uses%20either%203,for%20both%20user%20and%20kernel.), you can see that the memory layout with 4KB pages and a level-3 translation table limits the user space to 512GB, which is too low for Reth to sync on Ethereum mainnet.

## Build Reth

If both your CPU architecture and the memory layout are valid, the instructions for building Reth will not differ from [the standard process](https://paradigmxyz.github.io/reth/installation/source.html).

## Troubleshooting

> If you ever need to recompile the Linux Kernel because the official OS images for your ARM board don't have the right memory layout configuration, you can use [the Armbian build framework](https://github.com/armbian/build).

### Failed to open database

> This error is documented [here](https://github.com/paradigmxyz/reth/issues/2211).

This error is raised whenever MBDX can not open a database due to the limitations imposed by the memory layout of your kernel. If the user space is limited to 512GB, the database will not be able to grow below this size.

You will need to recompile the Linux Kernel to fix the issue.

A simple and safe approach to achieve this is to use the Armbian build framework to create a new image of the OS that will be flashed to a storage device of your choice - an SD card for example - with the following kernel feature values:
- **Page Size**: 64 KB
- **Virtual Address Space Size**: 48 Bits

To be able to build an Armbian image and set those values, you will need to:
- Clone the Armbian build framework repository
```shell
git clone https://github.com/armbian/build
cd build
```
- Run the compile script with the following parameters:
```shell
./compile.sh \
BUILD_MINIMAL=yes \
BUILD_DESKTOP=no \
KERNEL_CONFIGURE=yes \
CARD_DEVICE="/dev/sdX" # Replace sdX with your own storage device
```
- From there, you will be able to select the target board, the OS release and branch. Then, once you get in the **Kernel Configuration** screen, select the **Kernel Features options** and set the previous values accordingly.
- Wait for the process to finish, plug your storage device into your board and start it. You can now download or install Reth and it should work properly.
