> English: [leonida-boot-2026-11-23.md](../../research/leonida-boot-2026-11-23.md)

# Démarrer le 1.0.2.1 sous Leonida, 23 novembre 2026 : arrêts, images noires, constats statiques

État au soir du 23 novembre 2026. Cette note range les recherches du jour (Claude et Codex, sur la machine du
mainteneur) sur le démarrage du build 1.0.2.1 sous `gta6rt` de [Leonida](https://github.com/ItsNotRexed/Leonida),
du premier arrêt à la boucle de frames. Elle prolonge [premier-demarrage-1021.md](premier-demarrage-1021.md), qui
s'arrête aux six premiers arrêts et à la liste de travaux d'AnyXboxX. Les projets voisins en général :
[voisins.md](voisins.md) ; les notes de Leonida sur GTA 6 : [leonida-gta6.md](leonida-gta6.md).

**La règle 1 d'[AGENTS.md](../../../AGENTS.md) s'applique, plus strictement que dans la première note.** Cette note
ne contient que des numéros de slot de vtable, des valeurs de drapeaux, des HRESULT, des noms de types publics du
SDK, des numéros de PR, des comptes, des durées et des tailles mémoire. Elle ne contient aucune adresse ni aucun
décalage de l'exécutable, aucun code désassemblé ou décompilé, aucun octet, aucune chaîne, aucun nom de fichier ou de
shader du paquet, aucun journal brut et aucun hash de shader. Les dispositions de descripteurs sont données comme
positions de champs de structures publiques du SDK, jamais comme du code. Les rapports locaux qui les fondent (REA,
Ghidra et capstone côté Claude, rapports de Codex, journaux de lancement) restent hors du dépôt, ainsi que le nom de
leurs fichiers.

**Trois niveaux, dans chaque tableau.**

| Étiquette | Sens |
| --- | --- |
| mesuré | observé sur un lancement, ou lu dans la sortie d'un outil par une méthode reproductible. Dit qui a mesuré. |
| déduit statiquement | lu dans l'exécutable sans le lancer (décompilation ou désassemblage ciblés). Appels directs seulement : c'est une borne basse, sans preuve d'atteignabilité |
| supposé | inféré à partir des deux précédents, d'un nom, ou vrai sur une seule configuration |

Les mesures sont les nôtres, sauf nom cité. Les noms sont des identifiants GitHub de participants de
[Leonida](https://github.com/ItsNotRexed/Leonida) ; leur numérotation des arrêts n'est pas la nôtre, sauf mention.

## 1. Cadre des mesures

| Élément | Valeur | Statut |
| --- | --- | --- |
| Machine | Windows 11 (build 26300), Core i7-13700KF, RTX 4070 SUPER (pilote 610.88), natif, sans Wine | mesuré |
| Runtime | `gta6rt`, MSVC RelWithDebInfo, `LEONIDA_D3D12=vulkan` dès la deuxième passe | mesuré |
| Exécutable | le build 1.0.2.1 déjà décrit (SHA-256 commençant par `e95020b2`), copie personnelle, paquet en lecture seule | mesuré |
| Piles | `main` de Leonida à plusieurs dates (`1cc6e3c1`, puis `ec0e01c5`, puis `dde82f27`) avec des pull requests ouvertes fusionnées en local, conflits résolus à la main, **seul `gta6rt` compilé, tests propres des PR non lancés** sauf mention dans un commentaire de PR | mesuré |
| Base plus tardive | le `main` de la forge des mainteneurs (`2ac38583`, contient #7759), journal dans l'issue #8851 | mesuré |
| Shaders | `make_game --stages shaders` : 26 503 traduits en 32 min sur une pile (paliers wave64 : A 236, B 7, C 425 pour VS, PS, GS) | mesuré |
| Variables des lancements de boucle | `LEONIDA_ARENA_MB=4095` (sinon `E_OUTOFMEMORY`), `LEONIDA_STUB_PIPELINES=1` pour certains, `LEONIDA_SKIP_INT3=1` pour un `int3` de l'image | mesuré |

Résultats ordinaires des tests au repos sur cette machine, main `dde82f27` : ctest 268 sur 272. Les quatre échecs sont
`d3d12_vulkan_device` (un contrôle `tiles == 1`), `d3d12_indirect_dispatch_count`, `d3d12_depth_resolve` et
`d3d12_indirect_rt` ; ils échouent de la même façon sur main et ne viennent pas des PR testées. Sans SDL3, la cale
GameInput n'est pas compilée (ctest range `gameinput` en désactivé) : `-DLEONIDA_SDL3_FETCH=ON` la construit. mesuré.

## 2. La chaîne mesurée, arrêt par arrêt

Chaque ligne est l'arrêt rencontré après avoir ajouté à la pile locale la pull request de la ligne précédente. « t »
est la durée jusqu'à l'arrêt, « pic » le pic du working set. Les numéros 1 à 6 sont ceux de
[premier-demarrage-1021.md](premier-demarrage-1021.md) ; les numéros 13 à 19 sont ceux de KkelebekK (Linux, Wine,
#6910). Les pull requests appartiennent aux personnes nommées.

| Arrêt | Ce que fait le titre à cet endroit | Franchi par | t (s) | Pic (Mo) |
| --- | --- | --- | --- | --- |
| 1, 2 | `D3D12ConfigureVirtualMemoryX` absent, puis `XMemVirtualAlloc` de l'init refusé | #6792 (undeadhuman37), puis les formes du mot d'attributs de #6709 (Stalkerino), #4531 (labj1987), #6919 (NiiRoZz) | 6,3 à 8,3 | 238 |
| (mode log) | `XErrorReport` non exporté par la cale : `WinMain` prend la branche d'échec | #6582 (Kam471) | 4,9 à 7,2 | 238 |
| 24 | `DStorageGetFactory` non implémenté | #4348 (mariuslozo-boop) avec #4313 (index TLS) | 3,1 à 6,5 | 237 |
| (hôte) | violation d'accès dans l'hôte après `XMemAllocatePhysicalPages` (0x280 demandées, 11 360 pages), slot 26 du device | #6964 (sennecools) ; le backend Vulkan fait passer le pic de 395 à 687 Mo | 7,0 à 9,8 | 395, puis 687 |
| 4 | slot 48 du device, `Flags` de file `0x2`, `0x3` | #6954 (Pipodi) | 7,4 | 687 |
| 5 | slot 73 du device, IID console de liste | #7076 (NiiRoZz), avec #6953 (Pipodi) et #2490 (NolanCotter). #6953 et #2490 seules laissent l'arrêt | 7,0 à 8,2 | 687 |
| après 6 | slot 133 du device `E_NOTIMPL` « au-delà de la vtable PC » | #7075 (NiiRoZz) | 6,8 à 7,1 | 687 |
| 13 | tas de descripteurs, slot 9, `GetCPUDescriptorHandleForHeapStart` | #7021 (Khuraie) ; le titre passe ensuite par les slots 17, 18, 19 | 6,5 | 687 |
| (hôte) | violation d'accès en écriture de l'hôte au premier appel du slot 70 | #7066 (tedin7) | 6,5 à 6,9 | 687 |
| (hôte) | violation d'accès en lecture à l'adresse -1, premier appel du slot 25 | #7110 (tedin7). Note de fusion : elle et #6964 définissent toutes deux l'aide `proxy_get_custom_heap_properties` ; ensemble, le fichier ne compile pas (`C2440`) | 7,1 | 687 |
| 16 | slot 127 `E_NOTIMPL`, puis `int3` | #7052 (tedin7) | 6,9 à 7,0 | 687 |
| 17 | test de budget après les cibles de rendu | #7235 (NiiRoZz) : 33 423 360 octets par cible sur ce GPU. #7115 (slot 61, mrp1no) ne change rien ici, car l'arrêt précède le slot 61 | 6,7 à 6,9 | 687 |
| 18 | exception C++ non rattrapée de la DLL BVH du paquet | #5680 (zzEmmazz, fermée) plus les 52 programmes embarqués traduits à la main dans un dossier de travail ; #7180 (KkelebekK, zlib DirectStorage) ne change rien avant | 6,6, puis 31,9 en mode stop | 687, puis 1 091 |
| 19 | `int3` après les slots 39, 27, 41 du device | bouche-trous locaux (section 2.2) | 7 (mode log) | 686 |

Notre #6279 (stores MUBUF étroits) est identique octet pour octet, sur les 52 programmes, à ce que main produit déjà
grâce à #1864 (LouisRiverstone) : elle ne change rien sur le chemin de démarrage (mesuré, commentaire sur #6279).

Toutes les lignes ci-dessus sont **mesurées**. Les causes de la colonne « ce que fait le titre » sont mesurées pour les
six premières (voir la première note) et déduites du site d'appel pour les suivantes.

### 2.1 Contre-mesures sur les pull requests des autres (même machine, même pile)

Nous avons posté des commentaires mesurés, en testeurs, sur les pull requests ci-dessous. Ils ne changent pas le
tableau : ils ajoutent des lancements indépendants. Tous se terminent par les quatre mêmes échecs ctest de la
section 1.

| PR | Résultat | Remarque |
| --- | --- | --- |
| #6935 (Kam471) | mesure des `Flags` de file | les trois files sont créées une fois `Flags &= 0x1` |
| #7006 | réussie, avec une remarque | sans `LEONIDA_FFX_PC=1`, le drapeau d'upscaler n'accroche rien, contrairement au texte de la PR ; le sélecteur n'est jamais atteint sur cette base |
| #6986 | réussie | 154 contrôles de `xgameruntime_test` |
| #8003 et #7896 (Spacecer2) | #8003 réussie, 342 contrôles, `GameInput.dll!#1` se lie à la cale | les deux ajoutent la même ligne de table : une fusion la garde en double, la seconde à passer doit retirer la sienne |
| #8033, #8055, #8043, #8023, #7937 | réussies sur NVIDIA (les contrôles précédents étaient sur AMD ou GTX 1070) | #8033 : rien n'échoue sans le changement, un petit test de démontage le fixerait. #8043 : 601 cas d'état comparés à l'hôte, 0 différence. #7937 : 119 modes d'affichage au slot 21 |
| #7969 (sur #7490) | le commit réussit, la pile échoue `queue_proxy` 3 fois sur 3 | l'échec vient de #7490 (tedin7), comme l'a signalé sgonzalez-glitch |
| #7095 | réussie (`inline_scan` : 18 tests) | voir les limites en section 5.5 |

### 2.2 Bouche-trous locaux, jamais publiés

La troisième passe (boucle de frames) a utilisé des correctifs privés pour trouver l'arrêt suivant. Ils sont listés
pour que personne ne lise les résultats ci-dessous comme l'effet des seules pull requests publiques.

| Bouche-trou | Effet | Statut |
| --- | --- | --- |
| slot 139 de liste répond comme `Reset` | la remise à zéro de liste du titre fonctionne (même idée que #2490) | mesuré |
| mot d'état du slot 133 lu comme une disposition de barrière ; profondeur-pochoir avec UAV créé sans le drapeau UAV ; repli sur une ressource committed quand l'import de pages de #7218 échoue | passe le premier `int3` après le slot 133 | mesuré |
| `LEONIDA_SKIP_INT3=1` | le titre continue après un `int3` de l'image qui suit le slot 25 | mesuré |
| `TF_LENIENT` (ce que visait #7007) | une liste avec un draw invalide est tout de même soumise | mesuré |

## 3. Du premier draw à la boucle de frames

Même machine, natif, backend Vulkan. La base est `main` `dde82f27` de Leonida plus les PR du chemin de démarrage
(#7201, #7323, #7759, #7286, #7290, #7111, #7403, #7417, #7529, #7581, #6891, #7685, #7356, #7880, #7448, #7218,
#7949, #6885, #6981), plus les bouche-trous de la section 2.2. Les lancements sont numérotés dans notre journal ; le
tableau garde ce qui touche au résultat.

| Lancement | Résultat | Durée (s) | Pic |
| --- | --- | --- | --- |
| stubs seuls (pas de shaders traduits) | 4 `PresentX`, puis une violation d'accès de l'hôte (écriture nulle) ; l'arène est pleine sans `LEONIDA_ARENA_MB` | 40 à 110 | 2 à 6 Go |
| avec les shaders traduits | boucle de 300 s ; 53 830 pipelines créés, 602 refusés ; au moins 16 `PresentX` (environ un toutes les 10 s, le reste est de la compilation) ; 0 appel du slot 55 | 307 | 13,4 Go |
| listes exécutées | 0 draw : la liste de frame échoue à `Close`, donc n'est jamais soumise ; fenêtre blanche | 224 | 14 Go |
| listes tolérantes | `Close` : 661 réussis, 0 échec ; 17 lots exécutés avec un clear et deux draws de listes de rectangles | 200 | non relevé |
| avec les contrôles de swap chain | `PresentX` échoue 16 fois sur 16 avec `E_INVALIDARG` (le jeton de frame n'est jamais émis) ; au moins 2 048 pipelines refusés « comme AS » (ni VS ni MS) | 150 | non relevé |
| avec les jetons de frame de #7685 (Cattn) | `PresentX` réussit ; la fenêtre, rendue visible à la main, est noire à 100 % | 160 | non relevé |
| type 24 lu comme VS (local, depuis infirmé, section 6) | refus 602 puis 219 sur deux piles différentes, puis 3 140 puis 219 avec l'option stub, voir la section 6 | 160 | non relevé |

Autres piles, même machine :

| Pile | Résultat | Statut |
| --- | --- | --- |
| main `dde82f27` plus #7201 #7323 #7759 #7286 #7290 #7356 #7880 #7448 #7218 #7949 #6885 #6981, slot 139 de liste en `Reset` | 4 `PresentX` (R8G8B8A8 3840 par 2160, cible de rendu et UAV, double tampon), puis une violation d'accès. L'import par #7218 des tampons du slot 133 sur les pages du titre fonctionne sur NVIDIA mais demande `LEONIDA_ARENA_MB=4095`. Sous `LEONIDA_STUB_PIPELINES`, tous les draws sont refusés (pipeline 0) | mesuré |
| base `rt5-integration` (tibzejoker) plus #7759 plus #8815 | sans slot 57 pris en charge, le titre boucle sur le slot 57 : `CreateLifetimeTracker` refuse plus de 3 600 appels en 5 minutes, 0 appel du slot 55, 0 `PresentX`. Avec #8890 (RipcuBAN : le slot 57 rend `E_INVALIDARG` faute de `fspx_gfx`), la boucle avance : plus de 3 300 `CreatePipelineState` réussis en 5 minutes. Sans la cale `AcpHal` (#6885, #6981 de Jadiac5), une violation d'accès suit `ApuCreateHeap` | mesuré |
| `main` de la forge des mainteneurs `2ac38583` | `gta6rt` compile ; l'étape des shaders tournait encore en fin de journée (1 h 30 estimée). Pas encore de résultat de frame | mesuré (compilation), en attente (lancements) |

Mesuré par d'autres, sur le `main` de la forge une fois le slot 55 en place (#7759) : Yoshi-OOF compte 41 545
dérivations réussies quand les objets de base du slot 57 sont gardés. lines9686-rgb et sh1shu1 font tourner la même
boucle de frames et donnent les deux messages Vulkan de la section 5.6 et l'A/B du mesh shader de la section 5.3. Le
titre dessine sur le `main` de la forge ; les images restent noires (rapport du mainteneur et issue #8851).

### 3.1 Compilation des pipelines

**Mesuré** ([notre commentaire sur Leonida #7231](https://github.com/ItsNotRexed/Leonida/issues/7231#issuecomment-7107629416)) : les 52 pipelines du constructeur BVH du paquet
prennent de 2,41 s à 27,69 s au premier lancement (à froid, environ 25 s), pic 1 091 Mo, puis de 0,3 à 0,7 s ensuite
(moins de 1 s avec le cache de pipelines du pilote chaud), pic 686 à 687 Mo. RTX 4070 SUPER, pilote 610.88. Les 31,9 s
de l'arrêt 18 en section 2 sont le même cas à froid en mode stop. Les 26 503
shaders traduits et les 53 830 pipelines du lancement de 300 s viennent de lancements différents et ne se divisent
pas l'un par l'autre.

## 4. Les quatre causes mesurées des images noires

Natif, NVIDIA, boucle de frames. Chaque cause a été mesurée en l'activant ou la désactivant ; le tableau dit ce qui la
lève et qui l'a écrit.

| # | Cause | Mesure | La lève | Statut |
| --- | --- | --- | --- | --- |
| 1 | `PresentX` échoue avec `E_INVALIDARG` | 16 appels sur 16 échouent tant que le device n'a pas de jeton de frame | #7685 (Cattn) : l'attente `WaitFrameEventX` côté device et le pacer partagent un jeton. Avec elle, `PresentX` réussit | mesuré |
| 2 | La fenêtre n'est jamais visible | `IsWindowVisible` vaut 0 pendant tout le lancement ; le titre ne montre pas sa fenêtre | `TF_SHOW` local seulement ; aucune PR publique identifiée | mesuré |
| 3 | La liste de frame n'atteint jamais la file | un draw ou un dispatch est enregistré sans pipeline, donc `Close` rend `E_INVALIDARG` et la liste n'est pas soumise | #7007 (lines9686-rgb, fermée) avec #7417 (rafaelpssz, `LEONIDA_STUB_PIPELINES`) : en mode tolérant, `Close` réussit 661 fois sur 661 | mesuré |
| 4 | Pipelines refusés à la création | au moins 2 048 flux du slot 86 arrivent avec un sous-objet de type 24, ni VS ni MS, un pixel shader et une topologie triangle ; ils étaient refusés comme shaders d'amplification | lire le type 24 comme le mesh shader (section 5.3, sh1shu1 et lines9686-rgb) | mesuré ; le correctif est celui corrigé après notre mauvaise lecture, section 6 |

Après les causes 1 à 3, un draw de triangle (topologie 4, compte 3, une instance) n'a toujours pas de pipeline : il
lui faut un programme de sommets du titre qui est embarqué et pas encore traduit (#7746 de Faterek). Codex a vérifié
deux traces et trouve le même draw refusé avec pipeline 0 et signature racine 1 ; les mêmes lignes montrent deux draws
enregistrés en topologie 17 (listes de rectangles) et `PresentX` qui rend 0. Cela confirme le symptôme, pas l'identité
du shader ni une cause unique de l'écran noir (lecture statique de Codex, comptes mesurés).

## 5. Constats statiques

Tout ce qui suit est **déduit statiquement**, par Claude (REA 6.10.1, capstone, décompilation Ghidra ciblée) et par
Codex (Ghidra, capstone, un test C++ synthétique), sur notre copie. Les numéros de slot sont l'indice de vtable du
device, de la liste ou de la file console. Quand Codex donne une limite, la limite est gardée.

### 5.1 Un piège dans le test de HRESULT (perte de device)

| Élément | Constat |
| --- | --- |
| Où | une aide commune teste le résultat de 53 sites d'appel après un appel graphique. Ce n'est pas un simple test de signe |
| Quoi | les HRESULT `0x887A0005`, `0x887A0006`, `0x887A0007` et `0x887A0020` (famille DXGI de perte de device : retiré, bloqué, réinitialisé, erreur interne du pilote) mènent à une fonction sans chemin de retour qui se termine par un `int3`. Les autres HRESULT négatifs sont tolérés. Les sites, d'après la forme de leurs arguments, comprennent les slots appelés 50, 132 et 119 dans le style d'un vidage d'incident |
| Lecture | aucun `ret` dans cette fonction, toutes les branches mènent à l'`int3` (capstone). Le rôle « vidage d'incident » vient des seules formes d'arguments |
| Conséquence | une couche qui répond à une erreur de device par l'une de ces quatre valeurs termine le titre. Elle doit journaliser la raison et, là où elle doit échouer, rendre un autre HRESULT négatif. Non décrit publiquement avant. Aucun correctif proposé, comme l'a demandé le coordinateur |
| Statut | déduit statiquement ; 53 appelants est un compte d'appels directs |

### 5.2 Dispositions des descripteurs des slots 55, 57, 86, 108, 70, 127

Les dispositions sont des positions de champs dans les arguments, rapprochées de structures publiques du SDK quand
Codex ou nous avons pu le faire. Rien n'établit qu'un champ de rôle inconnu soit une taille ou un pointeur.

| Slot | Rôle | Forme | Statut |
| --- | --- | --- | --- |
| 55 | `CreateDerivedPipelineState(PSO de base, n, enregistrements, riid, sortie)` | `n` de 1 à 3 dans la lecture statique, 1 à 4 dans #7759 ; enregistrements de 0x14c octets, une étiquette de 32 bits puis une charge. Étiquettes 0 (profondeur-pochoir), 1 (rastériseur), 2 (mélange) et 5 (octets des masques de lecture et d'écriture du pochoir) lues dans les écritures du titre ; l'étiquette 3 (masque d'échantillons, TomBox) est servie et jamais écrite ; l'étiquette 5 est **supposée**, non mesurée à l'exécution | déduit ; supposé pour l'étiquette 5 |
| 57 | `DeserializeGraphicsPipelineStateX` (nom de #8890) | bloc de 0x108 octets : une signature racine au début, deux paires d'objets, puis sept blocs de 0x20 octets `{pointeur A, taille A, pointeur B, taille B}` ; l'indice 0xFFFFFFFF veut dire absent ; le résultat est testé par signe à travers le piège de 5.1. Le premier champ est très probablement une signature racine : elle vient du slot 16 du device avec l'IID PC `ID3D12RootSignature`, créée ou reprise d'un cache. Le rôle des paires est **supposé** (octets et taille de chaque étage est la lecture qui colle) | déduit ; supposé pour les paires |
| 86 | `CreatePipelineState` avec un flux | `{u64 0x2b0, pointeur de flux}`, la forme PC `D3D12_PIPELINE_STATE_STREAM_DESC` (transmise au slot 47 PC dans #7201, justr4mzi). Types de sous-objets 0 à 5 (signature racine, VS, PS, DS, HS, GS) à des positions fixes, puis 8 (mélange), 9, 10, 0x15 (profondeur-pochoir 1), 0x11 (échantillons), 0xc (disposition d'entrée), 0xe, 0xf (formats des cibles), 0x10 (format de profondeur). Forme mesh : le deuxième sous-objet porte le type 0x18 et le cinquième 0x17. Un échec rend silencieusement, sans `int3` (mesuré à l'exécution : le titre garde un pointeur nul). Appelé 6 fois de suite après l'arrêt 21 | déduit ; l'échec silencieux est mesuré |
| 108 | création de ressource committed avec un indice d'alignement | `(retour, device, 1, 1, description de ressource modifiée, info)`. Le titre demande d'abord un alignement 0x1000 quand la taille est sous 0x80008 et `flags & 3 == 0`, puis se replie sur 0 ; un `int3` suit un mot haut non nul de la taille. Déjà décrit publiquement par KkelebekK et justr4mzi | déduit |
| 70 | création d'une vue de ressource shader | le premier argument correspond à un `D3D12_RESOURCE_DESC` de tampon aux champs vérifiés (dimension 1, hauteur 1, profondeur 1, mip 1, disposition ligne par ligne, largeur issue d'un champ de 12 bits fois un compte de 32 bits) ; une branche texture écrit la dimension 3 et les champs de texture. Le second argument correspond à un `D3D12_SHADER_RESOURCE_VIEW_DESC` (40 octets) suivi d'un QWORD console en +0x28, pris dans le conteneur. Rien ne montre que ce QWORD soit une adresse GPU. Une boucle de réécriture de cache suit sur le troisième argument : le troisième est donc de la mémoire que possède le titre. Six appelants directs ; appelants indirects non couverts | déduit ; fortement cohérent |
| 127 | forme `CreateCommittedResource3` | propriétés de tas en cinq DWORD `{DEFAULT, ?, ?, 1, 1}`, drapeaux 8, une description, un mot de disposition initiale, l'IID PC `ID3D12Resource` et une sortie. Le titre teste le résultat par signe ; un échec libère l'objet et rend null à l'appelant qui finit sur l'`int3` de l'arrêt 16 | déduit |
| 35 (file) | `ExecuteCommandLists` (console) | consommé par la boucle de frames, qui lit un anneau d'entrées de 0x50 octets | déduit ; déjà public pour 78, 79, 80 |
| 80, 81 (device) | événements de frame (`(dev, 2, 0xffffffff, 0, 0, &jeton)` et `(dev, 0, 0, 0, 2, &objet)`) ; le slot 81 plante si négatif | le slot 81 est `GetFrameEventsX` dans #8869 | déduit |

### 5.3 Types de sous-objets 23 et 24 : notre erreur et la correction

| Lecture | Qui | Preuve | Statut |
| --- | --- | --- | --- |
| 24 = shader d'amplification, 23 = mesh | nous, au début | le retypage des deuxième et cinquième sous-objets lu comme AS et MS | **faux** |
| 24 = vertex shader, 23 = hull shader | TreXito (#8186, statique), puis nous (#8815) | ces PSO portent un pixel shader et une topologie triangle et aucun sous-objet mesh | **faux pour 24** ; 23 jamais vu à l'exécution |
| **24 = mesh shader console** | sh1shu1 : 430 tailles découpées sur 430 correspondent à des conteneurs mesh. lines9686-rgb : lire 24 comme MS donne 180 s sans exception et plus de 65 536 `PresentX` ; comme VS, le titre meurt à 28 s | A/B sur de vrais shaders, sans stubs | mesuré par d'autres |
| 23 | ouvert | statique : le titre écrit 24 quand deux bits de drapeau (positions 56 et 57) des drapeaux de l'objet shader sont à 1, et n'écrit 23, dans cette branche seulement, que si un second objet shader existe. Cela colle avec AS plus MS et n'exclut pas HS. Les producteurs des deux blocs d'octets sont la même fonction avec le même mode ; rien ici ne montre l'étage. Les scans de corpus de la forge ne trouvent ni HS, ni DS, ni AS parmi 26 503 shaders et n'utilisent jamais les slots 3 à 5 de la bibliothèque, ce qui ne prouve rien sur 23 | ouvert ; demander un vidage à l'exécution des drapeaux de l'objet 23 |

La correction de #8815 (24 devient le mesh shader quand aucun sous-objet mesh PC n'est présent ; le remappage HS est
retiré et 23 reste un type inconnu ; les tests passent dans la partie de `d3d12_mesh_test` qui gère le mesh) est
**en cours** à la rédaction de cette note.

### 5.4 Slots 131 et 132 de la liste, IID de la liste

| Élément | Constat | Statut |
| --- | --- | --- |
| Suite de slots | une fois la liste créée, le titre appelle le slot 132, le slot 131 puis `Close` (slot 9, testé). La boucle recommence avec un autre type d'allocateur. Les résultats de 132 et 131 ne sont pas testés | déduit |
| Valeurs d'arguments | déjà dans [premier-demarrage-1021.md](premier-demarrage-1021.md). Toutes passées par valeur ; rien ne prouve qu'une valeur soit une taille, un pointeur ou un mode | déduit |
| Traitement de #7076 | les slots 131 et 132 sont classés non transmis, journalisés, répondus 0. Cohérent avec l'absence de test, suffisant pour que la création et le `Close` passent ; cela ne valide aucun effet ni aucun rendu | mesuré sur le code de #7076 (Codex) |
| IID | dans tout `.rdata`, un seul GUID a la forme `xxxxxxxx-4024-474c-98cf-645b5?dc4874` (celui de l'arrêt 5). Les paires voisines `55050859` et `8754318e` n'y sont pas (cela n'exclut pas d'autres sections ni une construction à l'exécution). Le même IID sert avec un allocateur de type 0 (première liste) et de type 2 (boucle) : les sites d'appel ne le rattachent donc pas à l'un des quatre noms listés dans Leonida #3520 (calcul, copie, etc.) | déduit ; seuls les 32 premiers bits coïncident avec `ID3D12GraphicsCommandList6` PC, donc « l'IID console est List6 » reste supposé |
| Test indépendant | avec le même device et un allocateur DIRECT, `CreateCommandList` donne `E_NOINTERFACE` pour l'IID console et réussit pour l'IID PC de `ID3D12GraphicsCommandList6`. Substituer l'IID et rendre la liste PC ne suffit pas : la liste PC a 80 slots et le titre appelle 132 | mesuré (Codex, synthétique) |
| Aide de file | l'aide qui prépare les files crée une fence (slot 36 du device, IID de fence PC) et rend faux sans elle ; rien là ne dépend de l'indice de la file ni de ses drapeaux | déduit |

### 5.5 `inline_scan` et les limites d'un scan statique

`tools/inline_scan.py` de Leonida (#7095) balaie 26 674 075 instructions de l'exécutable et classe 110 sites comme des
écritures de liste qui suivent un contrôle de limite, 0 non contrôlé, avec 2 720 stores étiquetés « sans rapport ».
Notre revue a montré que **le compte mesure le classifieur, il ne prouve rien** :

| Contrôle | Résultat | Statut |
| --- | --- | --- |
| écritures de liste gardées parmi les « sans rapport » | 5 vérifiées à la main, toutes gardées : lecture du curseur, comparaison à un champ limite, branche, et un appel de repli (un slot 12, 13 ou une autre méthode) quand le curseur atteint la limite | mesuré à la main (Codex) |
| seconde passe sur les 2 720 | 10 candidats dans 6 fonctions (une lecture de curseur plus des champs de type liste dans 130 instructions). 4 de plus examinés : même motif | déduit |
| un faux positif parmi les 110 | un site « contrôlé » écrit dans une structure locale qui n'est pas une liste D3D12 | mesuré (Codex) |
| étiquettes de sortie | 12 sites « sans rapport » portent aussi l'étiquette `fields` : compter les lignes `fields` donne 24, pas 12 (notre commentaire sur #7095) | mesuré |
| atteignabilité | aucune n'est établie ; un motif statique n'est pas une exécution | énoncé |

La lecture sûre est donc : le titre semble contrôler le curseur avant d'écrire en ligne, et le scan sous-compte.

### 5.6 Les deux messages Vulkan

Lus dans le code de la forge (Codex, tête de forge `2ac38583`), non mesurés sur le titre. Les deux messages viennent
du lancement de lines9686-rgb.

| Message | Sens | Statut |
| --- | --- | --- |
| « a bindless image array is full; dimension 1 » | le nombre est l'énumération interne des dimensions d'image du runtime (0 2D simple, **1 2D-array y compris les cubes convertis**, 2 3D, 3 1D) : ce ne sont donc **pas** les textures 1D du titre. La capacité échantillonnée du binding 2D-array est de 1 024 par défaut, l'entrée 0 réservée à un repli, soit au plus 1 023 utilisables. Les indices sous `storage_capacity` sont réservés aux vues de stockage et une vue de stockage ne cherche que là : le tableau peut être « plein » alors qu'il reste des indices en échantillonnage seul. Aucune inférence sur le nombre de descripteurs que demande le titre | déduit du code ; la demande du titre non mesurée |
| « storage view not supported for depth-stencil image format: 124 » | 124 est `VK_FORMAT_D16_UNORM`, le format Vulkan du runtime, pas une valeur DXGI. Seule `CreateUnorderedAccessView` passe le drapeau de stockage dans cette révision : une création d'UAV a donc atteint une ressource dont le format Vulkan est D16, et la vue a été refusée avant `vkCreateImageView`. Les mots du descripteur sont tout de même recopiés pour une image non multi-échantillon après l'échec : rien n'est enregistré mais le descripteur est écrit. La table de conversion y associe D16 et, pour une ressource de profondeur, `R16_TYPELESS` ; le message ne dit pas lequel | déduit ; format source et appelant à journaliser au point de création de l'UAV |

## 6. Nos erreurs, pour ne pas les refaire

| Erreur | Ce qui s'est passé | Ce que nous faisons maintenant |
| --- | --- | --- |
| `Flags` de file | mesuré le `E_INVALIDARG` du slot 48 et proposé un correctif dans un commentaire de revue sur #6935 ; #6954 et #7037 le couvraient déjà | regarder les pull requests ouvertes et l'issue de coordination avant d'écrire un correctif |
| IID de liste console | ouvert #7124 pour l'IID du slot 73 ; #7076 avait déjà le proxy de liste console. Fermée en sa faveur, les mesures déplacées sur #7076 | idem. Un simple remappage d'IID ne donne pas une liste de 132 slots |
| Prédiction prise pour un arrêt | la première note prédisait `XErrorReport` comme premier arrêt : non reproduit dans le bisect (la base le dépassait déjà) | étiqueter les prédictions comme telles, les séparer des mesures |
| Code de sortie pris pour un arrêt | 127 ou 3 en moins d'une seconde sans événement dans la trace ressemblait à une sortie inexpliquée | un `int3` de l'image n'est pas un événement tracé ; ajouter d'abord un gestionnaire d'exception qui journalise (travail W12) |
| « 602 », puis « 602 puis 219 », puis « 3 140 puis 219 » | les premiers chiffres comparaient deux piles locales différentes. Le chiffre corrigé (même pile, un commit changé, 57 423 appels dans les deux lancements) était 3 140 refusés puis 219, créés 54 282 puis 57 203. Mais **il a été mesuré avec `LEONIDA_STUB_PIPELINES=1`** : les pipelines étaient acceptés alors que leur programme de sommets n'était jamais trouvé. Le chiffre ne soutient pas la lecture VS | publier la pile et les options avec chaque compte ; ne jamais comparer des lancements de piles différentes ; mesurer sans l'option stub quand l'affirmation porte sur la création |
| Type 24 lu comme VS | après les lectures AS et MS, puis HS et VS, #8815 a rapproché 24 et 23 de VS et HS. sh1shu1 et lines9686-rgb ont montré que 24 est le mesh shader | refonte de #8815 en cours ; 23 reste inconnu tant qu'il n'est pas vu à l'exécution |
| #6279 | restée ouverte alors que #1864 sur main produisait déjà les mêmes octets | après un changement sur main, remesurer et fermer une PR devenue redondante |
| Travail sur le slot 55 en recouvrement | #8869 servait aussi le slot 55 | nous avons gardé le slot 55 dans #7759 et proposé l'autre PR pour le slot 81, aux mainteneurs de trancher |

## 7. Ce que cela implique pour AnyXboxX

Depuis la PR #109, le proxy de TomBox est servi par défaut (`runtime/build/tombox`) et fournit `d3d12_xs`, `xmem` et
`XCRDAPI`. Tout ce qui précède dit ce que ce proxy doit encore servir, d'après les slots du démarrage chez Leonida.
Les colonnes « TomBox » viennent de [tombox.md](tombox.md) et n'ont pas été lancées contre le titre.

| Besoin vu sur le titre | Ce que TomBox a | Écart, d'après cette note | Statut |
| --- | --- | --- | --- |
| slots 131, 132 de liste | table de 110 slots (son `vt` en a 128), donc 131 et 132 hors limites | journaliser les arguments et rendre `E_NOTIMPL`, jamais 0 (le titre les ignore) ; demande le proxy de liste console | écart mesuré sur la table ; rôle des slots inconnu |
| IID de liste | l'IID console figure dans la liste de GUID de TomBox | adapter l'IID ; le vérifier pour les types d'allocateur 0 et 2 | supposé |
| slot 55 | `create_derived_pso_x`, étiquettes 0 à 3 | étiquette 5 (masques de pochoir) refusée ; notre #7759 la lit dans les écritures du titre | étiquette 5 supposée |
| slot 57 | `DeserializeGraphicsPipelineState` assigné | la forme du titre (signature racine, sept blocs, 0xFFFFFFFF absent) n'est pas comparée à lui | à vérifier |
| slot 86 | non assigné | flux de 0x2b0 octets avec des sous-objets console, type 24 comme mesh shader, 23 inconnu ; un échec silencieux laisse un PSO nul | écart mesuré |
| slots 70, 25, 127, 133 | slot 127 hors de la table de 90 slots du device ; les autres non comparés | formes ci-dessus ; 127 demande d'étendre la table | écart mesuré |
| perte de device | aucun | le piège de 5.1 : ne jamais rendre les quatre HRESULT | déduit |
| `XGSuggestSwizzleMode` | aucun | toujours bloqué ; #6891 (Jadiac5, fusionnée) sert les deux fonctions chez Leonida | inchangé |
| audio (`AcpHal`, `chad`, `xapu`) | cinq fonctions `xapu` seulement | le titre plante après `ApuCreateHeap` sans `AcpHal` (#6885, #6981) | mesuré |
| rythme des frames | non comparé | `PresentX` demande un jeton de frame émis par l'attente côté device (#7685) | mesuré chez Leonida |

Ordre de travail issu de cette note, chaque point testable sans le jeu (règles 1 et 4) :

1. un test synthétique du piège de HRESULT : les chemins d'échec du proxy ne rendent jamais les quatre valeurs ;
2. des listes de commandes console dans le proxy, avec les slots 131 et 132 en no-op journalisés qui rendent `E_NOTIMPL` ;
3. un flux synthétique pour le slot 86 avec le type 24 comme mesh shader, et pour le slot 55 avec l'étiquette 5 ;
4. comparer la forme du slot 57 à `DeserializeGraphicsPipelineState` de TomBox, par un test unitaire construit à partir des positions de champs de la section 5.2 ;
5. tenir dans ce dossier un tableau des PR de la boucle de frames et de leur pile, mis à jour à chaque passe, pour qu'un résultat soit toujours lié à sa base.

## Sources et crédits

- Mesures : lancements sur la machine du mainteneur, par Claude ; analyse statique par Claude (REA, capstone, Ghidra
  ciblé) et par Codex (Ghidra, capstone, un test C++ indépendant, lecture du code de Leonida). Rapports et journaux
  restent hors du dépôt.
- Pull requests et issues de Leonida, par auteur comme dans les tableaux : undeadhuman37, Stalkerino, labj1987,
  NiiRoZz, Kam471, mariuslozo-boop, sennecools, Pipodi, NolanCotter, Khuraie, tedin7, mrp1no, zzEmmazz, KkelebekK,
  LouisRiverstone, justr4mzi, Cattn, lines9686-rgb, rafaelpssz, Faterek, RipcuBAN, Jadiac5, tibzejoker, Spacecer2,
  alexander-src, DevilNine, Vadson159, TreXito, Krashgamer, gookie-dev. Les nôtres : #7759 (slot 55, intégrée au
  `main` de la forge, validée par Yoshi-OOF), #8815 (type 24, en correction), #6279, #7124 (fermée). Nos commentaires
  mesurés sont sur #6935, #6986, #7006, #7021, #7076, #7095, #7110, #7201, #7235, #5680, #6279, #7937, #7969, #8003,
  #8023, #8033, #8043, #8055, #8869 et #8815. Coordination : #6910, #7049, #8186, #8851.
- Mesures d'autres : Yoshi-OOF (dérivations du slot 55 sur le `main` de la forge), sh1shu1 et lines9686-rgb (A/B du
  mesh shader, messages Vulkan), dracbat (a signalé le correctif du jeton de frame ; la PR est #7685 de Cattn),
  KkelebekK (échelle publique des arrêts).
