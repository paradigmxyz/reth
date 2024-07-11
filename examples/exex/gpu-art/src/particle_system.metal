#include <metal_stdlib>
using namespace metal;

kernel void particle_system(texture2d<float, access::write> output [[texture(0)]],
                            constant uint64_t *block_data [[buffer(1)]],
                            uint2 gid [[thread_position_in_grid]])
{
    float2 uv = float2(gid) / float2(output.get_width(), output.get_height());
    
    uint64_t block_number = block_data[0];
    uint64_t timestamp = block_data[1];
    uint64_t gas_used = block_data[2];
    uint64_t block_hash_part = block_data[3];
    
    float time = float(block_number % 1000000) / 1000000.0;
    float3 color = float3(0.0);
    
    for (int i = 0; i < 100; i++) {
        float2 particle_pos = float2(
            sin(time * float(i) + float(timestamp % 1000) / 1000.0),
            cos(time * float(i + 10) + float(gas_used % 1000000) / 1000000.0)
        );
        float distance = length(uv - (particle_pos * 0.5 + 0.5));
        float3 particle_color = float3(
            sin(float(block_hash_part % 256) / 256.0 * 6.28),
            sin(float((block_hash_part >> 8) % 256) / 256.0 * 6.28),
            sin(float((block_hash_part >> 16) % 256) / 256.0 * 6.28)
        );
        color += 0.5 * particle_color / (distance * 100.0 + 1.0);
    }
    
    output.write(float4(color, 1.0), gid);
}