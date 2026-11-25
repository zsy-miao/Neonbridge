> English: [REA_RESEARCH.md](../../../research/package/REA_RESEARCH.md)

# Inspection avec REA

REA 6.10.1 a été installé temporairement depuis le paquet npm officiel et exécuté avec Node.js 24.22.1. L’inspection est statique et ne démarre aucun exécutable du jeu. Le dépôt source consulté était au commit `a125d37bfad92fc3b5e472c6c18ee71654037221`.

## Lanceur

La commande `inspect-managed-members` identifie un assemblage .NET nommé `Lancer GTA VI`, de version `0.0.0.0`, avec les métadonnées CLR `v4.0.30319`. Sa classe `Launcher` déclare trois méthodes. REA indique une couverture complète des métadonnées examinées.

| Méthode | Offset du corps dans le fichier | Taille IL | Instructions décodées |
| --- | ---: | ---: | ---: |
| CanLaunch | 592 | 76 | 30 |
| RunGame | 680 | 79 | 39 |
| Main | 788 | 296 | 126 |

SHA-256 du lanceur analysé : `c0bdbbf2076e03f72343d5cc4f73c1abd227dd8abea4aab3e190ad2b8d3ef191`. L’identifiant REA de cette analyse est `ev_3d5448eb94cb8d5630703a2b65f9c58a84e3b26a10ea68ca596647abfed1f8c5`. Les coordonnées et empreintes ne valent que pour ce fichier précis. Les comportements des méthodes sont expliqués dans `ANALYSE_JEU.md`, d’après la lecture de leur code IL.

## Dossier d’installation

La commande `inspect-desktop-application` a examiné 3 941 entrées et sondé 49 fichiers, avec 16 044 octets lus. Elle signale des fichiers candidats .NET, dont le lanceur et le prototype Scarlett. Elle identifie aussi les en-têtes PE de bibliothèques. Les résultats proviennent d’une lecture partielle et successive du dossier et ne prouvent pas que les fichiers ont été exécutés ensemble. REA n’a pas interprété le contenu des RPF ni la logique des missions.

## Reproduction

Une installation de REA 6.10.1 et Node.js 24.22.1 satisfait les versions utilisées ici. Depuis le dossier contenant les fichiers, lancer :

```powershell
rea inspect-desktop-application . --detail complete --json
rea inspect-managed-members ".\Lancer GTA VI.exe" --json
```

La sortie complète contient les coordonnées des méthodes, des instructions et des références. Le parcours du dossier inclut les sous-dossiers présents au moment de l’exécution, donc les chiffres de couverture peuvent évoluer.

## Inspection de `Catalog000.bin`

Une inspection statique supplémentaire avec REA 6.10.1 confirme un fichier PE32+ x64 de 815 104 octets, SHA-256 `4afbdbe22aad2bc338a5ba1ffe83cccb96e072eb4e92f6a5dd90b49b2e227c59`. Les quatre analyses suivantes déclarent une couverture complète des répertoires PE concernés :

| Commande REA | Résultat observé |
| --- | --- |
| `inspect-pe-imports` | Aucun répertoire d'imports ou d'imports différés. |
| `inspect-native-exports` | Aucun répertoire d'exports. |
| `inspect-pe-resources` | Aucun répertoire de ressources PE. |
| `inspect-pe-certificates` | Aucun répertoire de certificats PE. Cela ne vérifie pas une éventuelle signature externe. |

Une lecture complémentaire avec `pefile` trouve un point d'entrée nul et une seule section dont le nom est vide, marquée en lecture (`0x40000000`). Les données brutes de cette section occupent 811 008 octets à partir de `0x1000` ; leur entropie de Shannon empirique est d'environ 7,992 bits par octet. Cette mesure ne permet pas de distinguer compression, chiffrement ou autre encodage. Ces observations ne révèlent pas la fonction du contenu, et l'absence des répertoires usuels ne décrit pas d'éventuelles structures propres au format.

Pour reproduire les analyses REA :

```powershell
rea inspect-pe-imports .\Catalog000.bin --json
rea inspect-native-exports .\Catalog000.bin --json
rea inspect-pe-resources .\Catalog000.bin --json
rea inspect-pe-certificates .\Catalog000.bin --json
```

## Bibliothèques FidelityFX

REA a relevé les tables d'imports et d'exports des deux DLL, sans les charger. Leurs SHA-256 sont respectivement `431bdd2e26c0c8214fd7601140eb9fe6d8a249f5000539453a9a9b6a0b7c0ee8` pour `amd_fidelityfx_loader_xbox.dll` et `faff02f294a7e9149ce079dffe3dedbcabe99b1f180ab14222aa8689e00a75fa` pour `amd_fidelityfx_upscaler_xbox.dll`. Les deux exportent les mêmes cinq noms : `ffxConfigure`, `ffxCreateContext`, `ffxDestroyContext`, `ffxDispatch` et `ffxQuery`. REA rapporte 22 imports répartis dans neuf DLL pour le chargeur et 86 imports dans 24 DLL pour l'upscaler ; ces nombres décrivent les tables, pas les appels réellement exécutés.

Le chargeur importe notamment `LoadLibraryA` et `GetProcAddress`. Ses octets contiennent les noms `amd_fidelityfx_upscaler_xbox.dll` à l'offset fichier `0x19f0` et `amd_fidelityfx_framegeneration_xbox.dll` à `0x1a18`. Le premier fichier est présent à la racine du paquet ; le second n'a pas été trouvé dans le parcours des fichiers du dossier. Ces indices sont compatibles avec une résolution dynamique, mais ne prouvent ni le chemin de chargement ni qu'un module absent soit requis.

Le layout local déclare le chargeur et l'upscaler, mais ne mentionne pas le module de génération d'images. Cette absence du layout ne suffit pas non plus à déterminer s'il est nécessaire dans une autre configuration.

L'upscaler contient la chaîne `FFX_FSR4_XBOX_ACO` à l'offset fichier `0x47088` et le nom `amd_fidelityfx_upscaler_xbox_fsr4_full_720p1080p_input_sanitize_scarlett_opt2.dll` à `0xb9dea`. Ce dernier fichier n'a pas été trouvé dans le dossier. Il existe aussi des chaînes désignant des modèles FSR4. Ces chaînes indiquent du contenu lié à FSR4 dans la bibliothèque ; elles n'établissent pas que le jeu l'active ni qu'il charge ce fichier. Les ressources de version déclarent les versions `1.0.2.0` pour le chargeur et `3.1.5.0` pour l'upscaler ; ce sont des métadonnées déclarées, non une authentification de leur origine.

Reproduction avec REA :

```powershell
rea inspect-pe-imports .\amd_fidelityfx_loader_xbox.dll --json
rea inspect-native-exports .\amd_fidelityfx_loader_xbox.dll --json
rea inspect-pe-imports .\amd_fidelityfx_upscaler_xbox.dll --json
rea inspect-native-exports .\amd_fidelityfx_upscaler_xbox.dll --json
```

La tentative de décompilation native REA a retourné `provider_unavailable` : aucun fournisseur compatible PE n'était configuré, notamment Ghidra. Les relations précises entre chaînes, fonctions et appels restent donc à établir.
