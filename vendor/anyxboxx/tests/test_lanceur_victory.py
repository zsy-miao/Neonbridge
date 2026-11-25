"""VIctory copié dans third_party/victory (base Neptune (VIctory Edit.) 2026-11-25c, plus l'oracle XG) et réglages du lanceur."""
import configparser
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

RACINE = Path(__file__).resolve().parents[1]
VICTORY = RACINE / "third_party" / "victory"
# play.ps1 de Neptune (VIctory Edit.) 2026-11-25c, chemin Series X : la table du lanceur doit être exactement celle-ci
# (sans VICTORY_WAVE_OWN_LDS_GROUPS, que le code de Neptune ne lit pas).
NEPTUNE = {
    "VICTORY_CARD_WINDOWS_MB": "4096", "VICTORY_BAKE_TEXTURES_MS": "10000", "VICTORY_EARLY_TEXTURES_MS": "40",
    "VICTORY_WIDE_MERGE": "1", "VICTORY_RELAY_OWN_QUEUE": "1",
    "VICTORY_UNDER_WRITES_WORDS": "1", "VICTORY_UNDER_WRITES_RUNS": "100000", "VICTORY_UNDER_WRITES_MASKS": "1",
    "VICTORY_RELAY": "2", "VICTORY_WIDE_REFRESH": "1", "VICTORY_ASK_ONCE": "1", "VICTORY_TEXTURE_NO_WAIT": "1",
    "VICTORY_CARD_REHASH_RUNS": "1", "VICTORY_JOIN_COPIES": "1", "VICTORY_D3D12_BACKGROUND": "0",
    "VICTORY_LATE_WAIT_LISTS": "1024", "VICTORY_WINDOW_TAILS": "1", "VICTORY_TEXTURE_STAGE_MB": "512",
    "VICTORY_CARD_BUFFERS_MIN_KB": "64", "VICTORY_ASK_QUIET": "8", "VICTORY_COPY_CHANGED": "1",
    "VICTORY_QUIET_BACK": "1", "VICTORY_LIST_POOL_MOST": "2048", "VICTORY_LIST_POOL_MB": "2048",
    "VICTORY_WIDE_THREADS": "12", "VICTORY_UNDER_WRITES_MASKS_LEAST": "1", "VICTORY_INDIRECT_RAW": "2",
    "VICTORY_TEXTURE_HASH_EVERY": "32", "VICTORY_LEVELS_ON_DEMAND": "1", "VICTORY_WINDOW_KB": "1024",
    "VICTORY_CARD_BUFFERS_MB": "1536", "VICTORY_CARD_WINDOW_UPLOAD": "1", "VICTORY_DROP_DXIL": "1",
    "VICTORY_DESC_DATA": "1", "VICTORY_VRAM_IDLE_MS": "15000,2500,1000", "VICTORY_PIPELINE_WAIT_MS": "2500",
}
# Absentes du code de VIctory (base Neptune) : les poser ne change rien.
ABSENTES = (
    "VICTORY_RT_MISS", "VICTORY_WAVE32_READFIRSTLANE", "VICTORY_WAVE32_CROSSLANE", "VICTORY_SKIP_BVH_BUILDER",
    "VICTORY_CAPPED_DYNAMIC", "VICTORY_DSTORAGE_NOTIFY", "VICTORY_STAGING_POOL_MB",
    "VICTORY_WAVE_OWN_LDS_GROUPS",
)


def _lanceur():
    return (RACINE / "tools" / "lancer_victory.ps1").read_text(encoding="utf-8")


def _variables_posees(texte):
    """Noms VICTORY_* posés par le lanceur : table des réglages et affectations $env:."""
    table = texte[texte.index("$tuned = [ordered]@{"):texte.index("foreach ($name in $tuned.Keys)")]
    noms = set(re.findall(r"^\s*(VICTORY_[A-Z0-9_]+)\s*=", table, re.MULTILINE))
    return noms | set(re.findall(r"\$env:(VICTORY_[A-Z0-9_]+)\s*=", texte))


def test_gitmodules_no_longer_declares_victory():
    declares = configparser.ConfigParser()
    declares.read(RACINE / ".gitmodules", encoding="utf-8")
    assert all(declares[s]["path"] != "third_party/victory" for s in declares.sections())


