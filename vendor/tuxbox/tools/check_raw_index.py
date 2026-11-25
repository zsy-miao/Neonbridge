#!/usr/bin/env python3
"""Read back indexed raw SRV loads from the synthetic native translator fixture.

Generate the input HLSL with:
  cargo test --manifest-path tools/shader-compiler/Cargo.toml --target x86_64-unknown-linux-gnu proven_byte_index_emits_addresses_without_widening_loads -- --nocapture
Save the text between BEGIN_HLSL and END_HLSL as the --hlsl file.
Only test inputs and readback outputs are instrumented. No game data is needed.
"""
from pathlib import Path
import argparse
import sys, subprocess, struct, json, random, hashlib
from tempfile import TemporaryDirectory

p = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(p))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest

parser = argparse.ArgumentParser(description=__doc__)
for name in ("hlsl", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
    parser.add_argument("--" + name, type=Path, required=True)
parser.add_argument("--adapter", required=True)
args = parser.parse_args()
w = args.work.resolve()
w.mkdir(parents=True, exist_ok=True)
dxc = Path(find_dxc(None)).resolve()
src=args.hlsl.read_text()
def replace(s,a,b):
 assert s.count(a)==1,a
 return s.replace(a,b)
src=replace(src,"ByteAddressBuffer raw_sbuf0 : register(t0);","ByteAddressBuffer raw_sbuf0 : register(t0);\nRWByteAddressBuffer output : register(u0);")
src=replace(src,"    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;","""    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;
    exec_lo = uint(gtid.x < 16u);
    if (exec_lo & 1u) {
        v2 = raw_sbuf0.Load(4u * gtid.x);
        v3 = raw_sbuf0.Load(4u * (16u + gtid.x));
    }""")
for marker,offset in [("0x0020 buffer_load_dwordx4",0),("0x0028 s_endpgm",4)]:
 stores="".join(f"    if (exec_lo & 1u) output.Store(4u * (64u + 8u * gtid.x + {offset+k}u), v{4+k});\n" for k in range(4))
 src=replace(src,"    // "+marker,stores+"    // "+marker)
guest=(p/"tools/reference/independent-shaders/compute.cpp").read_text()
guest=replace(guest,"D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};","D3D12_DESCRIPTOR_RANGE range[]{{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0}, {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 0, 0, 1}};")
guest=replace(guest,"table.DescriptorTable = {1, &range};","table.DescriptorTable = {2, range};")
guest=replace(guest,"D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1,","D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 2,")
guest=replace(guest,"    D3D12_COMMAND_QUEUE_DESC qd{};","""    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.Format = DXGI_FORMAT_R32_TYPELESS;
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Buffer = {first_byte / 4, view_bytes / 4, 0, D3D12_BUFFER_SRV_FLAG_RAW};
    auto srv_handle = heap->GetCPUDescriptorHandleForHeapStart();
    srv_handle.ptr += device->GetDescriptorHandleIncrementSize(hd.Type);
    device->CreateShaderResourceView(seed.Get(), &srv, srv_handle);
    D3D12_COMMAND_QUEUE_DESC qd{};""")
rng=random.Random(0x53434152)
def digest(data):return hashlib.sha256(data).hexdigest()
proton=args.proton.resolve()
with TemporaryDirectory(dir=w) as td:
 trial=Path(td);(trial/"control.hlsl").write_text(src);(trial/"guest.cpp").write_text(guest)
 (trial/"context.h").write_text((p/"tools/reference/independent-shaders/context.h").read_text().replace('"../compute_buffers.h"','"'+str(p/"tools/reference/compute_buffers.h")+'"'))
 blob=trial/"control.cso"
 subprocess.run([str(dxc),"-T","cs_6_0","-E","main","-Fo",str(blob),str(trial/"control.hlsl")],check=True,timeout=30)
 subprocess.run([str(dxc.with_name("dxv")),str(blob)],check=True,timeout=30)
 expected={}
 for case in range(8):
  name=f"case{case:02}"
  seed=[rng.getrandbits(32) for _ in range(384)]
  addresses=[]
  for lane in range(16):
   base=rng.randrange(32,55)
   extra=rng.randrange(-3,4)
   seed[96+lane]=base*4
   seed[112+lane]=(extra*4)&0xffffffff
   addresses.append((base,base+extra+3))
  want=seed.copy()
  for lane,(first,second) in enumerate(addresses):
   want[160+8*lane:168+8*lane]=seed[96+first:100+first]+seed[96+second:100+second]
  (trial/f"{name}.seed").write_bytes(struct.pack("<384I",*seed))
  (trial/f"{name}.cso").hardlink_to(blob)
  expected[name]=struct.pack("<384I",*want)
 (trial/"cases.txt").write_text("".join(f"{n} 0 raw 1\n" for n in expected))
 exe,status,fields=run_gpu_guest(trial/"guest.cpp","raw-index.exe",trial,w,proton,args.prefix.resolve(),args.provider.resolve(),args.compiler_dir.resolve())
 print("guest",status,fields)
 assert status==0 and fields.get("adapter")==args.adapter
 rows=[]
 for name,want in expected.items():
  actual=(trial/f"{name}.bin").read_bytes()
  assert actual==want,(name,[(i,a,b) for i,(a,b) in enumerate(zip(struct.unpack("<384I",actual),struct.unpack("<384I",want))) if a!=b][:10])
  rows.append({"case":name,"matched":True,"sha256":digest(actual)})
 receipt={"scope":"Synthetic translated IDXEN and IDXEN+OFFEN raw SRV loads; instrumented input and output only","adapter":fields["adapter"],"proton":(proton/"version").read_text().strip(),"cases":8,"compared_words":3072,"result_words":1024,"preserved_words":2048,"mismatches":0,"emitted_hlsl_sha256":digest(args.hlsl.read_bytes()),"instrumented_hlsl_sha256":digest(src.encode()),"dxil_sha256":digest(blob.read_bytes()),"guest_sha256":digest(exe.read_bytes()),"rows":rows}
 args.output.parent.mkdir(parents=True,exist_ok=True)
 args.output.write_text(json.dumps(receipt,indent=2)+"\n")
 print("PASS: 8 cases, 3072 words, 0 mismatches")
