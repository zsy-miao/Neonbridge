> English: [first-boot-1021.md](../../research/first-boot-1021.md)

# Premier démarrage de l'exécutable 1.0.2.1 : ce que nous avons mesuré

Suite le même soir, jusqu'à la boucle de frames, dans [demarrage-leonida-2026-11-23.md](demarrage-leonida-2026-11-23.md).

État au 23 novembre 2026. Cette note range les recherches du jour sur `game_scarlett_master.exe`
(build 1.0.2.1) et sur les premiers lancements de cet exécutable sous `gta6rt` de Leonida. Elle remplace
l'état « exécutable absent » de [pistes-premier-executable.md](pistes-premier-executable.md) (22 novembre).

**Les règles 1 et 2 d'[AGENTS.md](../../../AGENTS.md) s'appliquent.** La note ne contient que des noms de fonctions,
des slots de vtable, des HRESULT, des IID, des valeurs de flags, des comptes et des formes d'arguments, relevés sur
une copie possédée légalement (constats d'interopérabilité). Aucun code désassemblé, aucune donnée, aucune adresse
ni RVA, aucun fichier du jeu n'est reproduit.

**Mesuré et supposé.** Chaque ligne dit lequel des deux elle est. *Mesuré* : observé sur un lancement, ou lu dans
le fichier par une méthode reproductible. *Supposé* : déduit, ou vrai sur une seule configuration. Les rapports
locaux derrière cette note (analyse statique de Claude avec REA et capstone, rapports de Codex, journaux de
lancement) restent hors du dépôt.

## 1. Triage de l'exécutable

| Élément | Valeur | Statut |
| --- | --- | --- |
| SHA-256 | `e95020b2fa4a721db3df59fdecf75621730ac559d8835252f6c3b658d5a08993` | mesuré |
| Taille, format | 120 181 760 octets, PE32+ AMD64, sous-système 2 (GUI), image de 184 995 840 octets, relocalisations présentes | mesuré |
| Point d'entrée | amorce MSVC, puis `WinMain` | mesuré |
| Fonctions | 299 257 entrées `.pdata` | mesuré |
| Imports | 587 imports nommés dans 68 DLL, 0 chargé en différé, aucun import par ordinal sauf `GameInput.dll!#1` et WS2_32 | mesuré |
| Imports côté console | 7 DLL : `xmem` (12), `d3d12_xs` (3), `xg_xs` (2), `XCRDAPI` (1), `chad` (5), `xapu` (5), `AcpHal` (9) = 37 | mesuré |
| Exports | un seul : `__XMemAllocationHooks__` (un export de donnée) | mesuré |
| Packer | aucun vu : le point d'entrée se désassemble comme l'amorce MSVC standard, les tables d'initialiseurs du CRT sont lisibles, entropie de `.text` 6,754, et les lancements ci-dessous exécutent du code du jeu jusqu'à un `int3` dans l'image | mesuré ; l'entropie seule ne prouve pas l'absence de zone compressée |
| Chaîne d'outils | deux sections `.xbld` nomment deux éditions du GDK (avril 2025 Update 2 et octobre 2025) ; chemin de débogage d'un build master Scarlett | mesuré |
| TLS | deux callbacks : le premier (n'agit que pour `reason == 2`) et le second (raisons 3 et 0) | mesuré |
| Control Flow Guard | `GuardFlags = 0` ; les trois pointeurs CFG sont initialisés dans le fichier vers des stubs de l'image | mesuré |
| DLL du paquet | `XboxBvhBuilder` (7 imports), `amd_fidelityfx_loader_xbox` (4), `libHttpClient.GDK` (38), `Bink2Scarlett` (23), plus les copies du runtime MSVC. `make_game.py --check` de Leonida signale tous les fichiers attendus du paquet présents | mesuré ; présence, pas authenticité |

L'origine du paquet n'est pas authentifiée, comme dans les autres notes.

## 2. Couverture des imports

Trois vues des mêmes 587 imports. Ce sont des rapprochements de noms statiques : un export nommé ne prouve ni une
implémentation complète ni une ABI correcte.

| Fournisseur | Résultat | Source |
| --- | --- | --- |
| `gta6rt --dry-run` de Leonida | shim 52, hôte Windows 432, module du paquet 72, **non implémentés 31** (code 0) | mesuré, Codex |
| Leonida, table statique | même répartition, avec 30 non implémentés : `XMemTransferMemory` est servi par le source mais signalé absent par un ancien binaire local | mesuré, Claude |
| Outil de couverture d'AnyXboxX | paquet 209, AnyXboxX 6, TomBox 15, Windows 341, **manquants 16** (9 `AcpHal`, 5 `chad`, `DStorageGetFactory`, `XGSuggestSwizzleMode`) | mesuré, Codex |

Les 16 d'AnyXboxX et les 31 de Leonida ne sont pas comparables : AnyXboxX compte les DLL de TomBox comme
disponibles (via `ANYXBOXX_TOMBOX_DIR`) et tous les ensembles d'API Windows comme servis.

Imports sans implémentation chez Leonida (31) :

| DLL | Fonctions | Statut |
| --- | --- | --- |
| `xmem.dll` (8) | `XMemAllocDefault`, `XMemFreeDefault`, `XMemAllocatePhysicalPages`, `XMemFreePhysicalPages`, `XMemMapPhysicalPages`, `XMemMapPhysicalPagesScatter`, `XMemIsToolsMemAvailable`, `XMemTransferMemory` | mesuré |
| `d3d12_xs.dll` (1) | `D3D12ConfigureVirtualMemoryX` | mesuré |
| `xg_xs.dll` (2) | `XGCreateTextureComputer`, `XGSuggestSwizzleMode` | mesuré |
| `XCRDAPI.dll` (1) | `DStorageGetFactory` | mesuré |
| `chad.dll` (5) | `XDspActivate`, `XDspConnectWithMaximumStreamLimit`, `XDspDeactivate`, `XDspDisconnect`, `XDspSubmitCommand` | mesuré |
| `xapu.dll` (5) | `XApuConnect`, `XApuDequeueResult`, `XApuDisconnect`, `XApuEnqueueCommand`, `XApuGetResultCount` | mesuré |
| `AcpHal.dll` (9) | `AcpHalCreate`, `AcpHalAllocateShapeContexts`, `AcpHalReleaseShapeContexts`, `ApuAlloc`, `ApuCreateHeap`, `ApuFree`, `ApuIsVirtualAddressValid`, `ApuMapApuAddress`, `ApuMapVirtualAddress` | mesuré |

Un import absent se lie à un thunk qui renvoie 0 et n'écrit aucun pointeur de sortie.

Où en est AnyXboxX pour les mêmes imports (lecture de `runtime/` et des crates TomBox, sans lancement) :

| Groupe | `runtime/` d'AnyXboxX | DLL TomBox (`ANYXBOXX_TOMBOX_DIR`) | Statut |
| --- | --- | --- | --- |
| `xmem` | `XMemAlloc`, `XMemFree`, `XMemAllocDefault`, `XMemFreeDefault`, crochets, `XMemVirtualQuery`, `XMemGetWorkingSetStatistics` | `XMemVirtualAlloc`, pages physiques, scatter, `XMemIsToolsMemAvailable`, `XMemTransferMemory` | symboles lus ; comportement sur les formes de la section 4 non testé |
| `d3d12_xs` | quatre redirections de signatures racine vers `d3d12.dll` | `D3D12XboxCreateDevice`, `D3D12ConfigureVirtualMemoryX` | symboles lus |
| `xg_xs` | aucune | `XGCreateTextureComputer` | `XGSuggestSwizzleMode` n'existe nulle part |
| `XCRDAPI` | aucune | aucune | `DStorageGetFactory` n'existe nulle part |
| `xapu` | aucune | cinq fonctions | |
| `chad`, `AcpHal` | aucune | aucune | |

## 3. Chemin de démarrage statique

Tout ce qui suit est mesuré par désassemblage statique, sur les appels directs seulement. Le jeu atteint ses
imports par des chemins indirects (vtables, tables de callbacks) : « atteint depuis `WinMain` » est un minorant.

| Étape | Où | Quoi |
| --- | --- | --- |
| 1 | point d'entrée | `__security_init_cookie`, puis `__scrt_common_main_seh` |
| 2 | initialiseurs C | table de 4 entrées (`_initterm_e`) |
| 3 | initialiseurs C++ | table de 1 115 entrées (1 114 non nulles), presque seulement `VCRUNTIME140`, `malloc` et `atexit`. Aucun n'appelle directement un import console |
| 4 | TLS dynamique | `__dyn_tls_init` ; le callback 1 exécute 15 initialiseurs `thread_local` seulement pour `DLL_THREAD_ATTACH` : le runtime doit l'appeler avec la raison 2 pour chaque thread invité. L'indice TLS du fichier doit être écrasé ; le bloc fait 5 248 octets |
| 5 | `WinMain` | première action : `XGameRuntimeInitialize` de la bibliothèque GDK liée statiquement. Un HRESULT négatif mène à un `int3` |
| 6 | chargeur du GDK | `GetModuleFileNameW(NULL)` avec le nom de fichier remplacé par `xgameruntime.dll`, `GetFileAttributesW`, valeur de registre `ForceUseLocalServices` sous `GamingServices`, `GetPackagesByPackageFamily`, puis `LoadLibraryExW("xgameruntime.dll", NULL, 0x800)` (`0xA00` en mode local) |
| 7 | six `GetProcAddress` | `QueryApiImpl`, `InitializeApiImpl`, `InitializeApiImplEx`, `InitializeApiImplEx2`, `XErrorReport`, `UninitializeApiImpl`. `QueryApiImpl`, `XErrorReport`, `UninitializeApiImpl` et une variante `Initialize*` doivent être non nuls, sinon HRESULT `0x89240101` puis `int3` |
| 8 | appel de l'initialiseur | deux constantes 64 bits (`0x633600002712`, `0x65f400001edd`) et un mot d'options où les bits `2`, `4`, `8` sont ajoutés. Sens des constantes : supposé |
| 9 | test de console | la fonction de test de mode console lit `HKLM\Software\Microsoft\Windows NT\CurrentVersion\OEM`, DWORD `ConsoleMode`. Absent renvoie 1. Seules les valeurs converties 1, 6, 7, 8 passent, sinon `int3`. Le brut `0x64` donne 7, `0x60` donne 6, `0x68` donne 8 |
| 10 | après le GDK | six enveloppes de `CreateSemaphoreExW`, `RegisterAppStateChangeNotification` et `RegisterAppConstrainedChangeNotification`, puis l'init du jeu et la boucle principale |
| 11 | init graphique | appelle l'init du device et de la fenêtre puis, plus tard, l'init de la seconde famille mémoire |
| 12 | init du device et de la fenêtre | `RegisterClassExA`, `CreateWindowExA`, `ShowWindow`, `D3D12XboxCreateDevice(NULL, &params, riid, &device)`, une longue série de méthodes du device, puis le front mémoire de la section 4 |

`D3D12XboxCreateDevice` reçoit un bloc de paramètres de 0x40 octets dont les deux premiers champs valent `0x20004`
et `0x100000`, et un `riid` pris dans les données en lecture seule. Le signe du HRESULT est testé.

Autres faits statiques pour la suite : `DStorageGetFactory` est appelée paresseusement par un périphérique de
fichiers. Il faut un pointeur de sortie non nul et un HRESULT 0, sinon `int3` ; puis la
méthode à l'offset de vtable `+0x40` est appelée avec `0x2000000` (une taille de tampon : supposé), le signe du
retour est testé. `GameInput.dll!#1` se lie à la `GameInput.dll` de l'hôte, car Leonida
enregistre son shim sous `GameInputCreate` et non `#1`.

## 4. Arrêts mesurés sous `gta6rt`

Copie personnelle de l'exécutable, Windows 11, RTX 4070 SUPER, `gta6rt` compilé depuis Leonida `main` `1cc6e3c1`,
paquet monté en lecture seule. La première colonne donne l'ordre où nous les avons rencontrés. Les correctifs
cités sont des pull requests ouvertes ou fermées de [Leonida](https://github.com/ItsNotRexed/Leonida), créditées à
leurs auteurs. « Bouche-trou local » désigne un correctif privé, jamais publié, servant à trouver l'arrêt suivant.

| # | Arrêt | Cause | Statut | Pull request qui le traite |
| --- | --- | --- | --- | --- |
| 0 | code de sortie 127 ou 3 en moins d'une seconde, aucun import appelé, 12 événements de trace (mappages, `DllMain`, callbacks TLS), aucune exception tracée | un `int3` n'est pas un événement de la trace ; expliqué par les arrêts suivants | mesuré ; explication supposée | aucune : leçon pour l'outillage (section 5) |
| 1 | `UNIMPLEMENTED d3d12_xs.dll!D3D12ConfigureVirtualMemoryX`, sortie 3, appelée depuis l'init mémoire du titre | import absent. Même arrêt sur la version de référence, 3 lancements en mode `stop` et 3 en mode `log` | mesuré | #6792 de undeadhuman37 (fusionnée le 2026-11-23) |
| 2 | `int3` `0x80000003` dans la même init, juste après `Configure...` rendu `S_OK` | le troisième `XMemVirtualAlloc` de l'init (taille `0x49400000`, attributs `0x20003000`, `r9 = 0x183`, protection `0x22004`) échoue avec `ERROR_NOT_SUPPORTED` | mesuré avec un bouche-trou local qui ignore le mot d'attributs et garde `protect & 0xff` | #6709 de Stalkerino (formes de réservation de 64 Kio), #4531 de labj1987 (pages de 2 et 4 Mio), #6878 de mrp1no (fermée ; mot d'attributs non nul, bits de protection console), #6919 de NiiRoZz (ouverte ; pages de 2 Mio et espaces réservés). Ajoutées tour à tour par-dessus #6792, #6709, #4531 et #5253 (maxxgabriel) laissent le même `int3` (bisect) |
| 3 | `int3` dans l'enveloppe d'allocation de mémoire physique après `XMemAllocatePhysicalPages(rcx = 0x280, ...)` | pages physiques non servies. Un bouche-trou qui rend `rcx * 64 Kio` de `VirtualAlloc` donne le même arrêt ; `XMemMapPhysicalPages` n'est pas appelée avant. La vraie forme du handle n'est pas établie | mesuré ; cause supposée | #6919 (les sept imports `xmem` manquants) ; l'issue de coordination range #6878 sur le même terrain |
| 4 | `int3` dans l'init du device : slot 48 du device (`CreateCommandQueueX`), deuxième et troisième file | descriptions de file `{Type 2, Priority 0, Flags 0x1 / 0x2 / 0x3, NodeMask 0}`. Le PC n'accepte que `0x1`, donc `0x2` donne `E_INVALIDARG` | mesuré | aiguillage dans #6935 de Kam471 (fusionnée) ; la mesure de `Flags` est notre commentaire de revue dessus ; #6954 de Pipodi et #7037 de sneylol (ouvertes) |
| 5 | `int3` dans l'init du device : slot 73 du device (`CreateCommandListX`) | `riid = c3827890-4024-474c-98cf-645b51dc4874`, seul GUID de cette forme dans `.rdata`. Le device PC répond `E_NOINTERFACE` (`0x80004002`). Le même device et le même allocateur DIRECT créent la liste pour l'IID PC `c3827890-e548-4cfa-96cf-5689a9370f80` (test synthétique indépendant, Codex) | mesuré. Que l'IID console soit l'`ID3D12GraphicsCommandList6` du PC est supposé : seuls les 32 premiers bits coïncident | #7076 de NiiRoZz (ouverte ; liste proxy console, dépend de #6935 et #2490 de NolanCotter). Notre #7124 a été fermée en sa faveur, mesures ajoutées là-bas |
| 6 | `int3` dans l'init du device : le slot 9 de la liste (`+0x48`, `Close` dans la table PC) rend un HRESULT négatif | juste après la création, le titre appelle les slots 132 (`+0x420`) puis 131 (`+0x418`) de la liste, résultats inutilisés, puis le slot 9, testé. La liste PC a 80 slots : une liste PC brute ne peut pas servir `+0x420`. Sens de 131 et 132 : inconnu | mesuré ; nature des slots non établie | aucune pour l'instant ; #2490 (listes de commandes à l'ABI console) est la plus proche |

Formes d'arguments des slots 132 et 131 de la liste (statique, Codex). Première liste : slot 132
`(0x700000, 0, 0, 0xffffffffff00)`, slot 131 `(0xc3c0, 0x80020000, 0, 0xffffffffff00)`. Seconde liste, dans une
boucle : slot 132 `(0x100000, 0, 0, 0xffffffffff00)`, slot 131 `(0xc380, 0x20000, 0, 0xffffffffff00)`. Les
constantes sont passées par valeur ; rien n'indique que ce soient des tailles ou des pointeurs.

### Prédit par l'analyse statique, non rencontré comme arrêt de notre côté

| Prédiction | Détail | Statut |
| --- | --- | --- |
| `XErrorReport` non exporté par le shim | `GetProcAddress("XErrorReport")` rendant NULL mène à `0x89240101` puis `int3` dans `WinMain`. Lors du bisect, #6582 (Kam471, ouverte) n'a eu aucun effet sur l'arrêt : la base passait déjà ce point | supposé ; la prédiction n'a pas été reproduite |
| `DStorageGetFactory` | un thunk non implémenté laisse la sortie non écrite et le titre fait `int3` | mesuré en statique ; comportement à l'exécution rapporté par d'autres (arrêt 24 ci-dessous) |
| `GameInput.dll!#1` | se lie à la DLL de l'hôte, contourne le shim SDL3 | mesuré en statique |

### Mesures publiques d'autres contributeurs

KkelebekK (Linux, Wine 11.21, Leonida #6910, 2026-11-23) est allé plus loin avec des bouche-trous locaux et a
numéroté ses propres arrêts de 13 à 26. Leur numérotation n'est pas la nôtre. Nous ne gardons que ce qui concerne
AnyXboxX.

| Leur arrêt | Forme | Pertinence |
| --- | --- | --- |
| 13 | tas de descripteurs : la description console porte l'adresse du titre à `+0x10` et une taille à `+0x18` ; le slot 9 du tas doit rendre cette adresse (#6915) | modèle de tas |
| 16 | slot 127 du device, forme `CreateCommittedResource3`, avec une disposition de barrière | ABI du device |
| 19, 20, 21 | slots 41, 61, 74 et 86 du device | ABI du device |
| 22 | `XGSuggestSwizzleMode`, puis `XGCreateTextureComputer` ; le slot 8 du calculateur est appelé une fois par mip. Un `E_NOTIMPL` qui laisse la sortie non écrite fait appeler le titre dans du vide (#6891, #7009) | `xg_xs` |
| 23 | `XMemMapPhysicalPagesScatter`, un appel de `0x1a40` pages | `xmem` |
| 24 | DirectStorage ouvre sa file ; #4348 de mariuslozo-boop répond `E_NOTIMPL` aux demandes zlib, BCPack et swizzle et l'exécution continue | `XCRDAPI` |
| 26 | slot 57 du device, une fois par pipeline, puis le slot 108 | pipelines |

Analyse statique de Codex sur le slot 57 : `rdx` pointe un
bloc dont les 8 premiers octets sont une référence d'objet, `riid` est l'`ID3D12PipelineState` du PC
(`765a30f3-f624-4c6f-a828-ace948622445`), le pointeur de sortie est le quatrième argument, et le signe du résultat
est testé. Le premier champ est très probablement une root signature : il vient du slot 16 du device (`+0x80`,
forme `CreateRootSignature`) avec l'IID PC `ID3D12RootSignature` (`c54a6b66-72df-4ee8-8be5-a946a1429214`). La
structure après le premier champ n'est pas reconstituée. La fence de la mise en place des files vient du slot 36
du device (`+0x120`) avec l'IID PC `ID3D12Fence` (`0a753dcf-c4d8-4b91-adf6-be5a60d95a76`) ; une création qui
échoue fait rendre faux à la fonction d'aide.

### Front mémoire, dans l'ordre des appels (statique)

Dans l'init du device et de la fenêtre, après `D3D12XboxCreateDevice`. Les arguments ne sont que des formes.

| Appel | Forme | Branche d'échec |
| --- | --- | --- |
| slot 49 du device (`+0x188`) | `(0x11f, 1)` | résultat inutilisé |
| `XMemVirtualAlloc` | `(0, 0x10000, 0x20401000, 0x283, prot 0x42404)` | NULL : repli, pas d'`int3` |
| slots 26, 8, 48, 9, 73, 79, ... du device | files, allocateurs, première liste (rôles probables, d'après la table PC) | résultats non testés, sauf slots 48 et 73 (arrêts 4 à 6) |
| `XMemVirtualAlloc` x6 | le premier `(…, 0x42000, 7, 1)`, puis cinq avec `0x20406000`, `r9 = 0x27`, protections `0x1004`, `0x1404`, `0x8404`, `0x2404`, `0x2004` | NULL : nettoyage, pas d'`int3` |
| `XMemAllocatePhysicalPages` | `(0x280, &nombre entrée/sortie, tableau de PFN de nombre * 8 octets)`, renvoie un BOOL | 0 : nettoyage |
| `XMemMapPhysicalPages` | `(adresse réservée, nombre, tableau de PFN)`, renvoie un pointeur | NULL : nettoyage |

La seconde famille (plus tard) demande `XMemVirtualAlloc(0, taille, 0x20402000, 0x21, 4)` puis
`(…, 3, 0x801)`, dont un NULL mène à `int3` ; puis `XMemAllocatePhysicalPages(0x380, ...)`, un bloc mappé, le slot
74 du device (`+0x250`) et le slot 8 du device (`+0x40`), chacun testé en signe avec `int3` en cas d'échec. Pour
le mot d'attributs `0x20402000`, un retour NULL ne doit jamais arriver.

## 5. Ce que cela implique pour AnyXboxX

Ordre de travail, d'après les arrêts mesurés. Les éléments « supposé » demandent un test avant qu'on s'appuie
dessus.

| # | Travail | Base mesurée | Statut de la base |
| --- | --- | --- | --- |
| W1 | **Fait, PR #107.** **Comportement de `xmem` sur les formes réelles.** Ajouter un test synthétique (règle 4, aucun fichier du jeu) qui appelle le `xmem` chargé par AnyXboxX, le nôtre ou celui de TomBox, avec les mots de `XMemVirtualAlloc` mesurés (attributs `0x20401000`, `0x20402000`, `0x20406000`, `0x20003000`, `0x42000` ; `r9` dans `0x3`, `0x7`, `0x21`, `0x23`, `0x27`, `0x183`, `0x283` ; protections listées plus haut) et avec le triplet de pages physiques (`0x280`/`0x380`, nombre entrée/sortie, tableau de PFN de 8 octets, pointeur mappé). Jamais de NULL pour `0x20402000` | arrêts 2, 3 ; table des appels du front | formes mesurées ; comportement de la DLL TomBox inconnu |
| W2 | **Fait, PR #107.** `runtime/xmem` ne sert lui-même aucun des sept imports manquants (`XMemVirtualAlloc`, trois appels de pages physiques, scatter, `XMemIsToolsMemAvailable`, `XMemTransferMemory`). Décider : continuer à compter sur TomBox, ou les écrire en C++ avec le décodeur ERA, et le dire dans [tombox.md](tombox.md) | section 2 | mesuré |
| W3 | **Fait, PR #107.** `XMemAlloc(0x60000, 0xf0814000)` : vérifier que le décodeur ERA de `runtime/xmem/era_heap.cpp` ne refuse pas ce mot (« mémoire physique refusée » renvoie NULL, que le titre traite comme une branche d'erreur) | site d'appel statique | décodage de ce mot supposé |
| W4 | **Fait, PR #107.** `D3D12ConfigureVirtualMemoryX` : le `d3d12_xs` d'AnyXboxX ne l'exporte pas (TomBox l'exporte et rejette une description NULL par `E_INVALIDARG`). Tester une description non nulle dont le deuxième champ de 32 bits dérive d'une taille, attendre `S_OK` | arrêt 1 | forme mesurée ; sens du champ supposé |
| W5 | **Résolu en réutilisant TomBox (section 6).** **Table des slots du device.** Comparer les slots que le titre emploie avant le premier échec de `Close` (8, 9, 16, 26, 36, 41, 48, 49, 57, 61, 73, 74, 79, 86, 127, et les slots 9, 131, 132 de la liste) avec le `d3d12_xs` de TomBox (194 méthodes sur 200 revendiquées). Résultat : un tableau slot, forme, couvert ou non, dans [tombox.md](tombox.md) | sections 3 et 4 | slots mesurés ; couverture TomBox non comparée |
| W6 | **Non fait : demande d'abord le proxy de liste console (Leonida #7076), voir section 6.** Liste de commandes console : établir ce que sont les slots 131 et 132 de la liste, à partir de l'ABI de liste console de TomBox, avant d'écrire une enveloppe. Ne pas donner une liste PC brute au titre | arrêts 5, 6 | sens des slots inconnu |
| W7 | **Fait, cette PR : `XCRDAPI.dll` minimal.** **`DStorageGetFactory`** manque chez AnyXboxX et TomBox. Une `XCRDAPI.dll` minimale doit écrire le pointeur de sortie, rendre 0, et exposer la méthode à `+0x40` rendant un HRESULT non négatif ; les lectures de fichier peuvent répondre `E_NOTIMPL` comme le fait #4348. #4348 de Leonida est en GPL-3.0, réutilisable avec crédit (règle 2) | statique, arrêt 24 | mesuré |
| W8 | **Bloqué, voir section 6.** **`xg_xs`** : `XGSuggestSwizzleMode` manque. `XGCreateTextureComputer` doit rendre un objet dont le slot 8 existe même s'il n'écrit aucune donnée tuilée, et aucun stub ne doit laisser un pointeur de sortie non écrit. `anyxboxx-texture` (SW_4K_S) est le moteur naturel | arrêt 22 (KkelebekK) | mesuré par d'autres |
| W9 | **Fait, cette PR : `tests/test_gdk_boot.py` ; type de retour gardé, voir section 7.** `xgameruntime` : `XErrorReport` est exporté avec un retour `void` (`runtime/xgameruntime/xgameruntime.def`) ; le titre ne teste que le pointeur, donc cela paraît sans effet. Ajouter un test du lanceur : les six noms de `GetProcAddress` se résolvent, les bits d'options `2`, `4`, `8` sont acceptés, et `xgameruntime.dll` se charge par chemin complet quand l'exécutable du jeu est dans un autre dossier | étapes 6 à 8 | innocuité du type de retour supposée |
| W10 | **Fait, cette PR : voir section 7.** `ConsoleMode` : `runtime/xgameruntime/registry.cpp` sert `0x64`, qui se convertit en 7 et passe le test du titre. Garder un test qui fige `0x64`, et la variante `RegOpenKeyW` (celle du titre) | étape 9 | mesuré et cohérent |
| W11 | **Fait, cette PR : nouveaux threads vérifiés ; les threads antérieurs à l'installation ne reçoivent pas la raison 2, comme sous Windows, voir section 7.** **TLS** : vérifier qu'`anyxboxx_tls.dll` appelle le callback 1 du titre avec la raison 2 pour chaque thread, existant ou nouveau, et que l'indice du fichier est écrasé. Le bloc (5 248 octets) tient dans la capacité de 256 Kio | étape 4 | taille mesurée ; couverture des threads non testée |
| W12 | **Observabilité.** Le lanceur natif n'a aucun gestionnaire pour `0x80000003` : un `int3` du titre finit en simple code de sortie (arrêt 0). Ajouter un gestionnaire vectoriel qui journalise l'adresse du premier `int3` ou de la première exception et le module, pour qu'un bisect donne un arrêt et non un code de sortie | arrêt 0 | mesuré |
| W13 | **Fait, cette PR : dépendance à l'hôte documentée dans [jour-j.md](../jour-j.md), pas de shim, voir section 7.** `GameInput.dll!#1` : AnyXboxX n'a pas de shim GameInput. L'unique appelant du titre se lie à la DLL de l'hôte. Choisir : servir l'ordinal 1 depuis le lanceur, ou documenter la dépendance à l'hôte | statique | mesuré ; version d'ABI non vérifiée |
| W14 | Audio (`chad` 5, `xapu` 5 chez TomBox, `AcpHal` 9) : manquant, mais non établi comme bloquant au démarrage, la chaîne d'appels étant indirecte. À traiter après W1 à W8 | section 2 | supposé |
| W15 | **Fait, cette PR : noté dans [jour-j.md](../jour-j.md), section 3.** Imports propres à l'hôte : `api-ms-win-core-psm-appnotify-*`, `ext-ms-win-rtcore-ntuser-*`, MFPlat, WS2_32 par ordinal. Le mode natif dépend d'un hôte Windows récent ; le noter dans [jour-j.md](../jour-j.md) | section 2 | mesuré sur Windows 11 seulement |
| W16 | **Non fait : aucune source publique, voir section 7.** `__XMemAllocationHooks__` : le titre exporte un symbole de données qu'aucun source de Leonida ne lit. Si `XMemAlloc*` se comporte mal, vérifier si le runtime de la console le lit par `GetProcAddress` sur l'exécutable | table d'exports | rôle supposé |

W1, W3, W7, W8, W9, W10, W11 et W12 se font à partir de la documentation publique et de tests synthétiques. W5 et
W6 n'ont besoin que des sources de TomBox. Aucun ne demande de relire le jeu.

## 6. Résultats de W5 à W8 (cette PR)

- **W5.** La table des slots de périphérique est dans [tombox.md](tombox.md), figée par `tests/test_d3d12_device_slots.py`.
  AnyXboxX ne servait aucun slot de périphérique. **Résolu en réutilisant TomBox**, sans écrire de proxy :
  `tools/build_tombox.py` compile le `d3d12_xs` de TomBox (64 exports, proxy du périphérique vers le D3D12 de l'hôte),
  que `runtime/build/tombox` sert désormais par défaut. `tests/test_tombox_native.py` obtient un périphérique via
  `D3D12XboxCreateDevice` (IID_ID3D12Device, WARP ou GPU). La même exécution vérifie W1 (`XMemVirtualAlloc` avec
  `0x20402000` ne rend jamais NULL) et la sortie de `DStorageGetFactory` (W5), par le `xmem` et le `XCRDAPI` de TomBox.
  Restent les nôtres, absents de TomBox : `xgameruntime`, `kernelx`, `anyxboxx_tls`, et l'export `XMemGetAllocationHooks`
  de xmem, que TomBox n'a pas (aucun import ne l'utilise dans l'analyse statique du titre ; à proposer en amont).
- **W6.** Non fait. Les slots 131 et 132 appartiennent à une liste de commandes console, qu'AnyXboxX n'a pas ; le proxy
  de liste (Leonida #7076) vient d'abord. Quand il existera, ces slots journaliseront leurs arguments et rendront
  `E_NOTIMPL` (le titre ignore leurs résultats), jamais 0.
- **W7.** `runtime/xcrdapi/` compile `XCRDAPI.dll`. `DStorageGetFactory` écrit une factory à la disposition console
  (slots 0 à 8). Le slot 8 (`SetStagingBufferSize`, +0x40) rend `S_OK` et journalise la taille, ignorée faute de tampon
  de transit : un no-op documenté qui laisse passer le test de signe du titre. `CreateQueue`, `OpenFile`,
  `CreateStatusArray` et `SetCpuAffinity` rendent `E_NOTIMPL` avec une sortie NULL. Les lectures de fichier ne sont pas
  encore servies. Tests : `tests/test_xcrdapi.py`.
- **W8.** Bloqué. `XGSuggestSwizzleMode` n'a pas de signature publique (recherchée dans Leonida, TomBox, le GDK local et la
  documentation publique) : aucun argument de sortie ne peut être écrit sans risque. `XGCreateTextureComputer` de TomBox
  (`crates/compat/xg_xs/src/lib.rs`, lignes 372 à 381) sort tôt pour une dimension supérieure à 4 et sur sa branche
  d'erreur sans écrire `*out`. Observé dans le source, pas mesuré sur le titre. La correction relève de l'amont, puisque
  `third_party/` n'est pas modifié ici.

## 7. Résultats de W9, W10, W11, W13 et W15 (cette PR ; W12 est traité dans l'issue #113)

- **W9.** `tests/test_gdk_boot.py` fabrique un titre (avec `tests/pe_builder.py`) dans son propre dossier, loin du
  runtime et sans `xgameruntime.dll` à côté, et rejoue les étapes 6 à 8 : `GetModuleFileNameW(NULL)` avec le nom de
  fichier remplacé par `xgameruntime.dll`, `LoadLibraryExW` de ce chemin complet avec `0xA00`, puis du nom seul avec
  `0x800`, les six `GetProcAddress`, et les trois `Initialize*` avec les deux constantes et les options `2 | 4 | 8`.
  Les deux chargements doivent rendre le même module d'AnyXboxX, les six noms doivent se résoudre, et chaque
  initialiseur doit rendre `S_OK`. Le même programme tourne sous `anyxboxx.exe` et sous le mode natif Python. Sans
  la redirection par chemin complet, le premier chargement échoue et le résultat tombe à 0 (vérifié en mutant le shim
  Python). L'ordre des arguments des initialiseurs (constantes dans `rcx`, `rdx`, options dans `r8`) est supposé ; nos
  initialiseurs ignorent leurs arguments quand le runtime de Windows est coupé, le test fige donc qu'aucun bit
  d'option n'est refusé.
  `XErrorReport` garde son retour `void`. Microsoft Learn ne documente aucun `XErrorReport` : la page
  [XError members](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xerror/xerror_members) ne
  liste que `XErrorCallback`, `XErrorSetCallback`, `XErrorSetOptions` et `XErrorOptions`. C'est un export interne
  que `xgameruntime.lib` résout par son nom : il n'existe aucune signature publique à laquelle le corriger, et le titre
  ne teste que le pointeur (statique, section 3). Ses paramètres `(HRESULT, const char*)` reprennent les deux premiers
  du [XErrorCallback](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xerror/functions/xerrorcallback)
  documenté. La décision est écrite à côté de la fonction dans `runtime/xgameruntime/xgameruntime.cpp`.
- **W10.** `0x64` est figé deux fois. `tests/test_runtime_cpp.py` ouvre la clé OEM par `AnyXboxX_RegOpenKeyW`
  (avec et sans barre oblique initiale, en toute casse) et lit `ConsoleMode` en `REG_DWORD` `0x64`, avec
  `ERROR_MORE_DATA` et la taille 4 pour un tampon de 2 octets ; `XSystemGetDeviceType` reste à 7.
  `tests/test_gdk_boot.py` fabrique un titre qui importe `ADVAPI32!RegOpenKeyW`, `RegQueryValueExW` et `RegCloseKey`,
  comme le titre, et lit `0x64` sous `anyxboxx.exe` et sous le mode natif Python. Sans la redirection de
  `RegOpenKeyW`, le même programme lit 0 sur un PC (vérifié en mutant la liste de redirections Python). La conversion
  de `0x64` en 7 se fait dans le `xgameruntime.lib` du titre et n'est pas testable ici (lecture statique, section 3).
- **W11.** `tests/test_native_tls_callbacks.py` charge un titre dont le callback TLS compte chaque raison, garde son
  premier argument et, pour la seule raison 2, écrit un marqueur dans la copie TLS du thread appelant, comme le
  `__dyn_tls_init` de MSVC le fait avec ses initialiseurs. Vérifié : le `_tls_index` de l'image est écrasé (il ne vaut
  plus `0xFFFFFFFF`) ; le callback reçoit la base de l'image et `DLL_PROCESS_ATTACH` une fois, à l'installation ;
  chaque nouveau thread l'exécute avec la raison 2 sur ce thread, après le remplissage de sa zone et avec le bon
  indice (chacun de quatre threads lit le marqueur), et avec la raison 3 à sa sortie. Avec l'appel de raison 2
  d'`anyxboxx_tls.dll` mis en commentaire, le test des nouveaux threads échoue (mutation du runtime C++, recompilé).
  Aucun changement C++ n'a été nécessaire.
  Deux threads ne reçoivent pas la raison 2 d'AnyXboxX, volontairement. Le thread qui installe l'image reçoit
  `DLL_PROCESS_ATTACH`, comme Windows le donne au thread principal d'un exécutable ; le démarrage du CRT MSVC du titre
  appelle ensuite lui-même son callback TLS dynamique avec `DLL_THREAD_ATTACH` pour ce thread (`exe_common.inl` et
  `tlsdyn.cpp` dans les sources du CRT livrées avec Visual Studio). Un thread né avant l'installation reçoit le modèle
  copié dans sa zone, mais aucun callback : Windows fait de même pour une DLL chargée après le démarrage de ses threads
  ([DllMain](https://learn.microsoft.com/en-us/windows/win32/dlls/dllmain), `DLL_THREAD_ATTACH`), et le code compilé
  avec [`/Zc:tlsGuards`](https://learn.microsoft.com/en-us/cpp/build/reference/zc-tlsguards) (actif par défaut depuis
  Visual Studio 2019 16.5) initialise ses variables `thread_local` au premier usage sur ces threads, car le modèle
  copié laisse la garde à faux. Exécuter du code sur un autre thread pour lui livrer la raison 2 demanderait de
  détourner ce thread, ce que nous ne faisons pas. Que le titre soit compilé avec les gardes TLS est supposé (sa chaîne
  d'outils est récente, section 1, mais la garde n'a pas été cherchée). Le lanceur installe la TLS avant le point
  d'entrée du titre, sur le même thread : dans `anyxboxx.exe`, les threads antérieurs à l'installation sont seulement
  ceux du lanceur et du chargeur de Windows.
- **W13.** L'ordinal 1 est `GameInputCreate(IGameInput**)`. Sources : Microsoft Learn dit qu'en GameInput v0,
  `GameInputCreate` est exporté par `GameInput.lib` et crée toujours l'interface v0, que les bibliothèques des
  versions antérieures, « comme celle du GDK », continuent de l'appeler et sont relayées au runtime installé, et que
  tout runtime accepte la v0 ([GameInputCreate](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputcreate),
  [GameInputInitialize](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/functions/gameinputinitialize),
  [versions de l'API](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-version-interop)).
  La bibliothèque NuGet récente exporte `GameInputInitialize` à la place : un import de `GameInputCreate` par ordinal
  correspond donc à la bibliothèque v0 du GDK. Mesuré sur notre hôte Windows 11 : `System32\GameInput.dll`
  (0.2309.26100.9818) exporte `GameInputCreate` à l'ordinal 1, tout comme `System32\GameInputRedist.dll` (4.0.306.0,
  avec `GameInputInitialize` à 2). La question d'ABI de la ligne est donc réglée pour la fabrique : v0, que tout runtime
  hôte sert. Que la `GameInput.dll` de la console numérote aussi `GameInputCreate` 1 est supposé, d'après l'import
  statique et la bibliothèque du GDK.
  **Choix : pas de shim, l'hôte le sert.** Un `GameInputCreate` du runtime répondant `E_NOTIMPL` (règle 3) priverait
  de vraies manettes un titre que l'hôte sert déjà, et un `GameInputCreate` fonctionnel doublerait le runtime de l'hôte.
  `tests/test_launcher.py` vérifie qu'`anyxboxx.exe` lie `GameInput.dll!#1` à l'hôte et que l'appel rend `S_OK` avec un
  objet (ignoré sur un hôte sans GameInput), et qu'une bibliothèque de l'hôte à laquelle manque un import par ordinal
  arrête le lanceur en nommant l'import (`dll!#1`), jamais un thunk muet. `tests/test_native_services.py` vérifiait
  déjà la même liaison en mode natif Python. [jour-j.md](../jour-j.md) dit quoi installer quand GameInput manque.
- **W15.** [jour-j.md](../jour-j.md) liste désormais, sous « Ce que l'hôte Windows doit fournir », GameInput (W13) et
  les imports propres à l'hôte de la section 2, avec ce qu'on en sait : mesurés sous Windows 11 seulement ; la
  présence d'un API set se décide appareil par appareil, quel que soit le préfixe `api-` ou `ext-`
  ([Windows API sets](https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-apisets)) ;
  `RegisterAppStateChangeNotification` est hébergé par `twinapi.core.dll` derrière
  `api-ms-win-core-psm-appnotify-l1-1-0`/`-l1-1-1`
  ([Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/appnotify/nf-appnotify-registerappstatechangenotification)) ;
  les éditions N n'ont pas Media Foundation sans le Media Feature Pack. Documentation seulement, sans test.
- **W16.** Non fait. Les pages de référence `xmem` du GDK (`XMemSetAllocationHooks` et la macro associée) ne sont
  servies par Microsoft Learn qu'aux programmes de développeurs sous NDA (vérifié le 2026-11-24) : ce n'est donc pas une
  source publique au sens de la règle 2, et aucune page publique ni aucun code open source trouvé ne lit cet export. La
  ligne reste ouverte jusqu'à une source publique ou une mesure sur le titre.

## Sources et crédits

- Analyse statique de l'exécutable : Claude (REA 6.10.1, capstone, bornes `.pdata`). Rapports, balayage des IID,
  décompilations Ghidra ciblées et test indépendant de `CreateCommandList` : Codex.
- Pull requests et coordination de Leonida : [#6935](https://github.com/ItsNotRexed/Leonida/pull/6935) (Kam471),
  [#7076](https://github.com/ItsNotRexed/Leonida/pull/7076) (NiiRoZz), #7124 (la nôtre, fermée),
  [#6910](https://github.com/ItsNotRexed/Leonida/issues/6910) et
  [#7049](https://github.com/ItsNotRexed/Leonida/issues/7049) (coordination, KoertLichtendonk),
  #6792, #6709, #4531, #6878, #6919, #5253, #6582, #6954, #7037, #4348, #2490.
- Nos commentaires avec les mesures : le commentaire de revue du 2026-11-23 sur #6935 (`Flags` des files) et le
  commentaire sur #7076 (IID de liste de commandes et slots de liste), tous deux limités à des comptes,
  des HRESULT et l'IID.
- Projets voisins en général : [voisins.md](voisins.md) ; notes de Leonida sur GTA 6 :
  [leonida-gta6.md](leonida-gta6.md).
