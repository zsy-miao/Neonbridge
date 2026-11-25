// Inspect the console body through GPU virtual addresses after queued copies.
ByteAddressBuffer firstBody : register(t0);
ByteAddressBuffer nextBody : register(t1);
ByteAddressBuffer clonedBody : register(t2);
RWByteAddressBuffer result : register(u0);
[numthreads(1,1,1)] void main() {
    for (uint offset = 0; offset < 64; offset += 16) {
        result.Store4(offset, firstBody.Load4(offset));
        result.Store4(64 + offset, nextBody.Load4(offset));
        result.Store4(128 + offset, clonedBody.Load4(offset));
    }
}
