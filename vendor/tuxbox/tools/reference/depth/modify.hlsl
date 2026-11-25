RWTexture2D<float> depth_plane : register(u0);

[numthreads(8, 8, 1)]
void main(uint2 position : SV_DispatchThreadID) {
    uint width, height;
    depth_plane.GetDimensions(width, height);
    if (position.x >= width || position.y >= height || (position.x + 3 * position.y) % 5 != 0)
        return;
    uint old_bits = asuint(depth_plane[position]);
    depth_plane[position] = asfloat(old_bits + position.x + 7 * position.y + 1);
}
