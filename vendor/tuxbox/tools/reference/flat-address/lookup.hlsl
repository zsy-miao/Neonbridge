// Bounded reference for resolving guest 64-bit FLAT addresses. This is not
// enabled in the runtime. Only naturally aligned DWORD vectors are supported.
ByteAddressBuffer directory : register(t0); // entries {baseLo, baseHi, byteLength, slot}
ByteAddressBuffer data0 : register(t1);
ByteAddressBuffer data1 : register(t2);
ByteAddressBuffer data2 : register(t3);
ByteAddressBuffer queries : register(t4); // records {addressLo, addressHi, bytes, reserved}
RWByteAddressBuffer results : register(u0); // {four data DWORDs, status}
cbuffer Counts : register(b0) { uint entryCount; uint queryCount; }

bool lt64(uint2 a, uint2 b) { return a.y < b.y || (a.y == b.y && a.x < b.x); }
uint capacity(uint slot) {
    uint bytes = 0;
    if (slot == 0) data0.GetDimensions(bytes);
    else if (slot == 1) data1.GetDimensions(bytes);
    else if (slot == 2) data2.GetDimensions(bytes);
    return bytes;
}
uint4 load_value(uint slot, uint offset, uint bytes) {
    uint4 value = 0;
    [unroll] for (uint i=0; i<4; ++i) if (i*4 < bytes) {
        if (slot == 0) value[i] = data0.Load(offset+i*4);
        else if (slot == 1) value[i] = data1.Load(offset+i*4);
        else value[i] = data2.Load(offset+i*4);
    }
    return value;
}
// 0 = missing, 1 = resolved, 2 = overlapping/ambiguous, 3 = invalid.
[numthreads(64,1,1)] void main(uint3 id : SV_DispatchThreadID) {
    uint dirBytes, queryBytes, resultBytes;
    directory.GetDimensions(dirBytes); queries.GetDimensions(queryBytes); results.GetDimensions(resultBytes);
    uint safeCount=min(queryCount,resultBytes/20);
    if (id.x>=safeCount) return;
    uint at=id.x*20;
    if (entryCount>3 || entryCount>dirBytes/16 || queryCount>queryBytes/16 || queryCount>resultBytes/20) {
        results.Store4(at,0); results.Store(at+16,3); return;
    }
    uint4 entries[3]; uint2 ends[3];
    [loop] for (uint i=0;i<entryCount;++i) {
        entries[i]=directory.Load4(i*16);
        uint2 base=entries[i].xy;
        uint len=entries[i].z, slot=entries[i].w;
        uint low=base.x+len; uint carry=low<base.x ? 1u : 0u; uint high=base.y+carry;
        if ((base.x|base.y)==0 || (base.x&3)!=0 || len==0 || (len&3)!=0 ||
            slot>=3 || len>capacity(slot) || high<base.y) {
            results.Store4(at,0); results.Store(at+16,3); return;
        }
        ends[i]=uint2(low,high);
        [loop] for (uint j=0;j<i;++j) {
            if (slot==entries[j].w || (lt64(base,ends[j]) && lt64(entries[j].xy,ends[i]))) {
                results.Store4(at,0); results.Store(at+16,2); return;
            }
        }
    }
    uint4 q=queries.Load4(id.x*16); uint2 address=q.xy;
    if ((address.x|address.y)==0 || (address.x&3)!=0 || q.w!=0 ||
        (q.z!=4 && q.z!=8 && q.z!=12 && q.z!=16)) {
        results.Store4(at,0); results.Store(at+16,3); return;
    }
    [loop] for (uint i=0;i<entryCount;++i) {
        uint2 base=entries[i].xy;
        if (lt64(address,base)) continue;
        uint borrow=address.x<base.x ? 1u : 0u;
        uint high=address.y-base.y-borrow;
        uint offset=address.x-base.x;
        if (high!=0 || offset>entries[i].z || q.z>entries[i].z-offset) continue;
        results.Store4(at,load_value(entries[i].w,offset,q.z)); results.Store(at+16,1); return;
    }
    results.Store4(at,0); results.Store(at+16,0);
}
