"""Synthetic gfx1030 vertex producer for portable pixel interface tests."""
import struct
import zlib

def pack(words):
    return struct.pack("<"+"I"*len(words),*words)

def signature(rows):
    data=bytearray(pack([len(rows),8]))
    names=bytearray()
    for name,index,sysval,kind,reg,mask in rows:
        data.extend(pack([8+24*len(rows)+len(names),index,sysval,kind,reg,mask|mask<<8]))
        names.extend(name.encode("ascii")+b"\0")
    return bytes(data+names)

def vertex(kind=3):
    # LLVM gfx1030: fullscreen triangle from v0 (VertexID); constant raw
    # parameter bits in v8:v11, POS0 and PARAM0 exports.
    code=[0x7e0202f3,0x7e0402ff,0x40400000,0x7d840082,0x02080501,
          0x7d840081,0x020a0501,0x7e0c02f0,0x7e0e02f2,
          0x7e1002ff,0x89abcdef,0x7e1202ff,0x3e800000,0x7e1402f0,
          0x7e1602f2,0xf80000cf,0x07060504,0xf8000a0f,0x0b0a0908,0xbf810000]
    image=bytearray(0x170)
    def q(at,v):struct.pack_into("<Q",image,at,v)
    q(0,6)
    for n in range(12):q(8+8*n,0x80 if n==7 else 0xdeadbeefdeadbeef)
    q(0x80,0x170);q(0x88,len(code)*4);q(0xe0,0x100)
    struct.pack_into("<II",image,0xe8,1,26)
    struct.pack_into("<H",image,0x112,6)
    image[0x114:0x11a]=b"Sprite";image[0x134:0x13c]=b"MainFake"
    image.extend(pack(code))
    fssm=b"FSSM"+pack([0x101])+struct.pack("<Q",len(image))+image
    compressed=zlib.compress(fssm)
    section=b"XBCH"+pack([3])+struct.pack("<QQ",len(compressed),len(fssm))+compressed
    xdat=b"XDAT"+pack([2,36,13,60,len(section),7,60+((len(section)+3)//4)*4,4])+section
    xdat+=bytes((-len(xdat))%4)+b"NOBC"
    body=pack([0x10060,(24+len(xdat))//4])+b"DXIL"+pack([0x100,16+len(xdat)-4,4])+xdat
    chunks=[(b"DXIL",body),(b"ISGN",signature([("SV_VertexID",0,6,1,0,1)])),
            (b"OSGN",signature([("SV_Position",0,1,3,0,15),("TEXCOORD",0,0,kind,1,15)]))]
    header=32+4*len(chunks)
    offsets=[];data=bytearray()
    for tag,part in chunks:
        offsets.append(header+len(data));data.extend(tag+pack([len(part)])+part)
        data.extend(bytes((-len(data))%4))
    return b"DXBC"+bytes(16)+pack([1,header+len(data),len(chunks),*offsets])+data
