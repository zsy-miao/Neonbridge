ByteAddressBuffer tableInput : register(t0);
ByteAddressBuffer rootInput : register(t1);
RWByteAddressBuffer result : register(u0);
cbuffer Values : register(b0) { uint4 values; }
[numthreads(1, 1, 1)]
void main() {
    result.Store4(0, uint4(tableInput.Load(0), rootInput.Load(4), values.x, values.z));
}
