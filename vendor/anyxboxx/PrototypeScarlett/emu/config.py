"""Lecture du manifeste MicrosoftGame.Config d'un paquet GDK."""
from dataclasses import dataclass
from pathlib import Path
import xml.etree.ElementTree as ET

MANIFEST = "MicrosoftGame.Config"
SCARLETT = "scarlett"
MAX_MANIFEST = 1024 * 1024
UTF16_LE_BOM, UTF16_BE_BOM = bytes((0xFF, 0xFE)), bytes((0xFE, 0xFF))


@dataclass(frozen=True)
class Executable:
    name: str
    id: str
    device_family: str
    display_name: str


@dataclass(frozen=True)
class GamePackage:
    folder: Path
    title_id: int
    identity: str
    version: str
    display_name: str
    executables: tuple

    def scarlett_entry(self):
        """Renvoie le chemin de l'exécutable Scarlett, vérifié dans le dossier."""
        for exe in self.executables:
            if exe.device_family.lower() != SCARLETT:
                continue
            path = (self.folder / exe.name).resolve()
            if not path.is_relative_to(self.folder):
                raise ValueError("Le manifeste désigne un fichier hors du dossier.")
            if not path.is_file():
                title = exe.display_name or self.display_name or "Jeu"
                raise ValueError(f"{title} : le fichier {exe.name} manque dans ce dossier.")
            return path
        raise ValueError("Aucun programme Scarlett déclaré dans le manifeste.")


def _decode(data):
    if data.startswith((UTF16_LE_BOM, UTF16_BE_BOM)):
        return data.decode("utf-16", errors="replace")
    if len(data) > 1 and (data[0] == 0 or data[1] == 0):
        return data.decode("utf-16-be" if data[0] == 0 else "utf-16-le", errors="replace")
    return data.decode("utf-8-sig", errors="replace")


def _hex(text):
    try:
        return int(text or "0", 16)
    except ValueError:
        return 0


def manifest_path(folder):
    """Manifest of a package folder, its name matched without case.

    Microsoft Learn names it `MicrosoftGame.config`; Windows ignores the case, Linux (Proton) does not.
    The exact name wins, then the first entry whose name matches once lowered.
    """
    folder = Path(folder)
    exact = folder / MANIFEST
    if exact.is_file():
        return exact
    try:
        entries = sorted(folder.iterdir())
    except OSError:
        return exact
    wanted = MANIFEST.lower()
    return next((entry for entry in entries if entry.name.lower() == wanted and entry.is_file()), exact)


def read_package(folder):
    folder = Path(folder).resolve()
    manifest = manifest_path(folder)
    if not manifest.is_file():
        raise ValueError(f"Le dossier choisi ne contient pas {MANIFEST}.")
    if manifest.stat().st_size > MAX_MANIFEST:
        raise ValueError("Manifeste refusé : fichier trop gros.")
    data = manifest.read_bytes()
    # Un manifeste GDK ne déclare jamais de DTD : on la refuse quel que soit l'encodage.
    text = _decode(data)
    if "<!DOCTYPE" in text or "<!ENTITY" in text:
        raise ValueError("Manifeste refusé : entités XML déclarées.")
    root = ET.fromstring(data)
    identity = root.find("{*}Identity")
    visuals = root.find("{*}ShellVisuals")
    executables = tuple(
        Executable(
            name=e.get("Name", ""),
            id=e.get("Id", ""),
            device_family=e.get("TargetDeviceFamily", ""),
            display_name=e.get("OverrideDisplayName", ""),
        )
        for e in root.findall(".//{*}ExecutableList/{*}Executable")
    )
    return GamePackage(
        folder=folder,
        title_id=_hex(root.findtext("{*}TitleId")),
        identity=identity.get("Name", "") if identity is not None else "",
        version=identity.get("Version", "") if identity is not None else "",
        display_name=visuals.get("DefaultDisplayName", "") if visuals is not None else "",
        executables=executables,
    )
