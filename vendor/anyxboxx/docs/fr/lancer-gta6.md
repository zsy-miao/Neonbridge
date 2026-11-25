> English: [running-gta6.md](../running-gta6.md)

# Lancer GTA VI (build Xbox Series 1.0.2.1) avec AnyXboxX

Ce guide explique comment installer et lancer le projet, et tout ce qu'il faut installer pour refaire nos essais sur le build Xbox Series 1.0.2.1 de GTA VI, jusqu'au point où nous en sommes : la première cinématique. Il ne fournit pas le jeu et n'explique pas comment l'obtenir : vous apportez votre propre copie. Ne mettez jamais de fichier du jeu dans ce dépôt et ne les joignez à aucune issue.

Le lancement passe par [VIctory](https://github.com/Marvisak/VIctory) (Marvisak, GPL-3.0 ou ultérieure), copié dans ce dépôt sous `third_party/victory`, où il est notre code : rien n'est synchronisé ni épinglé. La copie part de la branche `anyxboxx/fork-7b0-lot9-murs` du fork du mainteneur [LooperSalty/victory](https://github.com/LooperSalty/victory), puis a reçu 57 commits pour Proton et les cartes AMD (voir plus bas). Cette branche assemble le fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (GPL-3.0) au commit `7b0f736`, huit de ses pull requests précédentes, sept nouvelles, le portage de la précompilation hors ligne de #282 et des commits à nous (disposition `xg_xs.dll` servie comme la bibliothèque de Microsoft, voir plus bas), au commit `80d58f8`. Le script `tools/lancer_victory.ps1` le compile et le lance sur votre dossier du jeu.

## Base Neptune (25 novembre, soir)

`third_party/victory` contient désormais la source correspondante de Neptune (VIctory Edit.) 2026-11-25c (GPL-3.0-or-later ; elle repose sur `fxceless/victory-fork`, `myokex/victory-amd`, `soldsawn/victory-nvidia-8gb`, ShakiraRED-dt, `ZSlayerHQ/victory-world-runtime`, `JoshuuDev/victory-amd` et d'autres, voir [THIRD_PARTY.md](../../THIRD_PARTY.md)), plus nos quatre commits `xg:` (la `xg_xs.dll` de Microsoft comme oracle, `-XgDll`) et la Rich Presence Discord éteinte sauf `VICTORY_DISCORD=1`. Les sections suivantes décrivent la base précédente `80d58f8` et ses 57 commits Proton/AMD, qui ne sont pas repris.

```powershell
.\tools\lancer_victory.ps1 -GameDir <dossier du jeu> -Warm   # la première fois : environ 7 min de traduction des shaders, puis le jeu
.\tools\lancer_victory.ps1 -GameDir <dossier du jeu>         # ensuite
```

Le lanceur compile le profil release (`-Diag` ou `-Dred` pour la build de diagnostic) et pose le jeu de réglages du `play.ps1` de Neptune pour le chemin Series X (carte de 16 Go : baisser `VICTORY_CARD_WINDOWS_MB` et `VICTORY_CARD_BUFFERS_MB` sur une carte plus petite). `-Warm` lance `victory.exe warm` sur les seuls fichiers `.rpf` du dossier du jeu. `-AutoA` appuie sur A pendant les écrans de démarrage, `-FrametimeDump` écrit un CSV des temps d'image dans le dossier du cache. En jeu, F10 masque les chiffres du coin et F12 enregistre une image dans Images.

Mesuré par le mainteneur le 25 novembre (RTX 4070 SUPER, i7-13700KF, chemin Series X en 4K, monde, après `warm`, build release) : médiane d'environ 12,5 fps, environ 15 fps sur la dernière minute, pointes à 30, environ 7 500 draws par image, sans plantage ; la base précédente tournait à environ 5 fps au même endroit. Problèmes connus (notes de Neptune) : objets et vêtements noirs un instant pendant le remplacement de leurs textures, parties du monde absentes derrière certaines portes et sur certains toits, pas d'animation faciale, sauts du temps d'image pendant le streaming ; la lumière intérieure est une lumière ambiante uniforme à la place de l'éclairage en lancer de rayons.

## Correctifs Proton et AMD (25 novembre)

`third_party/victory` n'est plus un sous-module : il contient une copie de VIctory à l'épinglage précédent `80d58f8` (section suivante, inchangée), suivie dans l'historique de ce dépôt de 57 commits pour Proton et les cartes AMD (`git log -- third_party/victory`). Chaque commit nomme sa source ; un commit repris de quelqu'un d'autre se termine par les lignes `Source:` et `Co-authored-by:`. Chaque dépôt d'origine est listé dans [THIRD_PARTY.md](../../THIRD_PARTY.md).

Une copie existante qui a le sous-module doit le retirer avant le pull, sinon le checkout s'arrête sur les fichiers déjà présents :

```powershell
git submodule deinit -f third_party/victory
Remove-Item -Recurse -Force third_party\victory, .git\modules\third_party\victory
git pull
```

Sous Linux, `rm -rf third_party/victory .git/modules/third_party/victory` remplace `Remove-Item`. Cela supprime aussi `third_party/victory/target` : la première compilation repart de zéro. La clé du cache des shaders dépend de la source du traducteur : un cache rempli avec `-Precompile` sur `80d58f8` est rempli de nouveau.

Ce qu'apportent les 57 commits :

- **Proton (Wine)** : le HLSL interne du runtime (clamp des arguments, comparaison sur la carte, passe du plan de profondeur) compile avec le d3dcompiler de Wine, signatures racines construites en Rust (fork #127, BitWuehler) ; une bibliothèque de pipelines est créée sous vkd3d-proton (Marvisak/VIctory #1722, Impre-visible) et la liste des pipelines précoces est gardée sans elle ; beaucoup moins d'appels à `GetWriteWatch`, `VirtualQuery` et `VirtualProtect` par image ; les gardes restent ouvertes pendant les écritures noyau du runtime dans la mémoire du titre, et une faute sous le verrou des gardes ne bloque plus ; les lectures DirectStorage brutes et `fread` arrivent dans un tampon puis sont recopiées en mode utilisateur (JoshuuDev/victory-amd, Joshua Odom).
- **AMD (hôtes à 64 lanes, tas de vues de 2^21)** : les programmes de pixels wave32 sur un hôte à 64 lanes calculent ballots et réductions par demi-vague (PastiiJay/klbf-lab, PastiiJay) ; indices de vues bornés à la dernière case, nulle, du tas (fork #134, triippywhale, et #98, sillynocodecat) ; comparaisons d'entiers 64 bits sur leurs deux mots (fork #135, alexdeliu) ; sommes f16 sur une vague où toutes les lanes ne tournent pas, sur une vague de 64.
- **Shaders** : `v_cvt_f32_f64` et sommes `v_add_f64` (fork #9, MartinIndra02), `v_permlane16_b32` et `v_permlanex16_b32` (fork #134, triippywhale), indices de sommets des primitives de maillage masqués à 9 bits (Marvisak/VIctory #1718, tjthebigbearcat-hub), `v_interp_p2_f16` avec `op_sel`, `omod` et clamp (Marvisak/VIctory #1696, Calvix2k), entrées de pixels interpolées comme le déclare le PSV0 du conteneur console (fork #150, wildheathen), garde NaN sur les exports de couleur et les écritures typées (PastiiJay/klbf-lab, PastiiJay), passes de nettoyage sur chaque module traduit (commit `1684989` du fork, fxceless).
- **Vues mémoire et textures** : la garde des textures voit les écritures par toutes les vues de ses pages (fork #115, Spexivus ; correction de joroyt via myokex) ; enregistrements du tas et vues de tampon faits avec les octets recopiés à chaque soumission (portage de Marvisak/VIctory `86a5ed6b`, Marvisak) ; vues de textures placées en accès non ordonné recopiées sur des textures hôte (fork #114, Kaivyyy) ; les attentes tardives attendent aussi les écritures par alias et les tampons de transfert en vol (fork #191, gdawid9407, #206, AscilCH, #141, adritoyalser) ; copies propres des vues tenues dans un budget ; vues de textures placées distinguées par la mémoire de leurs mips, celles perdues remises dans la file, longues files de textures chargées plus vite pendant un temps borné, dispatchs de plus de 65 535 groupes faits en plusieurs parts, vues de tampons constants limitées à 64 Kio, tableaux de pages DirectStorage lus au moment de la requête (myokex/victory-amd, myokex).
- **Pipelines** : un pipeline encore en création n'est attendu que dans la première image qui le demande ; attentes de pipelines pendant le chargement d'un monde bornées par lancement, programmes de résolution du ray tracing non créés tant que le constructeur de structures est coupé, liste des pipelines précoces fusionnée d'un lancement à l'autre (myokex).
- **Diagnostics et tests** : compteurs par image, interrupteurs A/B, magenta pour les textures manquantes, test du pipeline de maillage ignoré sans mesh shaders (fork #14, oddiums).

Variables lues par ce code. `tools/lancer_victory.ps1` n'en pose aucune : un lancement prend les valeurs par défaut.

| Variable | Défaut | Rôle |
| --- | --- | --- |
| `VICTORY_SHADER_OPT` | toutes les passes | Passes de nettoyage des shaders traduits ; `0` les coupe. Fait partie de la clé du cache. |
| `VICTORY_PS_HALVES` | active | Ballots et réductions par demi-vague, seulement sur un hôte dont les vagues de pixels ont 64 lanes (AMD) ; `0` coupe. |
| `VICTORY_L6_HEAP_CLAMP` | active | Tas de vues de 2^21 et indices bornés, seulement sur un hôte qui accepte ces tas ; `0` coupe. |
| `VICTORY_L6_NAN_GUARD`, `VICTORY_L6_PSV_INTERP`, `VICTORY_L6_PLACED_UAV`, `VICTORY_L6_NULL_LIBRARY`, `VICTORY_L6_PLANE_PASS` | actives | Garde NaN, interpolation PSV0, vues de textures placées en accès non ordonné, bibliothèque de pipelines à blob nul, passe du plan de profondeur ; `0` en coupe une. |
| `VICTORY_L7_<NOM>` (`DYNAMIC_SLOTS`, `CB_SLOTS`, `VIEW_COPY_LRU`, `VIEW_ADDRESS`, `TEXTURE_SLOTS`), `VICTORY_L7_VIEW_COPY_MB` | actives, `512` | Cases et copies des vues ; `0` en coupe une. |
| `VICTORY_L9_DSTORAGE_GUARDS`, `VICTORY_L9_GUARD_TITLE_VIEWS` | actives | Gardes ouvertes pendant les écritures noyau ; les gardes n'arment que les vues du titre. |
| `VICTORY_TEXTURE_SWEEP` | active | Vues de textures placées perdues remises dans la file ; `0` coupe, `count` compte seulement. |
| `VICTORY_TEXTURE_GATE_S`, `VICTORY_PIPELINE_GATE_S` | `60`, `120` | Secondes par lancement passées à charger une longue file de textures, ou à attendre des pipelines pendant le chargement d'un monde ; `0` jamais. |
| `VICTORY_RT_RESOLVE` | non posée | Les programmes de résolution du ray tracing ne sont créés qu'avec `1` (ou `VICTORY_BVH_BUILDER=1`). |
| `VICTORY_L9_ALIAS_UNCLEAN`, `VICTORY_HEAP_RECHECK` | coupées | `1` en active une. Coupées exprès : sous Linux, le lancement 28 (les deux actives) a perdu la géométrie des visages et le lancement 30 (`HEAP_RECHECK` active) a noirci la peau des visages (observé). |
| `VICTORY_L7_ROLES`, `VICTORY_L7_SKIP`, `VICTORY_L7_UNLIT` | coupées | Diagnostic : draws sautés selon le rôle du shader. La table des rôles se construit en local depuis votre copie et n'entre jamais dans ce dépôt. |
| `VICTORY_NULL_MAGENTA`, `VICTORY_CAPTURE_MENUS` | coupées | Diagnostic : magenta pour les textures manquantes ; captures dans les menus. |

Mesuré sous Linux seulement, avec ce code (compilé comme `890a83a` avant la copie, même arbre ; Fedora 44, RX 9070 XT 16 Go, RADV via vkd3d-proton, GE-Proton11-12 avec un correctif communautaire « view-tree » du wineserver, journal VIctory du lancement 31, environ 23 min depuis le lancement, cache des shaders préparé hors ligne avec `victory translate --cache`) :

- Monde : image médiane du monde de 266 ms, environ 3,7 i/s. Mesuré.
- Aucun plantage, aucune perte du périphérique, aucun échec DirectStorage, aucune ligne `ERROR` dans le journal. Mesuré.
- Visages, peau et géométrie complets, textures stables. Observé.
- Les intérieurs et les zones à l'ombre sont noirs : le ray tracing ne tourne pas (le constructeur de structures est coupé et le lancement saute les passes de ray tracing, de GI et de reflets). Observé.
- Avant, sur Proton sans correctif, le lot 6 tournait à 685 ms d'image médiane ; avec le correctif du wineserver et les réductions de write-watch de cette branche, le lancement 14 (un lot local qui portait déjà la plupart de ces commits) tournait à 274 ms. Mesuré.

Réglages de ce lancement (non posés par le lanceur ; Linux seulement) :

| Variable | Valeur | Pourquoi |
| --- | --- | --- |
| `VICTORY_LATE_WAIT`, `VICTORY_WIDE_BACK` | `0`, `0` | Avec les attentes tardives, le périphérique est perdu sur RADV (issue #184 du fork). |
| `VICTORY_SHADER_LOOP_LIMIT` | `256` | Comme le lanceur. |
| `VICTORY_PIPELINE_WAIT_MS` | `50` | Attente des pipelines côté hôte par image. |
| `VICTORY_TEXTURE_BUDGET_MB` | `8192` | Budget de textures pour une carte de 16 Go. |
| `VICTORY_TEXTURE_NO_WAIT` | `1` | Les rafraîchissements de textures n'attendent pas la mémoire du titre. |
| `VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_CARD_BUFFERS_MB` | `1`, `1024` | Comme le lanceur. |
| `VKD3D_CONFIG` | `descriptor_heap` | Le titre demande 1 245 184 vues, plus que le plafond par défaut de vkd3d-proton. |
| `VICTORY_SKIP_PIPELINES` | 9 empreintes de shaders | `7f05ddab771799a9`, `a53a7977fe762268`, `be14e72d86362031`, `32ed072b9f845048`, `a1b20bc47e3c9597`, `17829ac71aab4e8d`, `e8c962358d862e3f`, `559d9f0184e15fe3`, `afeea48bbb9913ad`. |
| `VICTORY_L7_SKIP` | `rt,gi,reflections` | Avec une table des rôles locale (`VICTORY_L7_ROLES`) : ces passes coûtent du temps et ne dessinent rien tant que le ray tracing ne tourne pas. |

Non mesuré : Windows. Les valeurs Windows du lanceur (`VICTORY_LATE_WAIT=3`, `VICTORY_WIDE_BACK=1`) ne changent pas et n'ont pas tourné avec ces 57 commits ; la plupart des nouveaux comportements sont actifs par défaut et valent pour tous les hôtes. La cible Windows compile sans avertissement (`cargo build --profile diag -p victory --features diagnostics`, compilation croisée sous Linux) et les tests de `victory-gpu` passent sous Proton (158 réussis, 2 ignorés).

## Épinglage précédent : lot 8 plus la disposition xg, la précompilation hors ligne et sept pull requests du fork (25 novembre)

`third_party/victory` pointait sur la branche `anyxboxx/fork-7b0-lot9-murs` de [LooperSalty/victory](https://github.com/LooperSalty/victory), au commit `80d58f8fe2769d6c857eb27abc159b6cf64530a3` : le lot 8 (plus bas) plus 15 commits, listés par `git log --oneline 19feca8..80d58f8` de la branche :

Depuis `80d58f8` (branche `anyxboxx/fork-7b0-lot9-murs`), l'épinglage porte aussi deux commits de Yorivel venus du fork (`e49864a`, `bf870de`, issue #62 du fork) : les écritures DirectStorage dans des pages physiques sont signalées au côté GPU quand `VICTORY_DSTORAGE_NOTIFY=1`, que le lanceur pose. Mesuré par le mainteneur (journal VIctory, 25 novembre, vers 02:41 à 02:50, avec les réglages du lanceur ci-dessus) : d'une sauvegarde neuve jusque dans le monde en environ 9 minutes, 5,3 à 6,9 fps, 4 200 à 6 300 draws par image, pas de perte du périphérique ; cinématiques entières, jugé « incroyable » (observé) ; des graphismes manquent encore par endroits. La plupart des draws abandonnés viennent du constructeur des structures de ray tracing, coupé (14 943 sur 15 444 dans un journal ; mesuré).

- Trois commits à nous, `xg:` (`1f0fe1d`, puis les versions reprises des commits `4466d2e` et `a16f040` du fork) : le HLE de `xg_xs.dll` répond maintenant exactement comme la DLL de Microsoft (mips de queue en mode parent avec position ; tuilage 64 Kio indexé par sous-ressource, mips empaquetés à zéro, compte = mips standard dans la place donnée). Un test de `victory-hle` compare 54 descriptions avec la vraie DLL quand `VICTORY_XG_DLL` est posé. Mesuré en jeu par le mainteneur : 0 écart avec la DLL. La DLL n'est donc plus nécessaire pour ce gain ; le lanceur garde `-XgDll` comme oracle.
- Précompilation hors ligne, portage de la pull request #282 de `fxceless/victory-fork` (t4l03) : `victory translate --cache --pipelines [--used-pipelines] <shaders.rpf> <exe> <dlls>` traduit les shaders et construit les pipelines avant le jeu. La clé du cache ne dépend plus de l'exécutable. Un commit d'adaptation à nous retire `shader_opt` de la clé (le commit `1684989` du fork n'est pas dans notre base). Le lanceur la lance avec `-Precompile`.
- Sept pull requests du fork, reprises avec `cherry-pick -x` (auteurs dans [THIRD_PARTY.md](../../THIRD_PARTY.md)) : #349, #355, #396, #386, #358 (2 commits), #353, #374, plus un commit d'adaptation des tests de #353.

| Pull request du fork | Auteur | Effet |
| --- | --- | --- |
| #349 | AscilCH | `v_div_fixup_f32` : `0.0/x` et `x/Inf` ne donnent plus l'infini |
| #355 | AscilCH | Drapeau signé passé à `wavePrefixOp.f32`, test NaN entier dans `v_cvt_{u16,i16}_f16` |
| #396 | JordanFableFur | Le f16 du repli en rectangle arrondit au pair et garde les NaN |
| #386 | JordanFableFur | Un sampler utilisé avec et sans comparaison reçoit deux places |
| #358 | HailLucifer666 | Rendre exactement les pages verrouillées par un placement ; aucun placement sur place sous les attentes tardives |
| #353 | AscilCH | Décalages de texel Z des MIMG 3D, troncature du décalage a16, LOD de `sample_d` sur cubemap |
| #374 | davutsimsekk | BCPack : garder les 32 indices dans la première entrée du dictionnaire BC7 |

Mesuré ou observé par le mainteneur (Windows 11, RTX 4070 Super, journaux VIctory, nuit du 24 au 25 novembre), réglages du lanceur de `main` :

- `-XgDll` en mode `compare` avec le nouveau HLE : 0 écart avec la DLL de Microsoft. Mesuré.
- Précompilation (`victory translate --cache --pipelines`) : 25 689 shaders traduits en environ 4,5 min, 296 refusés, cache de 13 Go dans `%LOCALAPPDATA%\victory\cache`. Mesuré.
- Monde : 4,8 à 5,1 fps, environ 12,7 % des draws abandonnés par image (11 à 14 % avant). Mesuré. La cinématique est jugée meilleure par le mainteneur, rien de pire en jeu. Observé.
- Analyse de Codex (lue dans les diffs) : #374 (BC7) et #349 (`0/x` donnait l'infini) sont les meilleures pistes pour les carrés corrompus et l'exposition. Pas encore vérifié en jeu.

Pourquoi des objets et des murs manquent dans le monde, trouvé avec `VICTORY_D3D12_DEBUG=1` (mesuré) : 15 des 16 pipelines refusés par le pilote (`0x80070057`) sont des compute shaders wave64 que NVIDIA refuse (« Shader wants wave lane count of 64 but maximum wave lane count for device is 32 ») ; le 16e est un pixel shader qui écrit un entier dans une cible R8_UINT avec mélange. Environ 11 à 14 % des draws de chaque image du monde sont abandonnés faute de pipeline, dans tous les essais. Correctif candidat : Marvisak/VIctory #1890 (wave64 scindé en deux waves de 32), en cours de portage ; suivi dans l'issue « compute shaders wave64 refusés sur NVIDIA ».

## Épinglage précédent : lot 8 plus la bibliothèque de disposition des textures de Microsoft (25 novembre)

`third_party/victory` pointait sur la branche `anyxboxx/fork-7b0-lot8-xg` de [LooperSalty/victory](https://github.com/LooperSalty/victory), au commit `19feca8650c934913063076b612e87739d8d1912` : le lot 8 (plus bas) plus un commit à nous, `xg: serve or check the title from Microsoft's own xg_xs.dll (local copy)`. Sans `VICTORY_XG_DLL`, le code tourne exactement comme le lot 8.

Le jeu demande à la bibliothèque de disposition des textures de la console (`xg_xs.dll` : `XGSuggestSwizzleMode`, `XGCreateTextureComputer`) où se trouvent chaque mip et chaque tranche. VIctory répond avec sa propre réimplémentation. Le GDK fournit une version PC hors ligne de cette bibliothèque ; avec votre propre copie, `tools/lancer_victory.ps1 -XgDll <chemin de xg_xs.dll>` la laisse répondre à la place. La DLL est un fichier du GDK : elle n'entre jamais dans ce dépôt, le lanceur ne transmet que son chemin.

Mesuré ou observé par le mainteneur (Windows 11, RTX 4070 Super, journaux VIctory, 25 novembre, vers 00:37 et 00:42), réglages du lanceur de `main` :

- `-XgMode compare` (dispositions de VIctory, chaque écart journalisé une fois) : 84 descriptions de textures diffèrent, toutes des textures 64 Kio D (`layout` `0x10a`, quelques `0x105`). La bibliothèque annonce les mips de queue dans le mode parent (10) avec une position dans la queue ; VIctory annonce `256B_D` (2) sans position. Tailles, offsets et pas identiques. Le tuilage 64 Kio liste toutes les tranches d'un tableau avec queue de mips (VIctory s'arrête à deux). Aucun écart sur les modes suggérés ni sur les octets tuilés des copies CPU. Mesuré.
- `-XgMode real` (par défaut avec `-XgDll`) : 5,4 à 7,1 fps en fin d'essai contre 5,1 à 5,6, environ 6 080 draws par image, pas de perte du périphérique en environ 3 min 40 ; jugé par le mainteneur comme le meilleur essai à ce jour. Mesuré, puis observé.

## Épinglage précédent : lot 5 plus une pull request (25 novembre)

`third_party/victory` pointait sur la branche `anyxboxx/fork-7b0-lot8` du fork VIctory du mainteneur [LooperSalty/victory](https://github.com/LooperSalty/victory), au commit `d138351de32f860ab9df1aa1aa736fce0a40365e`. C'est le lot 5 (plus bas) plus une pull request du fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork), reprise avec `cherry-pick -x` :

| Pull request du fork | Auteur | Effet | Commit |
| --- | --- | --- | --- |
| n°11 | cxchhh | Shaders : les paires de masques de lanes parqués à quelques instructions d'écart sont restaurées ; les lectures VGPR relatives gardent les lanes parqués | `d138351` (commit du fork `90228a7`) |

Pourquoi : une capture PIX d'une image de cinématique montre que `IrradianceCache_Shade` et `IrradianceCache_ShadeSecondary` ne tournaient pas au lot 5 (0 dispatch). Leurs deux shaders ne se traduisaient pas (« a wave reduction where not every lane runs ») : le cache d'irradiance n'était donc jamais éclairé, et tout ce qui est hors de la lumière directe restait noir (côté ombré des visages, intérieurs). Avec le n°11, les deux shaders se traduisent et passent la validation DXIL, et leurs pipelines sont créés et liés en jeu.

Mesuré ou observé par le mainteneur (Windows 11, RTX 4070 Super), avec les réglages du lanceur de `main` (`tools/lancer_victory.ps1`), d'après le journal VIctory et une capture PIX (essai du 25 novembre, vers 00:01 à 00:05) :

- Lot 5 : capture PIX, 0 dispatch pour les deux passes du cache d'irradiance. Mesuré.
- Lot 8 : les deux shaders se traduisent et valident, pipelines créés et liés en jeu. Observé.
- Shaders non traduits : 6 au lieu de 9. Pipelines refusés par le pilote : 16, comme au lot 5. Mesuré.
- 4,8 à 5,4 i/s, environ 6 550 draws par image, aucune perte du périphérique en environ 4 min 20. Mesuré.
- Avis du mainteneur : « cette version est très bien ».

Limites. L'issue n°75 du fork signale environ 80 refus de pipeline de plus sur une RTX 3070 : cela n'a pas été observé sur la 4070 Super, attendez-vous donc à plus de refus sur d'autres cartes. L'éclairage par ray tracing est toujours absent (le constructeur BVH reste coupé, environ 350 dispatches jetés par image). Avec le réglage `LATE_WAIT` du lanceur, certains murs de la première salle peuvent être transparents (issue n°168 du fork, correctifs en cours : n°191, n°206). La tenue longue n'est mesurée que sur environ 4 min 20.

## Branche précédente : lot 5, fork plus sept pull requests (24 novembre, nuit)

`third_party/victory` pointait sur la branche `anyxboxx/fork-7b0-lot5` du fork VIctory du mainteneur [LooperSalty/victory](https://github.com/LooperSalty/victory), au commit `fd021bf26a7003749d668e5d9bf6b234bfab2e47`. Cette branche assemble le fork [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (GPL-3.0, anciennement `fxceless/victory-gta6`) au commit `7b0f736` et sept de ses pull requests encore ouvertes, reprises avec `cherry-pick -x` :

| Pull request du fork | Auteur | Effet | Commit |
| --- | --- | --- | --- |
| n°63 | arminkhaleghirod | Les textures modifiées récemment sont hachées à chaque contrôle (corrige le clignotement du démarrage) | `d9770b9` |
| n°71 | MartinIndra02 | File COPY de D3D12 pour les lectures de copie de cartes du relais, `CARD_COMPARE` en parallèle | `5e758c7` |
| n°32 | arminkhaleghirod | RGBA flottant 16 bits à la place des cibles de rendu R9G9B9E5 | `d214807`, `0e2c46a` |
| n°82 | Kaivyyy | Test de bornes de profondeur transmis aux pipelines et listes de commandes de l'hôte | `35cdf6f` |
| n°83 | ryotdev | Garde les branches à EXEC vide qui peuvent réactiver des lanes | `194c736` |
| n°81 | undeadhuman37 | Un descripteur lu seulement comme données de huit dwords est une image mesurée | `6119c21` |
| n°77 | m335671 | `v_fma_mixlo/mixhi` sur une première moitié d'interpolation f16 garde zéro | `fd021bf` |

Mesuré ou observé par le mainteneur (Windows 11, RTX 4070 Super), d'après le journal VIctory `victory-fork-lot5.log`, avec les réglages du lanceur de `main` (PR n°172 : sans DRED, boucles 256, `VICTORY_LATE_WAIT=3`, `VICTORY_WIDE_BACK=1`, tampons de cartes) et **sans** `VICTORY_RELAY` :

- Démarrage : sans clignotement. Observé.
- Monde après 2 min : 5,4 i/s et 6 449 draws par image (référence `main` : environ 5,5 i/s, environ 4 410 draws ; lot 4 avec `VICTORY_RELAY=2` : 6,6 i/s, 5 986 draws) ; 363 draws perdus faute de pipeline ; 9 shaders non traduits (13 au lot 4) ; 16 pipelines refusés par le pilote (6 au lot 4) ; aucune perte du périphérique en environ 2 min 30. Mesuré.
- Avis du mainteneur : « beaucoup mieux », la meilleure lumière et les meilleurs graphismes à ce jour. Des problèmes graphiques demeurent sur le visage de Lucia.

Le lanceur ne pose plus `VICTORY_RELAY` : l'essai jugé le meilleur l'avait coupé, et l'issue n°89 du fork rapporte qu'il fait disparaître la peau des personnages et superpose les sous-titres. Limites connues : ray tracing absent, shaders encore non traduits, tenue longue non mesurée (notre essai a duré environ 2 min 30). Avancée du 24 novembre, d'après les journaux VIctory : `d279e69` environ 4 i/s (13 min dans la prison puis périphérique perdu) → `7b0f736` + `VICTORY_RELAY=2` environ 4,5 i/s → réglages de la PR n°172 environ 5,5 i/s → lot 4 environ 6,6 i/s avec `VICTORY_RELAY=2`, clignotement du démarrage corrigé → cette branche environ 5,4 i/s avec plus de draws et la meilleure image à ce jour. Les auteurs sont crédités dans [THIRD_PARTY.md](../../THIRD_PARTY.md).

## Épinglage précédent : fork `7b0f736` (24 novembre, nuit)

`third_party/victory` pointe vers `fxceless/victory-fork` au commit `7b0f736` (« Shader tests: recognise the software adapter from the device's own adapter »). Il était d'abord épinglé sur `d279e69` (« Make host pipeline waits configurable and prewarm pending requests (#1) », contribué par ZazaJr24) ; `7b0f736` ajoute `e9765a7` (« Fix the device loss introduced by the wave64 update ») et `004bf69`. Les commits plus récents, à partir de `1684989`, ne sont pas pris : le mainteneur les a jugés en régression. Ce fork n'est pas un fork GitHub rattaché à VIctory ; sa licence est GPL-3.0, compatible avec AnyXboxX (GPL-3.0 ou ultérieure). Il remplace notre ancienne intégration `anyxboxx/stable`.

Mesuré par le mainteneur le 2026-11-24 (i7-13700KF + RTX 4070 SUPER, Windows 11) en lançant notre propre copie du build 1.0.2.1, d'après le journal de VIctory de l'essai (fréquence d'images et compteurs de dessins tels que le programme les affiche), contre notre ancienne intégration `last` :

| | fork `d279e69` (premier épinglage) | ancienne intégration `last` |
| --- | --- | --- |
| Fréquence dans le monde | 4,0 à 4,3 fps | 2,3 fps |
| Dessins par image | environ 4 580 | environ 2 860 |
| Dessins perdus sans pipeline | 428 | 400 |
| Dessins indirects perdus | 3 | 86 |
| Perte du périphérique | aucune pendant environ 13 minutes, puis perdu (voir limites connues) | non relevée |

Le monde est atteint et affiché. Rien ici ne vient de la lecture du code du jeu.

**Observé par le mainteneur (Windows 11, RTX 4070 Super).** Avec le fork `fxceless/victory-fork` au commit `d279e69` et les réglages du lanceur, le jeu a été joué à environ 4 fps pendant environ 13 minutes dans la zone de départ (la prison) : le personnage se déplace sous le contrôle du joueur et interagit avec les premiers PNJ, puis le périphérique hôte est perdu (limite connue).

**Mesuré sur `7b0f736` (mainteneur, même machine, d'après le journal VIctory de l'essai `victory-fork-7b0-relay.log`).** Avec `VICTORY_RELAY=2` et les réglages du lanceur, cache de pipelines déjà rempli : monde atteint en environ 1 min 25 ; 3,5 i/s à la première image du monde, puis 4,5 i/s après 2 min, environ 4 975 draws par image, 346 draws perdus faute de pipeline hôte ; aucune perte du périphérique pendant environ 2 min 40 dans le monde, puis le mainteneur a fermé la fenêtre. Un essai antérieur de `7b0f736` sans `VICTORY_RELAY` : 5,4 i/s à environ 3 min, aucune perte, fenêtre fermée. Avis du mainteneur : « légèrement mieux » que `d279e69`. Référence `d279e69` : 4,0 à 4,3 i/s, environ 4 580 draws, environ 430 perdus.

**Limites connues (stabilité).** La perte du périphérique après environ 13 minutes a été observée sur `d279e69` ; la suite de ce paragraphe décrit cet essai. `7b0f736` corrige une perte de périphérique liée au wave64 ; sa tenue longue n'est pas encore mesurée chez nous (notre essai le plus long dessus dure environ 3 minutes). L'issue n°26 du fork rapporte 21,5 minutes stables sur une RTX 4060 Ti avec `VICTORY_RELAY=2` (rapport tiers, non reproduit ici). Sur `d279e69`, le deuxième lancement a tenu environ 13 minutes dans le monde (premiers dessins à 19:07:27 UTC), puis a perdu le périphérique GPU à 19:20:35 UTC : « host GPU device removed after 15 submissions sent ahead (VICTORY_LATE_WAIT=1) of 82 lists », la dernière liste étant un compute shader (`c1488f485ad606fb`). Mesuré, d'après le journal de VIctory de l'essai ; la cause n'est pas connue. Comptez de quelques minutes à environ 13 minutes dans le monde avant une perte du périphérique, puis relancez. Le message cite `VICTORY_LATE_WAIT=1` alors que le lanceur pose `3` : le réglage réellement actif dans cet essai reste à vérifier. Vérifié depuis (lu statiquement dans `crates/victory-gpu/src/host.rs`, `late()`, même code à `d279e69` et `7b0f736`) : `3` était actif. L'étape 3 inclut les étapes 1 et 2, et les mentions `(VICTORY_LATE_WAIT=1)` et `(VICTORY_LATE_WAIT=2)` des lignes du journal nomment l'étape à laquelle appartient un compteur, pas la valeur posée.

**Premier lancement à froid.** Le premier lancement avec un cache vide a planté au chargement (compilation des pipelines, puis le plantage interne du jeu suivi sous VIctory #1464). Le deuxième lancement, avec le cache rempli, a tenu. Si le premier plante, relancez.

Les réglages de l'essai réussi sont maintenant ceux par défaut de `tools/lancer_victory.ps1` (tableau de la section Lancement), sauf cinq variables que le code épinglé ne lit jamais et DRED, devenu optionnel (voir Lancement). Ils demandent la compilation `diag` (`--profile diag --features diagnostics`). Passez `-NoTunedSettings` pour les désactiver.

## Ce que vous obtenez aujourd'hui

Mesuré le 2026-11-24 sur i7-13700KF + RTX 4070 SUPER, Windows 11, avec l'intégration stable :

- Le démarrage, l'écran de profil, les réglages de première exécution, les écrans légaux, le chargement et le menu pause fonctionnent, à 30 i/s dans les menus.
- La première cinématique (crédits d'ouverture de la mission 1) est atteinte.
- La scène 3D derrière les crédits est noire.
- Dans le monde, le jeu tourne à 0,3 à 0,6 image par seconde.
- Il n'est pas jouable.

La section suivante décrit un état plus avancé, mesuré le même après-midi sur une branche d'intégration locale qui n'est pas encore ce que pointe `third_party/victory`.

## État au 24 novembre (après-midi)

Mesuré en lançant notre propre copie du build 1.0.2.1 sous VIctory, sur une branche d'intégration locale (`main` de VIctory + environ 40 pull requests + correctifs locaux pas encore publiés), i7-13700KF + RTX 4070 SUPER. Rien ici ne vient de la lecture du code du jeu : ce sont des mesures d'exécution (i/s, temps, comptes) et la sortie de `victory translate` sur les shaders de notre copie.

- **Monde atteint, personnage contrôlable (observé).** Le tutoriel « Move / Move the camera » (L / R) s'affiche. L'interface est composée par-dessus une image 3D encore noire.
- **Vitesse dans le monde (mesurée) : 0,2 i/s, puis 2,3 à 2,5 i/s.** Causes et correctifs :
  - `VICTORY_D3D12_DRED=1` coûtait environ 2,5 fois (mise en forme de texte et un `VirtualQuery` par draw). Ne l'activer que pour enquêter sur un device removed.
  - La copie retour vers le titre passait 66 % du thread de soumission dans `WriteProcessMemory`, lui-même dans `NtQueryVirtualMemory` (environ 2 ms par appel). Une écriture directe `NtWriteVirtualMemory` fait passer de 1,5 s à 0,13 s par image.
  - Les copies n'appellent plus `VirtualQuery` au préalable ; l'instantané, la comparaison et le hachage des textures sont répartis sur un pool de threads.
  - Reste par image (environ 410 ms) : GPU environ 145 ms, copie retour environ 73 ms, hachage 20 à 60 ms, instantané environ 50 ms.
- **Shaders non traduits en jeu : 456 le matin, environ 170 l'après-midi (compte mesuré en jeu).** Sur le corpus du jeu (26 084 programmes, `victory translate`), les programmes émis sont : VS/PS 13 480 sur 14 038, CS 2 212, MS 8 654 sur 8 765 (comptes tels qu'affichés par l'outil). Motifs résolus dans la journée : descripteur lu comme donnée, mémoire scalaire lue comme donnée, buffer de stride inconnue, préfixe de vague en flot divergent (passes InstanceCulling), masque rangé dans une lane de VGPR, `readfirstlane` d'une adresse uniforme (ColorDownsample). Restent surtout les masques de lanes : « lane mask read as data » (586 programmes) et « holding lanes elsewhere » (282) sur le corpus.
- **Ray tracing.** Le constructeur de BVH du titre provoque un TDR (récupération après dépassement de délai du GPU) ; on le saute avec `VICTORY_SKIP_BVH_BUILDER=1`. La nouvelle option de diagnostic locale `VICTORY_RT_MISS=1` fait manquer tous les rayons : 36 conteneurs de ray tracing sur 47 sont émis au lieu de 18. Une analyse de Codex a trouvé que `ConstructBVH4` contient une scrutation bornée (256 itérations) sur une valeur produite par une autre passe.
- **Composition finale.** Comme dans VIctory #841, la scène existe en 2560x1440 mais la cible présentée reste à zéro.

### Variables utiles

Toutes sont des diagnostics : elles demandent le build `diag` (la fonctionnalité `diagnostics`). Provenance, vérifiée dans le code du worktree d'intégration et contre le `main` de VIctory le 2026-11-24 :

| Variable | Effet | D'où elle vient |
| --- | --- | --- |
| `VICTORY_KEYS` | Appuis de touches scriptés (voir plus bas). | `main` de VIctory |
| `VICTORY_SHADER_LOOP_LIMIT=32` | Les boucles traduites s'arrêtent après ce nombre d'itérations (décimal ou hexadécimal `0x`, au moins 1). | `main` de VIctory |
| `VICTORY_D3D12_DRED=1` | Données de device removed. **Attention : n'importe quelle valeur l'active, et le monde ralentit d'environ 2,5 fois ; ne l'utiliser que pour enquêter sur un device removed.** | `main` de VIctory |
| `VICTORY_SHOW_TARGET=busiest` | Affiche la cible de rendu la plus chargée au lieu de l'image finale. | Pull request ouverte #638 (`Marvisak/VIctory`), dans notre intégration |
| `VICTORY_CAPTURE_BUSY=<draws>[,<nombre>]` | Capture les images de plus de ce nombre de draws (2 images par défaut). | Pull request ouverte #638 |
| `VICTORY_PRODUCERS=1` | Journalise une passe qui lit ce que seul du travail abandonné (non traduit ou sauté) écrit, avec le shader abandonné. | Pull request ouverte #638 |
| `VICTORY_CAPPED_DYNAMIC=262144` | Plus d'emplacements de descripteurs dynamiques sur un tas plafonné (NVIDIA), au prix des plus hauts emplacements du titre. | Essai local, publié seulement sur notre fork (`anyxboxx/essai`, `anyxboxx/stable`) |
| `VICTORY_SKIP_BVH_BUILDER=1` | Écarte les shaders du constructeur de BVH du titre. | Ancienne branche `anyxboxx/essai` seulement |
| `VICTORY_PROFILE=<ms>` | Profileur à échantillonnage de tout le processus ; journalise les piles d'appels les plus fréquentes toutes les 5 s (période en ms, 4 par défaut). | Local, non publié |
| `VICTORY_RT_MISS=1` | Tous les rayons manquent (diagnostic de ray tracing, désactivé par défaut). | Local, non publié |

## État au 24 novembre (soir)

Mesuré en lançant notre propre copie du build 1.0.2.1 sous VIctory, sur la branche d'intégration locale (`main` de VIctory + pull requests + correctifs locaux pas tous publiés), i7-13700KF + RTX 4070 SUPER. Ce sont des mesures d'exécution (i/s, comptes, traces) et la sortie de `victory translate` ; rien ne vient de la lecture du code du jeu.

- **Le mode Series S est plus rapide (mesuré).** `victory run --console series-s ...` donne environ 3,1 à 3,3 i/s dans le monde, contre environ 2,0 à 2,4 i/s en mode Series X (environ +50 %), avec le même symptôme d'image. Le jeu rend alors en interne en 1280x720 et présente en 1920x1080. Recommandé pour itérer.
- **Diagnostic de l'image noire (mesuré).** La géométrie existe dans le G-buffer : les personnages de la cinématique sont visibles en 1440p et en 720p. La couleur finale est une teinte uniforme qui s'assombrit avec le temps (exposition), avec les sous-titres dessinés par-dessus. Presque toutes les passes d'éclairage différé sont traduites et liées. Une trace par passe (option locale `VICTORY_INDIRECT_TRACE`, non publiée) montre qu'environ 379 `ExecuteIndirect` de mesh partent avec des arguments à zéro, parce que les passes GPU qui produisent ces arguments ne sont pas encore traduites : le traducteur les refuse avec « an address from a phi » et « a lane mask holding lanes elsewhere read as data ». C'est le chantier en cours.
- **Traduction (mesurée).** Shaders non traduits en jeu : environ 130 à 170 (compte en jeu). Sur le corpus (26 084 programmes, `victory translate`) : VS/PS 13 780 émis sur 14 038, CS 2 212, MS 8 654 sur 8 765 (comptes tels qu'affichés par l'outil).
- **Correctifs envoyés à `Marvisak/VIctory`** : #1215 (vitesse : écriture directe `NtWriteVirtualMemory`, copies sans `VirtualQuery`, profileur), #1225 (`readfirstlane` d'une adresse uniforme : ColorDownsample), #1231 (compteur de boucle uniforme : sondes de lumière). Des commentaires de mesures ont aussi été ajoutés sur #497, #320, #1074, #1198 et #841.
- **Communauté.** Le projet a un serveur Discord (lien en haut du README).

Variable ajoutée au tableau ci-dessus (diagnostic, build `diag`) :

| Variable | Effet | D'où elle vient |
| --- | --- | --- |
| `VICTORY_INDIRECT_TRACE` | Trace par passe des appels `ExecuteIndirect` et de leurs arguments. | Local, non publié |

## Prérequis

- Windows 10 ou 11 x64.
- Un processeur avec AVX2. Intel convient : les instructions propres à AMD sont émulées.
- Un GPU Direct3D 12 avec shader model 6.6 et mesh shaders (NVIDIA RTX série 20 ou plus récent, AMD RDNA 2 ou plus récent).
- 32 Go de RAM conseillés et un fichier d'échange Windows assez grand : le jeu engage plusieurs Gio au démarrage. Sinon, `crashes.log` signale l'erreur Windows 1455 : fermez d'autres applications ou agrandissez le fichier d'échange.
- Votre propre copie du build Xbox Series 1.0.2.1 : un dossier qui contient `game_scarlett_master.exe`. Non fournie, non expliquée ici.
- De l'espace disque pour le cache de shaders (il grossit au fil du jeu ; voir la page d'état pour les chiffres à prévoir).

## Installation

1. [Git](https://git-scm.com/).
2. Rust via [rustup](https://rustup.rs/). La toolchain exacte est fixée par `third_party/victory/rust-toolchain.toml` et s'installe seule à la première commande `cargo`.
3. Visual Studio 2022 ou 2026 Build Tools avec la charge « Développement Desktop en C++ » (MSVC).
4. Windows SDK 10.0.26100 (fournit `dxc`).
5. Optionnel : la fonctionnalité Windows « Graphics Tools » (couche de débogage D3D12, utile seulement pour `VICTORY_D3D12_DEBUG`).
6. Python 3.12, seulement pour la suite de tests d'AnyXboxX (voir Tests).

Récupérer le code (VIctory est inclus) :

```powershell
git clone https://github.com/LooperSalty/AnyXboxX.git
cd AnyXboxX
```

## Compilation

```powershell
cargo build --profile diag -p victory --features diagnostics --manifest-path third_party/victory/Cargo.toml
```

Le profil `diag` est nécessaire pour les touches scriptées, les captures d'images et les options de diagnostic ci-dessous. La première compilation télécharge et compile toutes les dépendances : comptez plusieurs minutes. L'exécutable est `third_party\victory\target\diag\victory.exe` (ou `%CARGO_TARGET_DIR%\diag\victory.exe` si cette variable est définie). `tools/lancer_victory.ps1` lance cette compilation à votre place à chaque lancement : cargo ne fait rien quand l'exécutable est à jour (environ 0,1 s) et le recompile après une mise à jour du sous-module, si bien qu'un ancien `victory.exe` ne tourne jamais avec des réglages qu'il ne lit pas. Si la compilation échoue, le lanceur s'arrête et donne le code de sortie de cargo.

## Lancement

```powershell
.\tools\lancer_victory.ps1 -GameDir <dossier du jeu>
```

Le script règle les variables ci-dessous (sauf avec `-NoTunedSettings`, qui ne garde que `VICTORY_GAME_DIR`) puis exécute `victory.exe run <dossier du jeu>\game_scarlett_master.exe --execute` :

| Variable | Valeur | Pourquoi |
| --- | --- | --- |
| `VICTORY_GAME_DIR` | votre dossier du jeu | Aucun `victory.toml` n'est nécessaire. |
| `VICTORY_SHADER_LOOP_LIMIT` | `256` | Les boucles des shaders traduits étaient quasi infinies, elles sont donc coupées. À 32 (l'ancienne valeur), les cheveux s'affichent en mèches lumineuses et la scène est plus sombre ; à 1024 le périphérique a été perdu sur une RTX 3060 Ti avec le délai par défaut de 2 secondes ; 256 a tenu (issue #18 du fork, et observé sous Linux aux lancements 3 et 4 ci-dessous). |
| `VICTORY_LATE_WAIT` | `3` | Réglage d'attente des pipelines côté hôte, de la pull request n°1 du fork. |
| `VICTORY_WIDE_BACK` | `1` | Option du fork, active dans l'essai mesuré. |
| `VICTORY_RELAY` | non posé | Option du fork, plus posée par défaut (issue n°89 du fork : la peau disparaît, les sous-titres se superposent) ; `2` était actif dans les essais mesurés de `7b0f736` et du lot 4. |
| `VICTORY_CARD_BUFFERS_MIN_MB`, `VICTORY_CARD_BUFFERS_MB` | `1`, `1024` | Dimensionnement des tampons de carte, en Mo. |
| `VICTORY_DSTORAGE_NOTIFY` | `1` | Les écritures DirectStorage dans des pages physiques sont signalées au côté GPU (commits du fork `e49864a` et `bf870de` de Yorivel, issue #62 du fork) : sans elle, les murs de la première pièce peuvent lire des copies périmées et paraître transparents. |
| `VICTORY_STAGING_POOL_MB` | `1024` | Les tampons d'envoi des textures choisies par index sont gardés pour resservir, jusqu'à ce nombre de Mo, au lieu d'un tampon créé et mappé à chaque envoi (`0` n'en garde aucun). Mesuré sur une RTX 5080 (pilote 621.01, journaux de VIctory, deux lancements dans chaque cas, 25 novembre) : à l'entrée dans le monde, le passage sous 2,5 i/s dure environ 40 s au lieu d'environ 80 s, et la passe des textures des 100 premières secondes prend 190 ms par image au lieu de 560 à 730 ms ; rien ne change une fois la file vide (3,4 à 3,5 i/s dans les deux cas). |

Les anciennes valeurs par défaut `VICTORY_RT_MISS`, `VICTORY_WAVE32_READFIRSTLANE`, `VICTORY_WAVE32_CROSSLANE`, `VICTORY_SKIP_BVH_BUILDER` et `VICTORY_CAPPED_DYNAMIC` sont retirées : aucun fichier du commit épinglé ne les lit (le constructeur BVH est coupé sauf avec `VICTORY_BVH_BUILDER=1`). `tests/test_lanceur_victory.py` vérifie maintenant que VIctory lit chaque variable posée par le lanceur.

Ces valeurs ne changent pas depuis les 57 commits Proton et AMD : ce sont les réglages Windows du mainteneur, pas encore mesurés avec ces commits. Sous Linux avec RADV, `VICTORY_LATE_WAIT=3` fait perdre le périphérique (issue #184 du fork) ; les réglages du lancement Linux mesuré sont dans la section des correctifs Proton et AMD plus haut.

`-Dred` pose `VICTORY_D3D12_DRED=1` (données de périphérique perdu) pour diagnostiquer une perte du périphérique. Il est coupé par défaut : VIctory l'active dès que la variable existe, et il note chaque opération des listes de commandes, ce qui ralentit chaque image.

`-XgDll <chemin de xg_xs.dll>` pose `VICTORY_XG_DLL` : votre propre copie de la bibliothèque PC hors ligne de disposition des textures du GDK dispose les textures du jeu (voir la section du lot 8 et de la bibliothèque de disposition plus haut). `-XgMode compare` (`VICTORY_XG_MODE`) garde à la place les dispositions de VIctory et journalise chaque écart avec la bibliothèque ; `real` est la valeur par défaut. Ne copiez jamais la DLL dans ce dépôt.

`-Precompile` lance d'abord `victory.exe translate --cache --pipelines --used-pipelines <dossier du jeu>\shaders.rpf <dossier du jeu>\game_scarlett_master.exe <dossier du jeu>\XboxBvhBuilder.dll <dossier du jeu>\amd_fidelityfx_upscaler_xbox.dll` avec les mêmes variables d'environnement que le jeu (la clé du cache en dépend), puis démarre le jeu. Sur la machine du mainteneur, cela a pris environ 4,5 min et rempli 13 Go de cache (mesuré) ; les lancements suivants le réutilisent. Le lanceur s'arrête avec un message clair si l'un de ces fichiers manque.

Le premier lancement à froid peut planter pendant la compilation des pipelines : relancez une fois le cache rempli.

`-SkipPipelines <hash,...>` est transmis à `VICTORY_SKIP_PIPELINES` (voir plus bas).

Où sont les choses :

- Sauvegardes, réglages et `crashes.log` : `%LOCALAPPDATA%\victory\data`.
- Données dérivées (caches de shaders et de pipelines) : `%LOCALAPPDATA%\victory\cache`.

Les deux sont volontairement hors du dépôt. Supprimez le dossier du cache pour repartir d'un cache vide ; gardez le dossier des données si vous voulez conserver votre sauvegarde.

## Arriver à la première cinématique

Le titre de la fenêtre affiche la fréquence d'images. La fenêtre doit avoir le focus.

1. Écran de profil : Entrée.
2. Réglages de première exécution : Entrée à chaque écran.
3. Mentions légales : descendre sur « Suivant », attendre la fin du compte à rebours de l'avertissement de photosensibilité, cocher les deux cases, puis « Confirmer ».
4. Chargement, puis la première cinématique (crédits d'ouverture).
5. Échap ouvre le menu pause.

Clavier ou manette. Sans manette XInput branchée, le clavier sert de manette tant que la fenêtre du titre a le focus :

| Touches | Manette |
| --- | --- |
| Entrée, Retour arrière, X, Y | A, B, X, Y |
| Flèches | croix directionnelle |
| W A S D / I J K L | stick gauche / stick droit |
| Q / E, Z / C | gâchettes hautes, gâchettes basses |
| F / H | clics de sticks |
| Échap | Menu |
| Tab | View |

Maintenez chaque touche un instant : le jeu lit l'entrée à 30 Hz, un appui très bref peut être manqué. Les lancements suivants reprennent directement la sauvegarde.

## Lancements scriptés et diagnostics

Ils demandent le build `diag`.

| Variable | Effet |
| --- | --- |
| `VICTORY_KEYS=40:Enter,55:Enter,70:Enter` | Appuie sur les touches à ces secondes, avec ou sans focus. |
| `VICTORY_CAPTURE=<dossier>` et `VICTORY_CAPTURE_FRAMES=<liste>` | Enregistre les images présentées (ces numéros d'image). |
| `VICTORY_SHOW_TARGET=busiest` | Affiche la cible de rendu la plus chargée à la place de l'image finale (par exemple le G-buffer). |
| `VICTORY_D3D12_DRED=1` | En cas de panne du GPU, donne les commandes en cours. Ralentit le monde d'environ 2,5 fois : à n'activer que pour enquêter sur un device removed. |
| `VICTORY_D3D12_DEBUG=1` | Couche de débogage D3D12 (Graphics Tools requis). |
| `VICTORY_GPU_TIMING=1` | Chronomètre chaque liste de commandes et journalise les lentes. |
| `VICTORY_SKIP_PIPELINES=<hash,...>` | Omet les dessins et dispatchs qui utilisent les hash de shader à 16 chiffres listés. |
| `VICTORY_LOG=<cible>=debug,...` | Sortie debug et trace. |

Pour arrêter le jeu : `taskkill /F /IM victory.exe`. La commande `timeout` de Git Bash ne tue pas l'exécutable.

Pour mesurer un lancement, envoyez le journal de VIctory (stderr) dans un fichier, par exemple `.\tools\lancer_victory.ps1 -GameDir <dossier> 2> victory.log`, puis résumez-le avec `python tools/resume_journal_victory.py victory.log`. Le script garde les lignes d'images du monde (100 draws par image ou plus, `--draws-min` change le seuil) et donne la médiane et l'étendue de chaque partie de l'image (préparation, copies, textures, GPU, copie de retour, draws, draws perdus faute de pipeline hôte), ainsi que le nombre d'avertissements (shaders qui ne se traduisent pas, pipelines refusés, périphérique perdu). Avec deux journaux (`avant.log apres.log`), il compare les médianes colonne par colonne, pour juger un changement sur la partie de l'image qu'il vise. `--markdown` écrit des tableaux à coller dans une issue. Seuls des comptes et des temps sont écrits.

## Anciennes branches

Les branches `anyxboxx/stable` et `anyxboxx/essai` de `LooperSalty/victory` ne sont plus l'épinglage : le fork ci-dessus les remplace. Les sections qui les citent décrivent des mesures antérieures.

## Sécurité

- Sur NVIDIA, rester longtemps dans le monde peut provoquer une réinitialisation du pilote d'affichage (tous les écrans noirs un instant), et un testeur a eu un écran bleu (0x119). Arrêtez le jeu après quelques minutes dans le monde.
- Ne lancez qu'une instance à la fois.

## Tests

```powershell
cargo test -p victory-gpu -p victory-shader --manifest-path third_party/victory/Cargo.toml
```

AnyXboxX lui-même (voir la section Setup d'[AGENTS.md](../../AGENTS.md)) :

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -r requirements.txt -r requirements-dev.txt
.\.venv\Scripts\python.exe -m pytest
.\.venv\Scripts\python.exe -m ruff check --select W605,F,E9 PrototypeScarlett tests tools
```

## Où signaler un problème

Ouvrez une issue sur [AnyXboxX](https://github.com/LooperSalty/AnyXboxX/issues) ou sur [VIctory](https://github.com/Marvisak/VIctory/issues) (l'issue #264 regroupe les blocages GPU sur NVIDIA). Décrivez le symptôme, votre GPU et votre pilote, et les lignes utiles de `crashes.log`. Ne joignez jamais de fichier du jeu.
