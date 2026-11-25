> English: [test-game.md](../test-game.md)

# Jeu de test : SimpleTriangle

Un jeu de test légal qui tourne déjà à 100 % sur PC, pour comparer le comportement natif et celui d'AnyXboxX. Mesures du 21 novembre 2026.

## Pourquoi celui-ci

- **SimpleTriangle** est un échantillon officiel de Microsoft, publié sous licence MIT dans [microsoft/Xbox-GDK-Samples](https://github.com/microsoft/Xbox-GDK-Samples).
- Il existe en deux versions : `SimpleTriangle` pour Xbox Series X|S et `SimpleTriangleDesktop` pour PC. Le code de jeu est le même ; seule la couche plateforme change.
- La version Series X ne se compile qu'avec le GDKX, réservé aux studios sous accord de confidentialité. La version PC se compile avec le [GDK public](https://github.com/microsoft/GDK) : c'est elle qui sert de référence.
- Il est minuscule (une fenêtre, DirectX 12, deux shaders) mais passe par toutes les couches d'un vrai jeu GDK : démarrage C, `xgameruntime.dll`, fenêtre Win32, DirectX 12.

## Référence native

![SimpleTriangleDesktop exécuté nativement sur PC](../images/simpletriangle-natif.png)

## Dans AnyXboxX, mode natif : le triangle s'affiche

![SimpleTriangleDesktop chargé et lancé par AnyXboxX en mode natif](../images/simpletriangle-anyxboxx.png)

```powershell
python PrototypeScarlett\scarlett.py --natif <...>\x64\Release\SimpleTriangleDesktop.exe
```

AnyXboxX mappe lui-même l'exécutable en mémoire, applique les relocations, déclare sa table d'exceptions (`.pdata`), lie ses imports puis l'exécute directement sur le processeur. C'est l'approche de Wine et de WinDurango.

- **Servi par AnyXboxX** : `xgameruntime.dll`. Le jeu le charge par `LoadLibraryExW`, et AnyXboxX intercepte ce chargement pour fournir son propre runtime GDK (`InitializeApiImplEx2`, `QueryApiImpl`…). Il intercepte aussi `GetModuleHandle`, `GetModuleFileName` et `GetProcAddress`, pour que le jeu se voie lui-même et non `python.exe`.
- **Servi par Windows** : la fenêtre (`USER32`), DirectX 12 (`d3d12`, `dxgi`) et le CRT Visual C++. Pour cette version PC du jeu, ces appels sont déjà ceux du PC. Pour un binaire Series X, il faudra en plus traduire `d3d12_xs.dll` (D3D12.X) et `kernelx.dll`.
- **Sécurité** : en mode natif, le programme n'est pas isolé et a les droits de l'utilisateur. Ce mode est réservé aux programmes de confiance. `--rapport` et `--banc` restent dans Unicorn, isolés.
- **Diagnostic** : `ANYXBOXX_TRACE=1` affiche chaque chargement de DLL, `GetProcAddress` et `QueryApiImpl` intercepté.

## Dans AnyXboxX, mode isolé (Unicorn)

| Étape du jeu | État |
|---|---|
| Chargement : exécutable + `libHttpClient`, `vcruntime140`, `vcruntime140_1`, `msvcp140` liés | ✅ |
| Démarrage C (`_initterm`, `argv`, locale, `_errno`…) puis entrée dans `WinMain` | ✅ |
| Chargement de `xgameruntime.dll` et `InitializeApiImplEx2` (initialisation GDK) | ✅ |
| Création de la fenêtre (`RegisterClassExW`, `CreateWindowExW`, boucle de messages) | ❌ non émulée en mode isolé |
| Création du périphérique DirectX 12 (`CreateDXGIFactory2`, `D3D12CreateDevice`…) | ❌ non émulée en mode isolé |
| Affichage du triangle | ✅ en mode natif, ❌ en mode isolé |

Le mode isolé sert à l'analyse sans risque. Y relier DirectX 12 demanderait de recopier chaque structure entre la mémoire émulée et le PC ; le mode natif évite ce travail.

Fonctions importées : 202, dont **157 résolues (77,7 %)** : 75 émulées en Python et 82 fournies par les DLL Visual C++. Les 45 restantes sont surtout `USER32` (fenêtre, souris, messages), `d3d12`, `dxgi` et les fichiers du CRT.

## Ce que le test a appris

1. **Sur PC, le GDK passe par une interface interne.** Le jeu ne lie pas `XGameRuntimeInitialize` directement : `xgameruntime.lib` charge `xgameruntime.dll`, puis demande `QueryApiImpl`, `InitializeApiImpl`, `InitializeApiImplEx`, `InitializeApiImplEx2`, `XErrorReport` et `UninitializeApiImpl` par `GetProcAddress`. Chaque API `X*` obtient ensuite une interface par `QueryApiImpl`. AnyXboxX accepte l'initialisation et ne fournit pas encore d'interface.
2. **Les DLL Visual C++ ne sont pas dans le dossier du jeu sur PC**, contrairement à un paquet console. Il faut donner leur dossier à AnyXboxX.
3. **Une fonction absente qui renvoie un pointeur fait planter** à l'adresse `0x80004001`. Le traçage a permis d'émuler chaque fonction dans l'ordre où le jeu en a besoin.

## Reproduire

Prérequis : Visual Studio 2022 ou 2026 avec la charge C++, et le GDK public (`winget install Microsoft.Gaming.GDK`).

```powershell
git clone --filter=blob:none --sparse https://github.com/microsoft/Xbox-GDK-Samples.git
cd Xbox-GDK-Samples
git sparse-checkout set --no-cone /Samples/IntroGraphics/SimpleTriangleDesktop/ /Kits/ATGTK/ /Kits/ATGTelemetry/ /Kits/DirectXTK12/
```

Ensuite :

1. Placer le paquet NuGet `WinPixEventRuntime.1.0.240308001` dans `Samples\packages\`.
2. Lancer `Kits\DirectXTK12\Src\Shaders\CompileShaders.cmd dxil` avec son chemin complet.
3. Compiler `SimpleTriangleDesktop.sln` en `Release|x64`, avec `-p:PlatformToolset=v145` sous VS 2026.

Analyse dans AnyXboxX, avec le dossier des DLL Visual C++ en second argument :

```powershell
python PrototypeScarlett\scarlett.py --rapport <...>\x64\Release\SimpleTriangleDesktop.exe "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Redist\MSVC\<version>\x64\Microsoft.VC145.CRT"
```

## Deuxième jeu de test : `testgdk` de SDL3

Mesures du 21 novembre 2026. `testgdk` est le programme de démonstration GDK de [SDL3](https://github.com/libsdl-org/SDL) (licence zlib) : des sprites, du son, la manette et la connexion `XUser`. Il passe par `SDL3.dll`, qui charge `xgameruntime.dll` lui-même par `LoadLibrary`.

![testgdk lancé par AnyXboxX en mode natif](../images/testgdk-natif.png)

| Mode | Résultat |
|---|---|
| Natif (`--natif`) | ✅ fenêtre, sprites et son. `XUserAddAsync` échoue proprement (`E_NOINTERFACE`) : AnyXboxX ne fournit pas encore l'interface `XUser` derrière `QueryApiImpl`. |
| Isolé (`--lancer`) | Démarrage C et SDL passent ; arrêt sur la fenêtre et le texte (`USER32`, `GDI32`), non émulés en mode isolé. |

Deux corrections ont été nécessaires en mode natif :

1. **Ligne de commande** : le jeu lisait celle de Python (`python scarlett.py --natif …`) et la prenait pour ses arguments. AnyXboxX réécrit sur place les tampons de `GetCommandLineW` et `GetCommandLineA`, que relisent le CRT et toutes les DLL, même celles chargées par Windows.
2. **Dossier courant** : le jeu ouvre `icon.png` en chemin relatif. AnyXboxX se place dans le dossier du jeu avant de le lancer.

En mode isolé, `GetCommandLineW/A`, `CommandLineToArgvW` et `GetEnvironmentStringsW` sont maintenant émulées.

### Avec le runtime C++

Depuis le passage du runtime en C++ (`runtime/`), `xgameruntime.dll` est préchargée dans le processus : **toutes** les DLL du jeu passent par AnyXboxX, y compris `SDL3.dll`, que Windows charge lui-même. Les interfaces qu'AnyXboxX n'écrit pas encore sont déléguées au runtime GDK de Windows. Trace (`ANYXBOXX_TRACE=1`) pendant 10 secondes de jeu :

| Interface demandée (`QueryApiImpl`) | Appels | Servie par |
|---|---|---|
| `{073B7DCB-1FCF-4030-94BE-E3C9EB623428}` (file de tâches `XTaskQueue`) | 36 057 | Windows |
| `{01ACD177-91F9-4763-A38E-CCBB55CE32E0}` | 4 | Windows |
| `{973A344E-24BF-4D0F-8457-56C534892B29}` / `{2549F142-6419-4A06-97B5-931AAB7C2F34}` | 1 | Windows |

La file de tâches est demandée des milliers de fois par seconde : ce chemin doit rester en code natif. Elle est maintenant écrite dans AnyXboxX, en C++ : **testgdk tourne sans le runtime de Windows** (`ANYXBOXX_SYSTEM_RUNTIME=0`), et la connexion `XUser` réussit avec le profil local hors ligne d'AnyXboxX (gamertag réglable par `ANYXBOXX_GAMERTAG`). Quand le runtime de Windows est présent, il sert toutes les interfaces, file de tâches comprise : ses API (`XUser`…) ne savent lire que les files qu'elles ont créées.

Compilation : `VisualC-GDK` de SDL3 vise encore `Gaming.Desktop.x64`, absente du GDK 2610. Il faut remplacer cette plateforme par `x64` dans les projets, ajouter `_GAMING_DESKTOP` et les dossiers `include`/`lib` du GDK (`windows\include`, `windows\lib\x64`), puis compiler `tests\testgdk\testgdk.vcxproj` en `Release|x64` avec `-p:PlatformToolset=v145` et `-p:SolutionDir=<…>\VisualC-GDK\`.

## Troisième jeu de test : `Live/Fundamentals_Desktop`

Mesures du 22 novembre 2026. `Fundamentals_Desktop` est l'exemple Xbox Live de [microsoft/Xbox-GDK-Samples](https://github.com/microsoft/Xbox-GDK-Samples) (licence MIT) : connexion de l'utilisateur, gamertag et photo de profil, licence du Store, connectivité réseau et Xbox Live (`Xbl*`). C'est l'exemple PC qui sollicite le plus le runtime GDK.

| Étape | Avant | Après |
|---|---|---|
| Interfaces GDK demandées | 9, dont 8 servies (`XStore` non servie) | identique |
| `XNetworkingRegisterConnectivityHintChanged` (sous `ThrowIfFailed`) | servie depuis #86 | servie |
| Profil local, gamertag | ✅ | ✅ |
| `XUserGetGamerPictureAsync` (sous `ThrowIfFailed`) | `E_NOTIMPL` → exception C++ `0xe06d7363`, arrêt | PNG généré à la taille demandée, l'exemple continue |
| Mode isolé (`--rapport`) | 283 imports, 74,6 % résolus | identique |

Ce que le test a appris :

1. **Un `E_NOTIMPL` peut être fatal.** Les exemples Microsoft enveloppent beaucoup d'appels dans `DX::ThrowIfFailed`. Un jeu réel fera de même : il vaut mieux une réponse hors ligne honnête (image générée, abonnement sans événement) qu'un échec.
2. **`XStore` est la prochaine interface à servir** : l'exemple la demande dès le démarrage (licence du jeu, mises à jour). Servie hors ligne depuis `feat/xstore-hors-ligne` : licence locale valide, requêtes de produits vides, achat et téléchargements en `E_GAMESTORE_NETWORK_ERROR`.
3. Deux méthodes de `XNetworking` non écrites sont appelées, sans doute par `libHttpClient` ou `XCurl` pour la sécurité HTTPS ; l'exemple les tolère.

Reproduire (GDK public installé, Visual Studio 2026) :

```powershell
git clone --filter=blob:none --sparse --depth 1 https://github.com/microsoft/Xbox-GDK-Samples.git
cd Xbox-GDK-Samples
git sparse-checkout set --no-cone /Samples/Live/Fundamentals_Desktop/ /Kits/ATGTK/ /Kits/ATGTelemetry/ /Kits/DirectXTK12/ /Kits/LiveTK/ /Media/Fonts/ /Media/Textures/
Kits\DirectXTK12\Src\Shaders\CompileShaders.cmd dxil          # avec dxc dans le PATH
$env:GameDKCoreLatest = "C:\Program Files (x86)\Microsoft GDK\261000\"
MSBuild Samples\Live\Fundamentals_Desktop\Fundamentals_Desktop.sln /p:Configuration=Release /p:Platform=x64 /p:PlatformToolset=v145
$env:ANYXBOXX_SYSTEM_RUNTIME = "0"; $env:ANYXBOXX_TRACE = "1"
python PrototypeScarlett\scarlett.py --natif <...>\x64\Release\Fundamentals_Desktop.exe "<dossier Microsoft.VC145.CRT>"
```

## Instructions processeur et shaders

- **Processeur** : la Series X utilise un AMD Zen 2 en x86-64, le même jeu d'instructions qu'un PC. Rien n'est à traduire. Seules les extensions AVX/AVX2 manquent dans Unicorn ; l'exécution native les apportera.
- **Shaders** : la version PC utilise des shaders DXIL standard (`VertexShader.cso`, `PixelShader.cso`) que le pilote du PC exécute. Sur console, les shaders seraient livrés compilés pour le GPU RDNA 2 (à vérifier sur un binaire Scarlett légal). Les traduire demanderait un recompilateur RDNA 2 → SPIR-V ou DXIL, comme celui de l'émulateur PS4 libre shadPS4 pour le GPU GCN. AMD publie la documentation du jeu d'instructions RDNA 2.
