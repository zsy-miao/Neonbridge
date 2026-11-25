"""Synthetic native compute requests for the live-worker benchmark."""
import argparse
import hashlib
from pathlib import Path
import struct
import zlib

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

def uav_shader(code):
    """Package synthetic ISA and optional trailing table data with a root table for u0."""
    shader = container(code, resources=[(7, 0, 0, 0)])
    # Extend the fixture's two-chunk directory with a root table for UAV u0.
    root = b"RTS0" + pack([64, 1, 1, 24, 0, 0, 0, 0, 0, 36, 1, 44, 1, 1, 0, 0, 0])
    first, second = struct.unpack_from("<II", shader, 32)
    root_at = len(shader) + 4
    directory = pack([root_at + len(root), 3, first + 4, second + 4, root_at])
    return shader[:24] + directory + shader[first:] + root



def generate(directory):
    directory.mkdir(parents=True, exist_ok=False)
    # Distinct native programs, with 1..16 ALU instructions. This is a
    # small compute-only workload, not representative of retail shaders.
    for index in range(64):
        code = [0x7e0002ff, 0x12340000 + index]  # v_mov_b32 v0, literal
        for n in range(1 + index % 16):
            code.extend([0x7e0202ff, 0x65430000 + index * 16 + n])
        data = container(code + [0xbf810000], width=64)
        (directory / (hashlib.sha256(data).hexdigest() + ".dxbc")).write_bytes(data)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    generate(parser.parse_args().directory)
