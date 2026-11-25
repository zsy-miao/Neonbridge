"""Outil du premier lancement (tools/jour_j.py) sur un paquet synthétique : inventaire, fournisseurs
de chaque import, lancements isolé et natif, rapport écrit hors du dépôt."""
import os
import sys
from pathlib import Path

from pe_builder import Asm, build_pe

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import jour_j  # noqa: E402

IMPORTS = [("xmem.dll", "XMemAlloc"), ("kernel32.dll", "GetTickCount"), ("inconnu.dll", "Rien")]


def make_package(folder):
    folder.mkdir(parents=True, exist_ok=True)
    build_pe(folder / "game.exe", lambda iat: Asm().mov_eax(7).ret().build(), imports=IMPORTS, dll=False)
    (folder / "MicrosoftGame.config").write_text(
        '<Game><ExecutableList><Executable Name="game.exe" TargetDeviceFamily="Scarlett" /></ExecutableList></Game>',
        encoding="utf-8")
    return folder


def test_report_names_who_provides_each_import(tmp_path, runtime_dir):
    package = make_package(tmp_path / "paquet")
    text = jour_j.report(package, tmp_path / "sortie")
    assert "# Premier lancement : game.exe" in text and "SHA-256" in text
    assert "| `inconnu.dll` | 1 |" in text and "`inconnu.dll` manquantes : `Rien`" in text
    assert "| `xmem.dll` | 1 | 0 | 1 |" in text                 # servi par AnyXboxX
    assert "| `kernel32.dll` | 1 | 0 | 0 | 0 | 1 | 0 |" in text  # servi par Windows
    assert "## Analyse isolée (--rapport)" in text and "## Lancement isolé dans Unicorn (--lancer)" in text


def test_native_step_is_opt_in_and_reports_the_outcome(tmp_path, runtime_dir):
    package = make_package(tmp_path / "paquet")
    assert "Lancement natif" not in jour_j.report(package, tmp_path / "a")
    text = jour_j.report(package, tmp_path / "b", native=True, duration=30)
    assert "## Lancement natif (anyxboxx.exe)" in text and "inconnu.dll" in text


def test_native_report_inventories_only_psos_from_the_current_run(tmp_path, monkeypatch):
    package = make_package(tmp_path / "paquet")
    output = tmp_path / "sortie"
    launcher = tmp_path / "anyxboxx.exe"
    launcher.touch()
    monkeypatch.setattr(jour_j, "RUNTIME", tmp_path)
    stale_log = tmp_path / "ancien.log"
    stale_log.write_text("1 [d3d12_xs] CreateGraphicsPSO_X counts: pso_ok=99 pso_fail=98 untranslatable=97 | "
                         "shaders ok=0 fail=0\n", encoding="utf-8")
    monkeypatch.setenv("TOMBOX_LOG", str(stale_log))
    native_logs = []
    trace = """\
10 [d3d12_xs] CreateGraphicsPSO_X counts: pso_ok=1 pso_fail=0 untranslatable=0 | shaders ok=1 fail=0
20 [d3d12_xs] CreateGraphicsPSO_X PSO #1 created from translated shaders
25 [d3d12_xs] CreateGraphicsPSO_X real CreateGraphicsPipelineState FAILED hr=0x80070057
30 [d3d12_xs] CreateComputePSO_X compute PSO #1 created from translated shader
35 [d3d12_xs] CreateComputePSO_X real CreateComputePipelineState FAILED hr=0x80070057
40 [d3d12_xs] CreatePipelineState console-numbered stream PSO #1 raw subobjects [0, 24]
45 [d3d12_xs] CreatePipelineState PC driver rejected stream hr=0x80070057 subs=[0, 24]
50 [d3d12_xs] CreateGraphicsPSO_X counts: pso_ok=12 pso_fail=2 untranslatable=3 | shaders ok=13 fail=3
55 [d3d12_xs] pso_async Graphics PSO #1 not ready after 12 ms: handle 0x1 returned
60 [xmem] CreateGraphicsPSO_X PSO #2 created from translated shaders
65 [d3d12_xs] DeserializeGraphicsPSO_X PSO rebuilt from TomBox serialized parts
70 [d3d12_xs] DeserializeComputePSO_X real CreateComputePipelineState FAILED hr=0x80070057
"""

    def run(command, timeout, environment):
        if Path(command[0]) == launcher:
            log = Path(environment["TOMBOX_LOG"])
            assert log.is_absolute() and log.is_relative_to(output.resolve())
            assert environment["ANYXBOXX_TRACE"] == "1"
            assert log != stale_log and log not in native_logs
            if not native_logs:
                log.write_text(trace, encoding="utf-8")
            native_logs.append(log)
        return "code de sortie 0", "0.1 s", ""

    monkeypatch.setattr(jour_j, "_run", run)
    args = [str(package), "--sortie", str(output), "--natif"]
    assert jour_j.main(args) == 0
    text = (output / "rapport.md").read_text(encoding="utf-8")
    assert "## Inventaire des PSO" in text
    assert "| Graphique | 1 | 1 | 0 |" in text
    assert "| Calcul | 1 | 1 | 0 |" in text
    assert "| Flux | 0 | 1 | 1 |" in text
    assert "| Graphique désérialisé | 1 | 0 | 0 |" in text
    assert "| Calcul désérialisé | 0 | 1 | 0 |" in text
    assert "- Graphique — #1" in text and "- Calcul — #1" in text and "- Flux — #1" in text
    assert "Créations : **12**" in text
    assert "rejets du pilote : **2**" in text and "échecs de traduction : **3**" in text
    assert native_logs[0].relative_to(output.resolve()).as_posix() in text

    assert jour_j.main(args) == 0
    assert len(native_logs) == 2
    assert "Créations : **12**" not in (output / "rapport.md").read_text(encoding="utf-8")
    assert os.environ["TOMBOX_LOG"] == str(stale_log)
    assert "pso_ok=99" in stale_log.read_text(encoding="utf-8")


def test_native_step_reports_missing_tombox_log(tmp_path, monkeypatch):
    (tmp_path / "anyxboxx.exe").touch()
    monkeypatch.setattr(jour_j, "RUNTIME", tmp_path)
    monkeypatch.setattr(jour_j, "_run", lambda command, timeout, environment: ("code de sortie 5", "0.1 s", "erreur"))

    text = "\n".join(jour_j.native_step(tmp_path / "paquet", 30, tmp_path / "sortie"))

    assert "## Inventaire des PSO" in text and "Journal TomBox indisponible" in text
    assert "code de sortie 5" in text and "erreur" in text


def test_missing_executable_is_reported(tmp_path, capsys):
    (tmp_path / "MicrosoftGame.config").write_text(
        '<Game><ExecutableList><Executable Name="absent.exe" TargetDeviceFamily="Scarlett" /></ExecutableList></Game>', encoding="utf-8")
    assert jour_j.main([str(tmp_path), "--sortie", str(tmp_path / "out")]) == 1
    assert "absent.exe" in capsys.readouterr().err
