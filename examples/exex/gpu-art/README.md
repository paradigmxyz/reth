# GPU Art Execution Extension for Reth

This project implements a GPU-accelerated particle art generator as an Execution Extension (ExEx) for Reth, an Ethereum execution client. It generates unique particle art for each new block in the Ethereum chain using Metal GPU acceleration.

## Features

- Generates unique particle art for each Ethereum block
- Utilizes GPU acceleration via Metal API
- Integrates seamlessly with Reth as an Execution Extension

## Requirements

- Rust (latest stable version)
- Metal-compatible GPU (macOS)
- Reth and its dependencies


## How it Works

The GPU Art ExEx uses the following components:

1. `GpuArtExEx`: The main struct that implements the Execution Extension
2. `particle_system.metal`: A Metal shader that generates the particle art
3. `generate_particle_art`: A function that orchestrates the GPU operations
4. `save_particle_art`: A function that saves the generated art as PNG files

For each new block, the ExEx:

1. Extracts relevant data from the block (number, timestamp, gas used, and part of the hash)
2. Passes this data to the GPU shader
3. Generates a unique particle art image
4. Saves the image as a PNG file named `block_{number}.png`

## Configuration

Currently, the ExEx uses fixed parameters:

- Image size: 1024x1024 pixels
- Output directory: `gpu_art/`

Future versions may include configuration options for these parameters.

## Testing

To run the tests:

```
cargo test
```

The test suite includes a basic functionality test that ensures the ExEx can generate and save an image file.