def test_victory_source_is_tracked_in_the_repository():
    assert (VICTORY / "Cargo.toml").is_file() and (VICTORY / "LICENSE").is_file()
    sortie = subprocess.run(["git", "ls-files", "-s", "third_party/victory"], cwd=RACINE, capture_output=True,
                            text=True, check=False).stdout
    assert not any(ligne.startswith("160000 ") for ligne in sortie.splitlines())   # aucun gitlink


def test_victory_tree_has_no_dependency_outside_itself():
    # Tout est committé ici : aucune dépendance git ni chemin hors de l'arbre (règle 10 d'AGENTS.md).
    for manifeste in [VICTORY / "Cargo.lock", *VICTORY.rglob("Cargo.toml")]:
        if "target" in manifeste.relative_to(VICTORY).parts:
            continue
        texte = manifeste.read_text(encoding="utf-8")
        assert "git+" not in texte and not re.search(r"^\s*git\s*=|[{,]\s*git\s*=", texte, re.MULTILINE), manifeste
        for chemin in re.findall(r'path\s*=\s*"([^"]+)"', texte):
            assert (manifeste.parent / chemin).resolve().is_relative_to(VICTORY.resolve()), (manifeste, chemin)


def _table(texte):
    return texte[texte.index("$tuned = [ordered]@{"):texte.index("foreach ($name in $tuned.Keys)")]


def test_launcher_sets_neptunes_switch_set_and_can_turn_it_off():
    texte = _lanceur()
    table = dict(re.findall(r"^\s*(VICTORY_[A-Z0-9_]+)\s*=\s*'([^']*)'", _table(texte), re.MULTILINE))
    assert table == NEPTUNE
    assert "$NoTunedSettings" in texte and "-not $NoTunedSettings" in texte


def test_launcher_takes_the_series_x_path_and_taps_a_only_when_asked():
    texte = _lanceur()
    poses = _variables_posees(texte)
    assert "VICTORY_FSR_DLL" not in poses and "series-s" not in texte   # pas de DLL FidelityFX PC : chemin Series X
    assert "VICTORY_AUTO_A" not in _table(texte)
    assert "[switch]$AutoA" in texte
    assert re.search(r"if \(\$AutoA\) \{\s*\$env:VICTORY_AUTO_A = '150'", texte)


def test_launcher_keeps_discord_off():
    texte = _lanceur()
    assert "$env:VICTORY_DISCORD = '0'" in texte
    assert texte.index("$env:VICTORY_DISCORD = '0'") < texte.index("if (-not $NoTunedSettings)")
    discord = (VICTORY / "apps" / "victory" / "src" / "discord.rs").read_text(encoding="utf-8")
    assert 'v == "1"' in discord and 'v == "0"' not in discord        # la présence ne s'allume que sur demande


def test_launcher_leaves_out_variables_victory_does_not_read():
    poses = _variables_posees(_lanceur())
    assert not poses & set(ABSENTES)


def test_launcher_turns_dred_on_only_when_asked():
    texte = _lanceur()
    assert "[switch]$Dred" in texte
    assert "VICTORY_D3D12_DRED" not in _table(texte)
    assert re.search(r"if \(\$Dred\) \{\s*\$env:VICTORY_D3D12_DRED = '1'", texte)
    assert "$useDiag = $Diag -or $Dred" in texte                           # DRED n'existe que dans la build diagnostics


def test_every_variable_the_launcher_sets_is_read_by_victory():
    code = "\n".join(f.read_text(encoding="utf-8", errors="replace") for f in VICTORY.rglob("*.rs")
                     if "target" not in f.relative_to(VICTORY).parts)
    for nom in sorted(_variables_posees(_lanceur()) - {"VICTORY_GAME_DIR", "VICTORY_SKIP_PIPELINES"}):
        assert f'"{nom}"' in code, nom


def test_launcher_keeps_its_parameters_and_builds_release_or_diag():
    texte = _lanceur()
    assert "$GameDir" in texte and "$SkipPipelines" in texte and "[switch]$Diag" in texte
    assert "@('--release', '-p', 'victory')" in texte
    assert "@('--profile', 'diag', '-p', 'victory', '--features', 'diagnostics')" in texte


