"""Lancement isolé d'un paquet GDK (`--lancer`) et tri des fichiers du banc."""
from emu import bench, launch
from pe_builder import Asm, build_pe

MANIFEST = """<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <TitleId>0000ABCD</TitleId>
  <Identity Name="Studio.Exemple" Publisher="CN=Exemple" Version="1.0.0.0" />
  <ShellVisuals DefaultDisplayName="Jeu exemple" />
  <ExecutableList>
    <Executable Name="game.exe" Id="Game" TargetDeviceFamily="Scarlett" />
  </ExecutableList>
</Game>"""


def package(folder, code=None, imports=()):
    (folder / "MicrosoftGame.Config").write_text(MANIFEST, encoding="utf-8")
    if code is not None:
        build_pe(folder / "game.exe", code, imports=imports, dll=False)
    return folder


def run(target, *extra):
    lines = []
    code = launch.run(target, extra, log=lines.append)
    return code, "\n".join(lines)


def test_launches_scarlett_entry_of_a_package(tmp_path):
    code, out = run(package(tmp_path, lambda iat: Asm().mov_eax(7).ret().build()))
    assert code == 0
    assert "game.exe" in out and "Title ID 0x0000abcd" in out
    assert "retour 0x7" in out


def test_missing_executable_is_explained(tmp_path):
    code, out = run(package(tmp_path))
    assert code == 1
    assert "game.exe manque" in out


def test_unemulated_console_calls_are_listed(tmp_path):
    missing = ("d3d12_xs.dll", "D3D12XboxCreateDevice")
    folder = package(tmp_path, lambda iat: Asm().frame().call_iat(iat[missing]).unframe().ret().build(),
                     imports=[missing])
    code, out = run(folder)
    assert code == 0
    assert "d3d12_xs.dll!D3D12XboxCreateDevice" in out
    assert "non émulé" in out


def test_accepts_an_executable_directly(tmp_path):
    build_pe(tmp_path / "solo.exe", lambda iat: Asm().mov_eax(3).ret().build(), dll=False)
    code, out = run(tmp_path / "solo.exe")
    assert code == 0 and "retour 0x3" in out


def test_markdown_report_groups_missing_imports_by_dll(tmp_path):
    imports = [("d3d12_xs.dll", "D3D12XboxCreateDevice"), ("d3d12_xs.dll", "D3D12XboxSetProcessDebugFlags"),
               ("kernelx.dll", "XMemAllocPhysical")]
    folder = package(tmp_path, lambda iat: Asm().frame().call_iat(iat[imports[0]]).unframe().ret().build(),
                     imports=imports)
    report = tmp_path / "rapport.md"
    lines = []
    assert launch.run(folder, (), log=lines.append, markdown=report) == 0
    text = report.read_text(encoding="utf-8")
    assert text.startswith("# Rapport AnyXboxX : game.exe")
    assert "| d3d12_xs.dll | 2 | D3D12XboxCreateDevice, D3D12XboxSetProcessDebugFlags |" in text
    assert "| kernelx.dll | 1 | XMemAllocPhysical |" in text
    assert "| d3d12_xs.dll!D3D12XboxCreateDevice | 1 |" in text      # appel non émulé et sa fréquence


def test_bench_skips_files_that_are_not_pe32_plus(tmp_path):
    (tmp_path / "launcher.exe").write_bytes(b"MZ" + b"\0" * 200)   # ex. lanceur .NET 32 bits
    build_pe(tmp_path / "helper.dll", lambda iat: Asm().mov_eax(1).ret().build())
    assert [row.name for row in bench.run(tmp_path)] == ["helper.dll"]
