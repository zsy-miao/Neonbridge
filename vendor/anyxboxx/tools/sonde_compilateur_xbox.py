"""Compile an original synthetic shader with locally supplied Xbox graphics DLLs.

No SDK or game files are distributed. Output is temporary and never sent anywhere.
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SHADER = '''[RootSignature(" ")]
float4 main(float4 position : SV_Position) : SV_Target {
    return float4(0.25, 0.5, 0.75, 1.0);
}
'''
REQUIRED = ("dxcompiler_xs.dll", "xbsc_xs.dll", "newbe_xs.dll")
OPTIONAL = ("xg_xs.dll", "xgs12_pc_xs.dll", "d3d12_xs.dll")


def run(dxc: Path, dll_dir: Path, victory: Path | None) -> int:
    required = [dxc, dxc.with_name("dxil.dll"), *(dll_dir / f for f in REQUIRED)]
    if victory is not None:
        required.append(victory)
    missing = [str(p) for p in required if not p.is_file()]
    if missing:
        print("Fichiers manquants : " + ", ".join(missing), file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory(prefix="anyxboxx-shader-probe-") as temp:
        work = Path(temp)
        shutil.copy2(dxc, work / "dxc.exe")
        shutil.copy2(dxc.with_name("dxil.dll"), work / "dxil.dll")
        shutil.copy2(dll_dir / "dxcompiler_xs.dll", work / "dxcompiler.dll")
        for name in (*REQUIRED[1:], *OPTIONAL):
            src = dll_dir / name
            if src.is_file():
                shutil.copy2(src, work / name)
        shader = work / "probe.hlsl"
        result = work / "probe.dxil"
        shader.write_text(SHADER, encoding="ascii")
        cmd = [str(work / "dxc.exe"), "-T", "ps_6_0", "-E", "main", "-Fo", str(result), str(shader)]
        compiled = subprocess.run(cmd, cwd=work, capture_output=True, text=True, errors="replace")
        if compiled.returncode or not result.is_file():
            print("Echec de compilation :", file=sys.stderr)
            print(compiled.stderr or compiled.stdout, file=sys.stderr)
            return 1
        print(f"Compilation Xbox reussie : {result.stat().st_size} octets.")
        if victory:
            check = subprocess.run([str(victory), "shader-info", str(result), "--verbose"],
                                   cwd=work, capture_output=True, text=True, errors="replace")
            print(check.stdout, end="")
            if check.returncode:
                print(check.stderr, file=sys.stderr)
                return 1
            if "native code" not in check.stdout or "native Ps" not in check.stdout:
                print("VIctory n'a pas reconnu de code natif pixel.", file=sys.stderr)
                return 1
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dxc", required=True, type=Path, help="dxc.exe du SDK Windows, avec dxil.dll voisin")
    parser.add_argument("--dll-dir", required=True, type=Path, help="dossier local des DLL du kit")
    parser.add_argument("--victory", type=Path, help="victory.exe pour verifier le shader produit")
    args = parser.parse_args()
    return run(args.dxc.resolve(), args.dll_dir.resolve(), args.victory.resolve() if args.victory else None)


if __name__ == "__main__":
    raise SystemExit(main())