def test_launcher_uses_microsofts_xg_library_only_when_given():
    texte = _lanceur()
    assert "[string]$XgDll" in texte
    assert "[ValidateSet('real', 'compare')][string]$XgMode = 'real'" in texte
    assert "VICTORY_XG" not in _table(texte)
    bloc = texte[texte.index("if ($PSBoundParameters.ContainsKey('XgDll') -and $XgDll) {"):texte.index("& $exe run")]
    assert "Test-Path -LiteralPath $XgDll -PathType Leaf" in bloc and "throw" in bloc
    assert "$env:VICTORY_XG_DLL =" in bloc and "$env:VICTORY_XG_MODE = $XgMode" in bloc


# Faux outil console (.NET Framework, compilé par Windows PowerShell) : copié en cargo.exe et en victory.exe, il note
# « nom arguments » dans FAKE_TOOL_LOG, affiche deux réglages lus par victory et rend le code de
# FAKE_TOOL_EXIT_<NOM> (0 par défaut).
FAUX_OUTIL = r"""
using System; using System.IO;
public static class FakeTool {
    public static int Main(string[] args) {
        string name = Path.GetFileNameWithoutExtension(System.Reflection.Assembly.GetEntryAssembly().Location);
        File.AppendAllText(Environment.GetEnvironmentVariable("FAKE_TOOL_LOG"),
                           name + " " + string.Join(" ", args) + Environment.NewLine);
        if (name == "victory") {
            Console.WriteLine("card rehash runs: " + (Environment.GetEnvironmentVariable("VICTORY_CARD_REHASH_RUNS") ?? "unset"));
            Console.WriteLine("discord: " + (Environment.GetEnvironmentVariable("VICTORY_DISCORD") ?? "unset"));
        }
        string err = Environment.GetEnvironmentVariable("FAKE_TOOL_STDERR_" + name.ToUpperInvariant());
        if (!string.IsNullOrEmpty(err)) {
            Console.Error.WriteLine(err);
        }
        string code = Environment.GetEnvironmentVariable("FAKE_TOOL_EXIT_" + name.ToUpperInvariant());
        return string.IsNullOrEmpty(code) ? 0 : int.Parse(code);
    }
}
"""
CODE_ECHEC_CARGO = 101
CODE_SORTIE_VICTORY = 7
FICHIERS_PRECOMPILATION = ("shaders.rpf", "XboxBvhBuilder.dll", "amd_fidelityfx_upscaler_xbox.dll")


@pytest.fixture(scope="module")
def faux_outil(tmp_path_factory):
    if sys.platform != "win32" or not shutil.which("powershell"):
        pytest.skip("le lanceur est un script Windows PowerShell")
    dossier = tmp_path_factory.mktemp("faux-outil")
    source, exe = dossier / "FakeTool.cs", dossier / "FakeTool.exe"
    source.write_text(FAUX_OUTIL, encoding="utf-8")
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    f"Add-Type -Path '{source}' -OutputAssembly '{exe}' -OutputType ConsoleApplication"],
                   check=True, capture_output=True, timeout=120)
    return exe


def _lancer(tmp_path, faux_outil, code_cargo=0, options=(), code_victory=CODE_SORTIE_VICTORY):
    """Lance une copie du vrai lanceur : faux dépôt, faux jeu, faux cargo sur le PATH, victory.exe déjà compilé."""
    script = tmp_path / "depot" / "tools" / "lancer_victory.ps1"
    script.parent.mkdir(parents=True)
    shutil.copy(RACINE / "tools" / "lancer_victory.ps1", script)
    manifeste = tmp_path / "depot" / "third_party" / "victory" / "Cargo.toml"
    manifeste.parent.mkdir(parents=True)
    manifeste.write_text("", encoding="utf-8")
    jeu = tmp_path / "jeu"
    (jeu / "outils").mkdir(parents=True)
    for nom in ("game_scarlett_master.exe", "common.rpf", *FICHIERS_PRECOMPILATION):
        (jeu / nom).write_bytes(b"")
    (jeu / "outils" / "autre.rpf").write_bytes(b"")              # un sous-dossier : jamais parcouru par -Warm
    cible, outils = tmp_path / "cible", tmp_path / "outils"
    for profil in ("diag", "release"):
        (cible / profil).mkdir(parents=True)
        shutil.copy(faux_outil, cible / profil / "victory.exe")   # compilation déjà présente (peut-être périmée)
    outils.mkdir()
    shutil.copy(faux_outil, outils / "cargo.exe")
    journal = tmp_path / "appels.log"
    env = {**os.environ, "PATH": f"{outils}{os.pathsep}{os.environ['PATH']}", "CARGO_TARGET_DIR": str(cible),
           "FAKE_TOOL_LOG": str(journal), "FAKE_TOOL_EXIT_CARGO": str(code_cargo),
           "FAKE_TOOL_EXIT_VICTORY": str(code_victory)}
    for nom in ("VICTORY_CARD_REHASH_RUNS", "VICTORY_DISCORD"):
        env.pop(nom, None)
    fini = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script),
                           "-GameDir", str(jeu), *options], env=env, capture_output=True, text=True, timeout=120)
    appels = journal.read_text(encoding="utf-8").splitlines() if journal.is_file() else []
    return fini, appels, manifeste, jeu


