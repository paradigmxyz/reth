//! GPU Art Execution Extension for Reth
//!
//! This module implements a GPU-accelerated particle art generator as an Execution Extension for Reth.
//! It generates unique particle art for each new block in the Ethereum chain using Metal GPU acceleration.

use image::ImageBuffer;
use metal::{ComputePipelineDescriptor, ComputePipelineState, Device, MTLResourceOptions, MTLSize};
use objc::rc::autoreleasepool;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::{self, info};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Represents the GPU Art Execution Extension
///
/// This struct holds the necessary components to generate particle art using GPU acceleration.
struct GpuArtExEx<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    device: Device,
    pipeline_state: ComputePipelineState,
}

impl<Node: FullNodeComponents> GpuArtExEx<Node> {
    /// Creates a new instance of GpuArtExEx
    ///
    /// # Arguments
    ///
    /// * `ctx` - The Execution Extension context
    ///
    /// # Returns
    ///
    /// A new instance of GpuArtExEx
    pub(crate) fn new(ctx: ExExContext<Node>) -> Self {
        let device = Device::system_default().expect("No Metal device found");
        let pipeline_state = Self::create_pipeline_state(&device);
        Self { ctx, device, pipeline_state }
    }

    /// Creates a compute pipeline state for GPU operations
    ///
    /// # Arguments
    ///
    /// * `device` - The Metal device to create the pipeline state for
    ///
    /// # Returns
    ///
    /// A new ComputePipelineState
    fn create_pipeline_state(device: &Device) -> ComputePipelineState {
        let shader_source = include_str!("particle_system.metal");
        let library =
            device.new_library_with_source(shader_source, &metal::CompileOptions::new()).unwrap();
        let kernel = library.get_function("particle_system", None).unwrap();

        let pipeline_state_descriptor = ComputePipelineDescriptor::new();
        pipeline_state_descriptor.set_compute_function(Some(&kernel));

        device.new_compute_pipeline_state_with_function(&kernel).unwrap()
    }

    /// Generates particle art for a given block
    ///
    /// # Arguments
    ///
    /// * `block` - The sealed block to generate art for
    ///
    /// # Returns
    ///
    /// A vector of bytes representing the generated image
    fn generate_particle_art(&self, block: &reth_primitives::SealedBlock) -> Vec<u8> {
        autoreleasepool(|| {
            let command_queue = self.device.new_command_queue();

            let width = 1024;
            let height = 1024;
            let buffer_length = width * height * 4;

            let output_buffer =
                self.device.new_buffer(buffer_length as u64, MTLResourceOptions::StorageModeShared);

            let command_buffer = command_queue.new_command_buffer();
            let compute_encoder = command_buffer.new_compute_command_encoder();

            compute_encoder.set_compute_pipeline_state(&self.pipeline_state);
            compute_encoder.set_buffer(0, Some(&output_buffer), 0);

            // Pack block data into a buffer
            let block_data = [
                block.number,
                block.timestamp,
                block.gas_used,
                u64::from_str_radix(&block.hash().to_string()[2..18], 16).unwrap_or(0),
            ];
            let block_buffer = self.device.new_buffer_with_data(
                block_data.as_ptr() as *const std::ffi::c_void,
                std::mem::size_of_val(&block_data) as u64,
                MTLResourceOptions::StorageModeShared,
            );
            compute_encoder.set_buffer(1, Some(&block_buffer), 0);

            let thread_group_size = MTLSize::new(16, 16, 1);
            let thread_groups =
                MTLSize::new((width as u64 + 15) / 16, (height as u64 + 15) / 16, 1);

            compute_encoder.dispatch_thread_groups(thread_groups, thread_group_size);
            compute_encoder.end_encoding();

            command_buffer.commit();
            command_buffer.wait_until_completed();

            let data_ptr = output_buffer.contents() as *const u8;
            unsafe { std::slice::from_raw_parts(data_ptr, buffer_length).to_vec() }
        })
    }
}

impl<Node: FullNodeComponents> Future for GpuArtExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        while let Poll::Ready(Some(notification)) = this.ctx.notifications.poll_recv(cx) {
            if let Some(committed_chain) = notification.committed_chain() {
                for (_, block) in committed_chain.blocks() {
                    let art_data = this.generate_particle_art(block);
                    if let Err(e) = save_particle_art(&art_data, block.number) {
                        info!("Failed to save particle art for block {}: {}", block.number, e);
                    } else {
                        info!("Generated and saved particle art for block {}", block.number);
                    }
                }

                this.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Poll::Pending
    }
}

/// Initializes and returns the GPU Art Execution Extension.
///
/// # Arguments
///
/// * `ctx` - The Execution Extension context
///
/// # Returns
///
/// A Future that resolves to a Result containing another Future
pub(crate) fn init_gpu_art_exex<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> impl Future<Output = eyre::Result<impl Future<Output = eyre::Result<()>>>> {
    async move { Ok(GpuArtExEx::new(ctx)) }
}

/// Saves the generated particle art to a file
///
/// # Arguments
///
/// * `data` - The raw image data
/// * `block_number` - The block number for which the art was generated
///
/// # Returns
///
/// A Result indicating success or failure
fn save_particle_art(data: &[u8], block_number: u64) -> eyre::Result<()> {
    let width = 1024;
    let height = 1024;

    // Create an ImageBuffer with explicit type annotations
    let img: ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(width, height, data.to_vec())
            .ok_or_else(|| eyre::eyre!("Failed to create image from raw data"))?;

    // Create the gpu_art directory if it doesn't exist
    std::fs::create_dir_all("gpu_art")?;

    // Save the image with a formatted filename
    let filename = format!("gpu_art/block_{}.png", block_number);
    img.save(&filename)?;

    // Log successful save operation
    tracing::info!("Saved particle art for block {} as {}", block_number, filename);

    Ok(())
}

// TODO: Consider adding error handling for GPU operations
// TODO: Implement a mechanism to handle potential GPU resource exhaustion
// TODO: Add configuration options for image size and output directory
