import pytest

from emu.config import manifest_path, read_package

MANIFEST = """<?xml version="1.0" encoding="utf-8"?>
<Game configVersion="1">
  <TitleId>0000ABCD</TitleId>
  <Identity Name="Studio.Exemple" Publisher="CN=Exemple" Version="1.2.3.4" />
  <ShellVisuals DefaultDisplayName="Jeu exemple" />
  <ExecutableList>
    <Executable Name="one.exe" Id="Game" TargetDeviceFamily="XboxOne" />
    <Executable Name="{name}" Id="Game" TargetDeviceFamily="Scarlett" />
  </ExecutableList>
</Game>"""


def make(tmp_path, name="game.exe", create=True):
    (tmp_path / "MicrosoftGame.Config").write_text(MANIFEST.format(name=name), encoding="utf-8")
    if create:
        (tmp_path / "game.exe").write_bytes(b"MZ")
    return read_package(tmp_path)


def test_reads_identity_and_title(tmp_path):
    package = make(tmp_path)
    assert package.title_id == 0xABCD
    assert package.identity == "Studio.Exemple"
    assert package.version == "1.2.3.4"
    assert package.display_name == "Jeu exemple"
    assert len(package.executables) == 2


def test_picks_scarlett_executable(tmp_path):
    assert make(tmp_path).scarlett_entry().name == "game.exe"


def test_missing_executable_is_reported(tmp_path):
    with pytest.raises(ValueError, match="manque"):
        make(tmp_path, create=False).scarlett_entry()


def test_rejects_path_outside_folder(tmp_path):
    with pytest.raises(ValueError, match="hors du dossier"):
        make(tmp_path, name="../../evil.exe").scarlett_entry()


def test_rejects_missing_manifest(tmp_path):
    with pytest.raises(ValueError, match="MicrosoftGame.Config"):
        read_package(tmp_path)


def test_rejects_entity_declarations(tmp_path):
    (tmp_path / "MicrosoftGame.Config").write_text(
        '<!DOCTYPE g [<!ENTITY a "x">]><Game/>', encoding="utf-8")
    with pytest.raises(ValueError, match="entités"):
        read_package(tmp_path)


@pytest.mark.parametrize("name", ["MicrosoftGame.config", "microsoftgame.CONFIG"])
def test_manifest_name_is_matched_without_case(tmp_path, name):
    """Microsoft Learn names it MicrosoftGame.config: found on a case-sensitive file system too."""
    (tmp_path / name).write_text(MANIFEST.format(name="game.exe"), encoding="utf-8")
    (tmp_path / "game.exe").write_bytes(b"MZ")
    assert read_package(tmp_path).scarlett_entry().name == "game.exe"


def test_exact_manifest_name_wins(tmp_path):
    (tmp_path / "MicrosoftGame.Config").write_text(MANIFEST.format(name="game.exe"), encoding="utf-8")
    (tmp_path / "microsoftgame.config").write_text("<Game/>", encoding="utf-8")
    assert manifest_path(tmp_path).name == "MicrosoftGame.Config"


def test_folder_holding_a_manifest_named_folder_is_rejected(tmp_path):
    (tmp_path / "microsoftgame.config").mkdir()
    with pytest.raises(ValueError, match="MicrosoftGame.Config"):
        read_package(tmp_path)