def test_launcher_rebuilds_release_and_runs_with_discord_off(tmp_path, faux_outil):
    # Après une mise à jour de third_party/victory, l'ancien victory.exe ignorait les nouveaux réglages : cargo décide.
    fini, appels, manifeste, jeu = _lancer(tmp_path, faux_outil)
    assert [a.split()[0] for a in appels] == ["cargo", "victory"], (appels, fini.stderr)
    assert appels[0] == f"cargo build --release -p victory --manifest-path {manifeste}"
    assert appels[1] == f"victory run {jeu / 'game_scarlett_master.exe'} --execute"
    assert fini.returncode == CODE_SORTIE_VICTORY
    sortie = fini.stdout.splitlines()
    assert "card rehash runs: 1" in sortie and "discord: 0" in sortie

    fini, _, _, _ = _lancer(tmp_path / "rehash", faux_outil, options=("-BatchCardRehash", "-NoTunedSettings"))
    assert fini.returncode == CODE_SORTIE_VICTORY, fini.stderr
    assert "card rehash runs: 1" in fini.stdout.splitlines() and "discord: 0" in fini.stdout.splitlines()

    fini, _, _, _ = _lancer(tmp_path / "nu", faux_outil, options=("-NoTunedSettings",))
    assert "card rehash runs: unset" in fini.stdout.splitlines()


def test_launcher_builds_the_diag_profile_for_dred(tmp_path, faux_outil):
    fini, appels, manifeste, _ = _lancer(tmp_path, faux_outil, options=("-Dred",))
    assert appels[0] == f"cargo build --profile diag -p victory --features diagnostics --manifest-path {manifeste}"
    assert fini.returncode == CODE_SORTIE_VICTORY, fini.stderr


def test_warm_reads_only_the_game_folders_rpf_files_before_the_game(tmp_path, faux_outil):
    fini, appels, _, jeu = _lancer(tmp_path, faux_outil, options=("-Warm",), code_victory=0)
    assert fini.returncode == 0, fini.stderr
    assert [a.split()[:2] for a in appels] == [["cargo", "build"], ["victory", "warm"], ["victory", "run"]]
    assert appels[1] == f"victory warm {jeu / 'common.rpf'} {jeu / 'shaders.rpf'}"   # ni le dossier, ni ses sous-dossiers


def test_warm_failure_stops_before_the_game(tmp_path, faux_outil):
    fini, appels, _, _ = _lancer(tmp_path, faux_outil, options=("-Warm",))
    assert [a.split()[:2] for a in appels] == [["cargo", "build"], ["victory", "warm"]]
    assert fini.returncode != 0 and "victory warm failed" in fini.stderr


def test_launcher_stops_when_the_build_fails(tmp_path, faux_outil):
    fini, appels, _, _ = _lancer(tmp_path, faux_outil, code_cargo=CODE_ECHEC_CARGO)
    assert [a.split()[0] for a in appels] == ["cargo"], appels         # l'ancien victory.exe n'est pas lancé
    assert fini.returncode != 0
    assert f"cargo build failed (exit code {CODE_ECHEC_CARGO})" in fini.stderr


