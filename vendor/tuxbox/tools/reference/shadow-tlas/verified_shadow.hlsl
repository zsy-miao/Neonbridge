// Standalone GPU verification control. Each stage is a separate dispatch;
// caller must insert a global UAV barrier between stages and synchronize the
// source against all other writers. All buffers use bounded descriptors. The
// caller must set groupsX/groupsY to the actual compare and commit dimensions.
// The host must reject a status view shorter than four bytes before dispatch;
// no safe status write exists in that case.
ByteAddressBuffer source : register(t0);
ByteAddressBuffer validated : register(t1);
RWByteAddressBuffer shadow : register(u0);
RWByteAddressBuffer status : register(u1);
cbuffer Count : register(b0) { uint bytes; uint groupsX; uint groupsY; }

bool invalid_views() {
    uint sourceBytes, validatedBytes, shadowBytes;
    source.GetDimensions(sourceBytes);
    validated.GetDimensions(validatedBytes);
    shadow.GetDimensions(shadowBytes);
    if (bytes == 0 || (bytes & 3) != 0 || bytes > 64 * 1024 * 1024 ||
        groupsX == 0 || groupsY == 0 || groupsX > 65535 || groupsY > 65535 ||
        bytes > sourceBytes || bytes > validatedBytes || bytes > shadowBytes) return true;
    uint requiredGroups = (bytes / 4 + 63) / 64;
    return groupsY < (requiredGroups + groupsX - 1) / groupsX;
}
uint index_of(uint3 id) { return id.x + id.y * groupsX * 64; }

[numthreads(64, 1, 1)] void initialize(uint3 id : SV_DispatchThreadID) {
    uint statusBytes; status.GetDimensions(statusBytes);
    if (id.x == 0 && id.y == 0 && statusBytes >= 4) status.Store(0, 0);
}
[numthreads(64, 1, 1)] void compare(uint3 id : SV_DispatchThreadID) {
    uint statusBytes; status.GetDimensions(statusBytes);
    if (statusBytes < 4) return;
    uint index = index_of(id);
    if (invalid_views()) {
        if (id.x == 0 && id.y == 0) status.Store(0, 3);
        return;
    }
    if (index < bytes / 4 && source.Load(index * 4) != validated.Load(index * 4)) {
        uint previous;
        status.InterlockedOr(0, 1, previous);
    }
}
[numthreads(64, 1, 1)] void commit(uint3 id : SV_DispatchThreadID) {
    uint statusBytes; status.GetDimensions(statusBytes);
    if (statusBytes < 4 || status.Load(0) != 0) return;
    uint index = index_of(id);
    if (index < bytes / 4) shadow.Store(index * 4, source.Load(index * 4));
}
[numthreads(64, 1, 1)] void finalize(uint3 id : SV_DispatchThreadID) {
    uint statusBytes; status.GetDimensions(statusBytes);
    if (statusBytes < 4 || id.x != 0 || id.y != 0) return;
    uint value = status.Load(0);
    status.Store(0, value == 0 ? 1 : value == 1 ? 2 : 3);
}
