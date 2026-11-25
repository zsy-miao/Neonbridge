> English: [victory-xcl-fivestar.md](../../research/victory-xcl-fivestar.md)

# VIctory, xcl et five-star-recomp : les trois voisins suivis depuis le 24 novembre

État au 24 novembre 2026, après la suppression de Leonida. La règle 2 de [AGENTS.md](../../../AGENTS.md) s'applique : seulement des noms d'exports, slots, formes, flags/HRESULT et comptes, chacun étiqueté « mesuré » ; aucune adresse, octet, chaîne, nom ou hash de shader, aucun log du jeu. Vue d'ensemble des autres voisins : [voisins.md](voisins.md).

| Projet | Langage, licence | Ce qu'il fait |
| --- | --- | --- |
| [Marvisak/VIctory](https://github.com/Marvisak/VIctory) | Rust, GPL-3.0 ou ultérieure | Exécute le code x86-64 du titre dans le processus, plateforme HLE, shaders RDNA 2 traduits en DXIL à la première utilisation |
| [ivall/xcl](https://github.com/ivall/xcl) | Rust, GPL-2.0-only | Couche de compatibilité générique Xbox Series vers Windows, shaders RDNA 2 vers HLSL vers DXIL, exécute les programmes wave64 sur des GPU à vagues de 32 lignes en deux moitiés (leur `docs/wave64-on-wave32.md`) |
| [ThatRecompDev/five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp) | C# / .NET 10, GPL-3.0 ou ultérieure | Formats, traduction de shaders, runtime console hors ligne ; la préparation `fivestar setup` prend de 1 heure à plusieurs heures et plus de 100 Go |

Note de licence : xcl est en GPL-2.0-only, incompatible avec notre GPL-3.0 ou ultérieure pour du code copié. On lit son approche, on ne copie pas ses fichiers ; VIctory et five-star-recomp peuvent être repris avec crédit dans [THIRD_PARTY.md](../../../THIRD_PARTY.md).

## Mesuré sur notre machine (2026-11-24)

Banc : i7-13700KF, RTX 4070 SUPER, Windows 11. VIctory `main` plus sa pull request #89. Toutes les lignes ci-dessous sont **mesurées**.

- Mesuré : menus, écrans de première exécution, écrans légaux, carte titre de la mission 1 et menu pause atteints à 30 fps. Les instructions CPU propres à AMD sont émulées correctement sur Intel.
- Mesuré : sans la PR #89, deux cibles de rendu 16 bits (formats DXGI B5G5R5A1 et B4G4R4A4) sont refusées par le pilote NVIDIA.
- Mesuré : dans le monde, 1 à 2 images/s, puis `DXGI_ERROR_DEVICE_HUNG` environ 2 minutes après ; DRED situe l'arrêt sur un `DispatchMesh`.
- Mesuré : la couche de débogage D3D12 compte 119 compute shaders refusés car ils demandent des vagues de 64 lignes (le maximum du pilote est 32), 117 barrières avec un mauvais état « avant » et 74 tampons de sommets encore en état UAV.
- Mesuré : 351 programmes ne se traduisent pas encore chez VIctory dans le monde. Principales raisons (comptes) : select sur masque non suivi 82, masque de lignes lu comme donnée 76, adresse dans un SGPR hors paramètre racine 35, structured buffer de pas inconnu 31, lecture scalaire comme donnée 24.

## Ce que cela signifie pour AnyXboxX

Nos issues [#68](https://github.com/LooperSalty/AnyXboxX/issues/68) (contrôle de flux structuré et masques de lanes) et [#70](https://github.com/LooperSalty/AnyXboxX/issues/70) (wave64) sont exactement ces manques : masques de lignes lus comme données et select sur masque non suivi relèvent de #68 ; les 119 compute shaders refusés relèvent de #70. On réutilise plutôt que de réécrire, selon l'approche « ce qu'il faut reprendre » de [voisins.md](voisins.md) : l'approche wave64-sur-32 de xcl (deux moitiés, lue comme conception, non copiée à cause de la licence) et les PR VIctory #223 et #233 (GPL-3.0 ou ultérieure, reprises avec crédit).

## Intégration (2026-11-24)

Notre intégration était publiée sur le fork [LooperSalty/victory](https://github.com/LooperSalty/victory) et, quand cette note a été écrite, suivie par ce dépôt comme sous-module `third_party/victory` (le workflow horaire des voisins suivait la branche stable). VIctory est maintenant copié dans le dépôt sous `third_party/victory` ; `tools/lancer_victory.ps1` le compile et le lance sur votre propre dossier de jeu.

- Branche `anyxboxx/stable` : VIctory `main` plus les 24 pull requests créditées dans [THIRD_PARTY.md](../../../THIRD_PARTY.md), avec des adaptations locales (conflits résolus, plages mémoire déjà à jour dédupliquées, réglage `VICTORY_CAPPED_DYNAMIC` pour la plage de descripteurs dynamiques sur un tas plafonné NVIDIA). C'est la branche que nous épinglons.
- Branche `anyxboxx/essai` : la branche stable plus quatre pull requests (#294, #709, #571, #452). **Expérimentale seulement** : elle provoque encore des blocages du GPU.

Résultats sur la branche stable, i7-13700KF, RTX 4070 SUPER, Windows 11. Toutes les lignes sont **mesurées**.

- Mesuré : menus, écrans de première exécution, écrans légaux et menu pause à 30 i/s ; entrée dans le monde et **première cinématique (ouverture de la mission 1, crédits d'ouverture affichés)**.
- Mesuré : aucun retrait du GPU pendant 5 minutes dans le monde (avant : `DXGI_ERROR_DEVICE_HUNG` environ 2 minutes après).
- Mesuré : la scène 3D est présente dans le G-buffer mais l'image finale reste noire (passes d'éclairage non traduites) ; 0,3 à 0,5 image par seconde dans le monde. Pas jouable.

Causes trouvées, une à une :

- (a) Mesuré : le pilote NVIDIA refuse les compute shaders wave64 ; l'émulation du wave64 sur des vagues de 32 lignes (#189) règle le problème.
- (b) Mesuré : le `DEVICE_HUNG` sur `DispatchMesh` disparaît avec #493 et #206.
- (c) Mesuré : la plage de descripteurs dynamiques était trop petite sur un tas plafonné à 1 000 000 de descripteurs (65 536 slots) ; avec 262 144 slots, environ 720 dispatchs mesh indirects abandonnés par image disparaissent.
- (d) Mesuré : les boucles des shaders traduits étaient quasi infinies ; avec une limite de 32 itérations, le temps GPU par image passe de 1,1 s à 0,2 s.
- (e) Mesuré : le coût CPU de la surveillance de la mémoire du titre passe de 413 à 224 ms par image avec #393 et la déduplication des plages.
- (f) Mesuré : le G-buffer est rempli mais l'éclairage est bloqué par des compute shaders pas encore traduits (root UAV, accès LDS octet et demi-mot, chargement de tampon non signé 16 bits).
- (g) Mesuré : la branche expérimentale débloque les root UAV, mais certains compute shaders ainsi débloqués bloquent encore le GPU.
