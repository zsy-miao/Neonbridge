cbuffer Source : register(b0) { uint magic; }
RWBuffer<uint> output : register(u0);

[numthreads(1, 1, 1)]
void main(uint3 index : SV_DispatchThreadID) {
    output[index.x] = magic ^ (index.x * 2654435761u);
}
