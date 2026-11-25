// Read GPU-visible leaf IDs for the bounded directory control. This does not
// validate graph topology or establish runtime provenance.
ByteAddressBuffer leaves : register(t0);
ByteAddressBuffer directory : register(t1);
RWByteAddressBuffer output_map : register(u0);
RWByteAddressBuffer status : register(u1);
cbuffer Sizes : register(b0) { uint leaf_bytes; uint output_bytes; uint count; uint directory_bytes; }

[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= count) return;
    if (count > output_bytes / 16 || directory_bytes < 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 h = directory.Load4(0);
    if (h.x != 1 || h.y == 0 || h.w > leaf_bytes || h.y > (directory_bytes - 16) / 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint start = 16 + h.y * 16;
    if (h.z > (directory_bytes - start) / 16) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 instance = directory.Load4(16);
    if (instance.z != 0 || instance.w != 0 || instance.x > h.z ||
        instance.y != count || instance.y > h.z - instance.x) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint4 entry = directory.Load4(start + (instance.x + id.x) * 16);
    if (entry.z != 0 || entry.w != 0 || entry.y > 1 || (entry.x & 63) != 0 ||
        entry.x > h.w || h.w - entry.x < 64) {
        uint ignored; status.InterlockedOr(0, 1, ignored); return;
    }
    uint descriptor = leaves.Load(entry.x + 48);
    uint second = leaves.Load(entry.x + 56);
    uint selectors = leaves.Load(entry.x + 60);
    bool paired = (descriptor & 0x80) != 0;
    uint selector = (selectors >> (entry.y * 8)) & 255;
    if ((descriptor & 0x7f) != 0 || selectors >> 16 != 0 || paired != (second != 0xffffffffu) ||
        paired != ((selectors >> 8) != 0) || (entry.y == 1 && !paired) ||
        (selector != 2 && selector != 4 && selector != 9)) {
        uint ignored; status.InterlockedOr(0, 2, ignored); return;
    }
    uint primitive = leaves.Load(entry.x + 52 + entry.y * 4);
    output_map.Store4(id.x * 16, uint4(descriptor >> 8, primitive, entry.x, entry.y));
}