def test_launcher_survives_stderr_redirection_in_windows_powershell_51(tmp_path, faux_outil):
    """Issue #233: sous Windows PowerShell 5.1 avec 2> victory.log, les warnings stderr ne doivent pas avorter."""
    def run_redirected(sub, options, code_victory):
        base = tmp_path / sub
        script = base / "depot" / "tools" / "lancer_victory.ps1"
        script.parent.mkdir(parents=True)
        shutil.copy(RACINE / "tools" / "lancer_victory.ps1", script)
        manifeste = base / "depot" / "third_party" / "victory" / "Cargo.toml"
        manifeste.parent.mkdir(parents=True)
        manifeste.write_text("", encoding="utf-8")
        jeu = base / "jeu"
        jeu.mkdir()
        for nom in ("game_scarlett_master.exe", "common.rpf", *FICHIERS_PRECOMPILATION):
            (jeu / nom).write_bytes(b"")
        cible, outils = base / "cible", base / "outils"
        for profil in ("diag", "release"):
            (cible / profil).mkdir(parents=True)
            shutil.copy(faux_outil, cible / profil / "victory.exe")
        outils.mkdir()
        shutil.copy(faux_outil, outils / "cargo.exe")
        journal, errlog = base / "appels.log", base / "victory.log"
        env = {
            **os.environ,
            "PATH": f"{outils}{os.pathsep}{os.environ['PATH']}",
            "CARGO_TARGET_DIR": str(cible),
            "FAKE_TOOL_LOG": str(journal),
            "FAKE_TOOL_EXIT_CARGO": "0",
            "FAKE_TOOL_EXIT_VICTORY": str(code_victory),
            "FAKE_TOOL_STDERR_CARGO": "warning: use of deprecated method fetch_update",
            "FAKE_TOOL_STDERR_VICTORY": "victory diagnostic stderr line",
        }
        opts = (" " + " ".join(options)) if options else ""
        cmd = f"& '{script}' -GameDir '{jeu}'{opts} 2> '{errlog}'; exit $LASTEXITCODE"
        fini = subprocess.run(
            ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", cmd],
            env=env, capture_output=True, text=True, timeout=120,
        )
        appels = journal.read_text(encoding="utf-8").splitlines() if journal.is_file() else []
        raw_err = errlog.read_bytes() if errlog.is_file() else b""
        contenu_err = (
            raw_err.decode("utf-16")
            if raw_err.startswith((b"\xff\xfe", b"\xfe\xff"))
            else raw_err.decode("utf-8", errors="replace")
        )
        return fini, appels, contenu_err

    fini, appels, err = run_redirected("run", (), CODE_SORTIE_VICTORY)
    assert [a.split()[:2] for a in appels] == [["cargo", "build"], ["victory", "run"]], (appels, fini.stderr, err)
    assert fini.returncode == CODE_SORTIE_VICTORY, fini.stderr
    assert "warning: use of deprecated method fetch_update" in err
    assert "victory diagnostic stderr line" in err

    fini, appels, err = run_redirected("warm", ("-Warm", "-Precompile"), 0)
    assert [a.split()[:2] for a in appels] == [
        ["cargo", "build"], ["victory", "warm"], ["victory", "warm"], ["victory", "run"]
    ], (appels, fini.stderr, err)
    assert fini.returncode == 0, fini.stderr
    assert "warning: use of deprecated method fetch_update" in err
    assert "victory diagnostic stderr line" in err


def test_no_xg_library_in_the_repository():
    # Règle 1 d'AGENTS.md : la xg_xs.dll du GDK vient de la copie de l'utilisateur, jamais du dépôt.
    sortie = subprocess.run(["git", "ls-files", "*.dll"], cwd=RACINE, capture_output=True, text=True,
                            check=False).stdout
    assert "xg_xs" not in sortie.lower()


def test_precompile_warms_shaders_the_game_and_its_dlls_before_the_game(tmp_path, faux_outil):
    texte = _lanceur()
    assert "[switch]$Precompile" in texte and "Precompile = $true" not in texte
    # Les variables du jeu (clé du cache) sont posées avant la précompilation, pas après.
    assert texte.index("$env:VICTORY_GAME_DIR =") < texte.index("if ($Precompile) {") < texte.index("& $exe run")
    fini, appels, _, jeu = _lancer(tmp_path, faux_outil, options=("-Precompile",), code_victory=0)
    assert fini.returncode == 0, fini.stderr
    attendu = " ".join(str(jeu / n) for n in ("shaders.rpf", "game_scarlett_master.exe", *FICHIERS_PRECOMPILATION[1:]))
    assert appels[1] == f"victory warm {attendu}"
    assert appels[2].startswith("victory run ")
