#!/usr/bin/env python3
"""Generate synthetic DXBC request containers and expected candidate outcomes.

Includes ordinary compute, guarded/divergent wave reductions, indexed raw
reads and one malformed input. Expected outcomes target the verified patches.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import zlib

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path)
requests = parser.parse_args().directory
requests.mkdir(parents=True, exist_ok=False)

def pack(words):return struct.pack("<"+"I"*len(words),*words)
def container(code,width=64,resources=()):
 image=bytearray(0x170)
 def q(at,v):struct.pack_into("<Q",image,at,v)
 q(0,6)
 for n in range(12):q(8+8*n,0x80 if n==7 else 0xdeadbeefdeadbeef)
 q(0x80,0x170);q(0x88,len(code)*4);q(0xe0,0x100)
 struct.pack_into("<II",image,0xe8,1,26)
 struct.pack_into("<H",image,0x112,6);image[0x114:0x11a]=b"Sprite";image[0x134:0x13c]=b"MainFake"
 struct.pack_into("<4I",image,0x160,0,width,1,1)
 image.extend(pack(code))
 fssm=b"FSSM"+pack([0x101])+struct.pack("<Q",len(image))+image
 compressed=zlib.compress(fssm)
 section=b"XBCH"+pack([3])+struct.pack("<QQ",len(compressed),len(fssm))+compressed
 xdat=b"XDAT"+pack([2,36,13,60,len(section),7,60+((len(section)+3)//4)*4,4])+section
 xdat+=bytes((-len(xdat))%4)+b"NOBC"
 body=pack([0x50060,(24+len(xdat))//4])+b"DXIL"+pack([0x100,16+len(xdat)-4,4])+xdat
 info=bytearray(48);info[24]=5;struct.pack_into("<3I",info,36,width,1,1)
 psv=pack([48])+info+pack([len(resources)])
 if resources:psv+=pack([16,*[word for row in resources for word in row]])
 chunks=[b"DXIL"+pack([len(body)])+body,b"PSV0"+pack([len(psv)])+psv]
 return b"DXBC"+bytes(16)+pack([1,40+sum(map(len,chunks)),2,40,40+len(chunks[0])])+b"".join(chunks)
fixtures=[("endpgm",container([0xbf810000]),"compiled"),("alu",container([0x7e000280,0xbf810000]),"compiled"),("malformed",b"invalid synthetic request","failed")]
for width in (32,64):
 block=[0xbe803dc1 if width==32 else 0xbe8025c1,0xd5010002,0x00020080,0x4a0404e9,0xde54c102,0x4a0404e9,0x93601202,0x4a0404e9,0x00092402,0x4a0404fa,0xff094002,0xd7780003,0x02010102,0x4a040503]
 if width==64:block.extend([0xd7600006,0x00014102,0x4a040406])
 block.extend([0xd7600007,0x00010102,0xbefe0300 if width==32 else 0xbefe0400])
 for reg, status in ((7, "compiled"), (8, "refused")):
  fixtures.append((f"wave{width}_guard_s{reg}",container(block+[0xbf068000 | reg,0xbf850000 | (len(block)+1),0x4a000081]+block+[0xbf810000],width),status))
raw=[0xbe8b03ff,0x11014fac,0xbe8a03c1,0xbe88030c,0x8809ff0d,0x10000,0xe0382000,0x80020402,0xe038300c,0x80020402,0xbf810000]
fixtures.append(("raw_idxen",container(raw,resources=[(4,0,0,0)]),"compiled"))
bad=raw.copy();bad[5]=0x40000
fixtures.append(("raw_unknown_stride",container(bad,resources=[(4,0,0,0)]),"refused"))
expected={}
for label,blob,status in fixtures:
    path = requests / (hashlib.sha256(blob).hexdigest() + ".dxbc")
    with path.open("xb") as stream:
        stream.write(blob)
    expected[path.name]={"case":label,"status":status}
(requests / "expected.json").write_text(json.dumps(expected,indent=2)+"\n")
