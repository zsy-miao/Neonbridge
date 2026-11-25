// Standalone control: report failure without returning unverified shadow data.
ByteAddressBuffer shadow : register(t0);
ByteAddressBuffer validation : register(t1);
RWByteAddressBuffer result : register(u0);
[numthreads(1,1,1)] void main() {
    [branch] if (validation.Load(0) != 1) {
        for (uint offset = 0; offset < 64; offset += 16)
            result.Store4(offset, uint4(0xdeadbeef, 0xdeadbeef, 0xdeadbeef, 0xdeadbeef));
        return;
    }
    for (uint offset = 0; offset < 64; offset += 16)
        result.Store4(offset, shadow.Load4(offset));
}
