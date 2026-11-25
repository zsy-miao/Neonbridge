// Guarded software traversal control for a validated sphere BLAS and the
// standalone two-instance synthetic TLAS. Not hardware-exact BVH instructions.
ByteAddressBuffer scene : register(t0);
ByteAddressBuffer body : register(t1);
ByteAddressBuffer rays : register(t2);
ByteAddressBuffer guard : register(t3);
RWByteAddressBuffer hits : register(u0);
cbuffer Count : register(b0) { uint rayCount; }

float3 f3(ByteAddressBuffer b, uint offset) { return asfloat(b.Load3(offset)); }
float half_at(uint offset) {
    uint pair = body.Load(offset & ~3u);
    return f16tof32((pair >> ((offset & 2u) * 8)) & 0xffffu);
}
bool box_hit(float3 lo, float3 hi, float3 origin, float3 direction, float limit) {
    float nearT = 0.0, farT = limit;
    [unroll] for (uint axis = 0; axis < 3; ++axis) {
        if (direction[axis] == 0.0) {
            if (origin[axis] < lo[axis] || origin[axis] > hi[axis]) return false;
        } else {
            float a = (lo[axis] - origin[axis]) / direction[axis];
            float b = (hi[axis] - origin[axis]) / direction[axis];
            nearT = max(nearT, min(a,b)); farT = min(farT, max(a,b));
        }
    }
    return nearT <= farT;
}
float triangle_hit(float3 origin, float3 direction, float3 a, float3 b, float3 c) {
    float3 e1=b-a, e2=c-a, p=cross(direction,e2);
    float determinant=dot(e1,p);
    if (abs(determinant) < 1e-9) return asfloat(0x7f800000u);
    float inv=1.0/determinant; float3 v=origin-a;
    float u=dot(v,p)*inv; if (u<0.0 || u>1.0) return asfloat(0x7f800000u);
    float3 q=cross(v,e1); float w=dot(direction,q)*inv;
    if (w<0.0 || u+w>1.0) return asfloat(0x7f800000u);
    float t=dot(e2,q)*inv;
    return t>=0.0 ? t : asfloat(0x7f800000u);
}
[numthreads(64,1,1)] void main(uint3 id : SV_DispatchThreadID) {
    uint i=id.x; if (i>=rayCount) return;
    uint outOffset=i*16;
    if (guard.Load(0) != 1) { hits.Store4(outOffset,uint4(0xfffffffeu,0,0,0)); return; }
    uint ray=i*36; float3 origin=f3(rays,ray), direction=f3(rays,ray+16);
    float maximum=asfloat(rays.Load(ray+28)); uint flags=rays.Load(ray+32);
    if (asfloat(rays.Load(ray+12)) != 0.0 || !all(isfinite(origin)) ||
        !all(isfinite(direction)) || all(direction == 0.0) || !isfinite(maximum) || maximum < 0.0 ||
        (flags != 0 && flags != 0x40 && flags != 0x80)) {
        hits.Store4(outOffset,uint4(0xfffffffdu,0,0,0)); return;
    }
    uint instance=0xffffffffu, primitive=0xffffffffu, geometry=0xffffffffu;
    float best=maximum; bool failed=false;
    if (flags != 0x80) {
        uint root=(scene.Load(48)>>3)*64;
        [loop] for (uint child=0; child<4; ++child) {
            uint pointer=scene.Load(root+child*4); if (pointer==0xffffffffu) continue;
            if ((pointer & 7) != 6) { failed=true; break; }
            uint bounds=root+16+child*24;
            if (!box_hit(f3(scene,bounds),f3(scene,bounds+12),origin,direction,best)) continue;
            uint node=(pointer>>3)*64;
            uint idmask=scene.Load(node+48); if ((idmask>>24)==0) continue;
            if (scene.Load(node+52) != 0 || scene.Load(node+60) != 0x100u) { failed=true; break; }
            float3 localOrigin,localDirection;
            [unroll] for (uint row=0; row<3; ++row) {
                float3 m=f3(scene,node+76+row*12);
                localOrigin[row]=asfloat(scene.Load(node+64+row*4))+dot(m,origin);
                localDirection[row]=dot(m,direction);
            }
            uint stack[128]; uint sp=1; stack[0]=scene.Load(node+56)&0x1fffffffu;
            [loop] while (sp>0) {
                uint p=stack[--sp]; uint kind=p&7u; uint at=(p>>3)*64;
                if (kind==4 || kind==5) {
                    [unroll] for (uint k=0;k<4;++k) {
                        uint next=body.Load(at+k*4); if (next==0xffffffffu) continue;
                        uint base=at+16+k*(kind==4?12:24);
                        float3 lo,hi;
                        if (kind==4) {
                            lo=float3(half_at(base),half_at(base+2),half_at(base+4));
                            hi=float3(half_at(base+6),half_at(base+8),half_at(base+10));
                        } else { lo=f3(body,base); hi=f3(body,base+12); }
                        if (box_hit(lo,hi,localOrigin,localDirection,best)) {
                            if (sp==128) { failed=true; break; }
                            stack[sp++]=next;
                        }
                    }
                    if (failed) break;
                } else if (kind==0) {
                    uint descriptor=body.Load(at+48);
                    float3 a=f3(body,at), b=f3(body,at+12), c=f3(body,at+24);
                    float t=triangle_hit(localOrigin,localDirection,a,b,c);
                    if (t<best) { best=t; instance=idmask&0xffffffu; geometry=descriptor>>8; primitive=body.Load(at+52); }
                    if ((descriptor & 0x80u)!=0) {
                        float3 d=f3(body,at+36);
                        t=triangle_hit(localOrigin,localDirection,b,d,c);
                        if (t<best) { best=t; instance=idmask&0xffffffu; geometry=descriptor>>8; primitive=body.Load(at+56); }
                    }
                } else { failed=true; break; }
            }
            if (failed) break;
        }
    }
    if (failed) hits.Store4(outOffset,uint4(0xfffffffdu,0,0,0));
    else if (instance==0xffffffffu) hits.Store4(outOffset,uint4(0xffffffffu,0xffffffffu,0xffffffffu,0));
    else hits.Store4(outOffset,uint4(instance,geometry,primitive,asuint(best)));
}
