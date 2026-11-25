cbuffer Constants : register(b0) { uint4 values[4096]; };
RWByteAddressBuffer result : register(u0);
[numthreads(1,1,1)]
void main() {
    result.Store4(16, values[0]);
    result.Store4(32, values[1]);
    result.Store4(48, values[2048]);
    result.Store4(64, values[4095]);
}
