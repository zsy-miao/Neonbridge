ByteAddressBuffer body : register(t0);
RWByteAddressBuffer result : register(u0);
[numthreads(1,1,1)] void main() {
    for (uint offset = 0; offset < 64; offset += 16)
        result.Store4(offset, body.Load4(offset));
}
