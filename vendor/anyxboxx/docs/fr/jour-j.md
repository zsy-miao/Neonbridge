> English: [day-one.md](../day-one.md)

# Premier lancement d'un exécutable console

Marche à suivre quand l'exécutable du paquet (par exemple `game_scarlett_master.exe`) est disponible. Tout se lance depuis la racine d'AnyXboxX.

## 1. Préparer (une fois)

```powershell
git submodule update --init
cmake -S runtime -B runtime/build -A x64
cmake --build runtime/build --config Release
cargo build --release --manifest-path rust/Cargo.toml
python tools/build_tombox.py d3d12_xs xg_xs xmem
```

## 2. Diagnostic sans risque

```powershell
python tools/jour_j.py ..
```

L'outil :

- fait l'inventaire de l'exécutable : empreinte SHA-256, en-têtes, sections, TLS et imports ;
- indique pour chaque fonction importée qui la fournit : le paquet, AnyXboxX, TomBox, Windows ou personne ;
- lance `--rapport` et `--lancer`, qui exécutent le programme dans Unicorn, isolé du PC.

Le rapport est écrit dans `%LOCALAPPDATA%\AnyXboxX\jour-j\<date>\rapport.md`, hors du dépôt : il nomme des fonctions du jeu et ne doit pas être versionné.

## 3. Lancement réel (facultatif)

```powershell
python tools/jour_j.py .. --natif --duree 120
```

`anyxboxx.exe` exécute alors le code du jeu directement sur le processeur, avec les DLL console de TomBox (`ANYXBOXX_TOMBOX_DIR`) et la trace des appels (`ANYXBOXX_TRACE=1`).

Le rapport natif présente les objets d'état de pipeline (PSO) à partir des créations et rejets échantillonnés par TomBox, ainsi que le dernier relevé des compteurs graphiques quand il est disponible. Chaque lancement de diagnostic fixe `TOMBOX_LOG` vers un nouveau fichier sous le dossier du rapport. Le rapport indique ce journal local lorsque TomBox l'écrit.

**Ce mode n'isole pas le jeu.** Il est réservé à un exécutable dont la provenance est connue.

### Ce que l'hôte Windows doit fournir

Le mode natif lie certains imports à Windows lui-même, pas à AnyXboxX. Quand l'un d'eux manque, `anyxboxx.exe` s'arrête avant le point d'entrée et le nomme (`dll!fonction` ou `dll!#ordinal`).

- **GameInput** (`GameInput.dll!#1`). Le titre importe `GameInputCreate` par l'ordinal 1, comme le lie la bibliothèque GameInput du GDK. AnyXboxX ne le sert pas, volontairement : le runtime GameInput de l'hôte donne les vraies manettes, et tout runtime GameInput accepte l'interface v0 que crée ce `GameInputCreate` ([GameInputCreate](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputcreate), [versions de l'API](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-version-interop)). Les versions récentes de Windows l'incluent (`System32\GameInput.dll`) ; s'il manque, installer le redistribuable avec `winget install Microsoft.GameInput` ([GameInput NuGet](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-nuget)).
- **API sets et bibliothèques Windows sans équivalent console dans AnyXboxX** : `api-ms-win-core-psm-appnotify-*` (`RegisterAppStateChangeNotification` et `RegisterAppConstrainedChangeNotification`, appelées juste après le démarrage du GDK), `ext-ms-win-rtcore-ntuser-*`, Media Foundation (`MFPlat.dll`) et Winsock (`WS2_32.dll`, importé par ordinal). L'exécutable 1.0.2.1 n'a été mesuré que sous Windows 11, où tous se résolvent. Windows décide, appareil par appareil, quels contrats d'API set existent, quel que soit leur préfixe `api-` ou `ext-` ([Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)), et les éditions N de Windows sont livrées sans Media Foundation tant que le [Media Feature Pack](https://support.microsoft.com/windows/media-feature-pack-for-windows-n-8622b390-4ce6-43c9-9b42-549e5328e407) n'est pas installé. Utiliser un Windows 11 récent qui ne soit pas une édition N, ou installer ce pack.

## 4. Après le diagnostic

- Les fonctions **manquantes** du rapport deviennent des issues, une par DLL.
- Les **shaders** du jeu passent par `--rdna2 … --liste` puis `--hlsl` ; le cache (`emu/shader_cache.py`) évite de les recompiler.
- Rien du jeu n'entre dans le dépôt : ni l'exécutable, ni ses shaders, ni ses données. Seuls des noms de fonctions et des constats peuvent être publiés.
