> English: [GAME_ANALYSIS.md](../../../research/package/GAME_ANALYSIS.md)

# GTA VI sur Xbox Scarlett — ce que révèle le paquet 1.0.2.1

Le paquet sépare les ressources communes, les données du mode histoire et celles d’un mini-jeu. On peut retrouver les limites des blocs de ressources et en décompresser une partie. La logique des missions reste inconnue. Le layout désigne un exécutable principal absent à sa destination, et aucun script de mission lisible n’a été reconstruit à partir des deux archives de scripts.

## Reproduire les résultats

Depuis le dossier qui contient `MicrosoftGame.Config`, avec Python 3.10 ou ultérieur. Vérifié localement avec Python 3.12.

```powershell
# Inventaire JSON par fichier, avec son chunk, sa présence et sa taille
python .\analyse_paquet.py inventaire .

# Examiner le bloc d’index ETPF, puis le premier bloc de données de l’interface
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 0
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 1

# Facultatif : enregistrer les octets décompressés dans un nouveau fichier
python .\analyse_paquet.py bloc .\data\ttdb\ui\platform.ttpf 1 --sortie .\platform-bloc-1.bin

# Retrouver les en-têtes candidats des deux archives de scripts
python .\analyse_paquet.py reperes-rpf .\script.rpf .\pack\story000\story000_script.rpf
```

## Organisation du jeu sur disque

Le rapprochement entre les destinations du layout et le disque donne **103 fichiers présents sur 104**, répartis dans **25 chunks**. Le seul fichier manquant est `game_scarlett_master.exe` dans le chunk 1001. Les deux manifestes désignent ce même fichier et déclarent la version `1.0.2.1`. `MicrosoftGame.Config` précise la cible `Scarlett`. Ces identifiants décrivent le paquet local. Son origine n’a pas été authentifiée.

| Ressources | Observation locale | Ce qu’on peut en déduire |
| --- | --- | --- |
| Monde commun | `levels.rpf` occupe 70,92 Gio. `navmeshes.rpf` a son propre chunk. Les sous-dossiers TTDB contiennent huit fragments `ttdb_maps_0_7` à `ttdb_maps_7_7` et huit fragments `ttdb_terrain_0_7` à `ttdb_terrain_7_7`. | La distribution distingue les niveaux, la navigation et le terrain. La correspondance entre fragments et quartiers reste inconnue. |
| Histoire | Le chunk 3000 regroupe `story000.rpf` (18,65 Gio), `story000_script.rpf` et trois TTPF dédiés aux cartes, au terrain et aux effets visuels. Le chunk 1014 contient `script.rpf`. | Les scripts de l’histoire disposent d’une archive distincte de l’archive commune. Le regroupement décrit l’installation. L’ordre de chargement en jeu reste inconnu. |
| Mini-jeu | Le chunk 3003 regroupe `shared_minigame000.rpf` avec ses propres TTPF de carte et de terrain. | Le mini-jeu a des ressources de monde dédiées. Son nom, ses règles et son déroulement restent indéterminés. |
| Audio et vidéo | `audio.rpf` occupe 26,58 Gio et `movies.rpf` 8,23 Gio. La table d’exports de `Bink2Scarlett.dll` contient `BinkOpen`, `BinkDoFrame` et `BinkNextFrame`. | Le paquet embarque une bibliothèque capable d’ouvrir et de décoder des séquences vidéo. |
| Rendu | Les deux DLL FidelityFX exportent notamment `ffxCreateContext` et `ffxDispatch`. Les exports de `XboxBvhBuilder.dll` incluent `BuildRaytracingAccelerationStructure` dans leurs noms C++ décorés. | Des interfaces de mise à l’échelle et de construction de structures d’accélération sont livrées. Les réglages actifs en jeu restent inconnus. |

## Décodage des conteneurs TTPF

Le détail des champs ETPF et les commandes du lecteur dédié figurent dans [ANALYSE_ETPF.md](ANALYSE_ETPF.md).

Les **57 TTPF** déclarés dans le layout totalisent **62 217 061 888 octets**. Le lecteur a vérifié **1 026 890 intervalles** sur leurs tables, y compris ceux des sous-dossiers de `pack`. La vérification porte sur chaque borne. Le décodage des données a été échantillonné séparément.

| Position | Interprétation vérifiée sur cette copie |
| --- | --- |
| `0x00` | Signature ASCII `RDS5`. |
| `0x04` | Entier 32 bits little-endian, nombre de lignes de la table. Deux lignes successives délimitent un intervalle. |
| `0x08` | Les 56 bits de poids faible du mot 64 bits donnent la taille logique finale, en octets. |
| `0x10` | Début de la table. Pour chaque mot 64 bits little-endian `w`, la position sur disque vaut `(w & 0xffffff) * 256`. La position dans les données logiques vaut `((w >> 30) & 0x3fffff) * 4096`. |

Dans les 57 fichiers, les deux séries de positions augmentent strictement. La première position physique suit la table arrondie au multiple supérieur ou égal de 256 octets. Elle pointe sur la signature `ETPF`. La dernière position physique correspond exactement à la taille du fichier. L’intervalle d’indice 0 contient l’en-tête ETPF. Les données suivantes commencent à l’indice 1.

Le **bit 29** d’une ligne permet de reconnaître les blocs zlib dans l’échantillon examiné. Sur le bloc 1 de chacun des 57 fichiers, le bit est positionné 52 fois. Dans les 52 cas, le décodeur atteint la fin du flux zlib et obtient exactement la taille prédite par les positions logiques. Les cinq autres blocs ont un codage encore indéterminé. Le lecteur restitue alors leur préfixe et leurs bornes. Il applique une limite de 64 Mio par bloc à décompresser.

Un second échantillon reproductible, tiré avec `random.seed(42)` et jusqu'à 40 intervalles de données par TTPF, a porté sur 2 160 blocs dont les tailles stockée et logique ne dépassent pas 2 000 000 octets. Les 1 889 blocs dont le bit 29 est positionné ont produit un flux zlib complet de la taille attendue. Les 271 autres n'ont pas été reconnus comme zlib. Ce résultat décrit l'échantillon, pas l'ensemble des blocs.

**Exemple vérifiable.** Dans `data/ttdb/ui/platform.ttpf`, l’intervalle 0 va de `0x100` à `0x1100` exclu. L’intervalle 1 commence à `0x1100` et finit à `0x36700` exclu. Sur ses 218 624 octets, 218 411 appartiennent au flux zlib et 213 le suivent. La décompression produit **524 288 octets**, dont le SHA-256 est `4887713f84d6184cc6b4ded7e90d3d6495a66c173840f6972a2d70100d9b61d6`. L’empreinte permet de vérifier indépendamment l’export. Le format visuel du bloc reste à identifier.

## Archives de scripts

Le relevé des champs RPF et le lecteur d'en-têtes figurent dans [RPF_RECHERCHE.md](RPF_RECHERCHE.md).

Les 22 fichiers RPF du layout commencent par les octets `39 46 50 52`, soit `9FPR` dans l’ordre de lecture. À l’intérieur des deux archives de scripts, des positions alignées sur 4 096 octets reproduisent cette signature et les huit octets `00 7a 04 00 50 01 00 00` à la position relative `0x18`.

| Archive | Positions candidates en octets depuis le début du fichier |
| --- | --- |
| `script.rpf` | 0, 4 096, 802 816 |
| `pack/story000/story000_script.rpf` | 0, 4 096, 98 304, 51 539 968, 77 819 904 |

La position zéro est l’en-tête externe. Les répétitions internes sont compatibles avec des conteneurs imbriqués. Le filtre fournit des points où reprendre l’analyse. Déduire un nombre de missions demanderait d’identifier les entrées de répertoire, puis le bytecode associé à chacune.

## Ce que lance réellement le fichier Windows

Le désassemblage statique du code .NET de `Lancer GTA VI.exe` montre trois méthodes qui expliquent son comportement.

- `Main` lit un chemin mémorisé dans `jeu.txt`. La touche Maj permet de revenir au sélecteur de fichiers. La méthode peut y enregistrer le nouveau choix.
- `CanLaunch` vérifie que la cible existe et porte l’extension `.exe`. Elle exclut le lanceur lui-même et tout fichier nommé `game_scarlett_master.exe`. Les comparaisons de noms ignorent la casse.
- `RunGame` prépare le répertoire de travail et appelle `System.Diagnostics.Process.Start` à l’instruction IL `0033`. Le rôle du lanceur est donc de démarrer un exécutable Windows sélectionné par l’utilisateur.

## Configuration binaire et ressources

`microsoft_gameconfig.bin` commence par `regf` et sa structure a pu être lue comme une ruche de registre Windows avec `python-registry`. Elle contient notamment les mêmes nom, éditeur, version, StoreId, cible Scarlett et exécutable que `MicrosoftGame.Config`. Les champs `PersistentLocalStorage\\SizeMB` et `VirtualMachine\\SMTEnabled` valent respectivement 2048 et 1. Ces données décrivent la configuration déclarée du paquet.

`resources.pri` commence par `mrm_pri2` et contient les chaînes UTF-16 `ms-appx://Jim_vesz1v3mcwykm/` et `Jim_vesz1v3mcwykm`. Ces chaînes diffèrent du nom d'identité présent dans les manifestes. Leur présence signale une divergence de métadonnées dont la cause et les conséquences n'ont pas été établies. `Catalog000.bin` possède une structure PE x64 avec une section et un point d'entrée nul. Son contenu interne reste à identifier.

## Méthode et rôle de REA

L'accès réseau ayant été rétabli, REA 6.10.1 a été exécuté avec Node 24.22.1 dans un environnement temporaire. Il a inspecté le dossier et les méthodes .NET du lanceur. Les résultats et la couverture figurent dans [REA_RECHERCHE.md](REA_RECHERCHE.md). Les mesures de blocs proviennent du lecteur Python local, et les exports des bibliothèques de leur table PE. [Guide REA pour les installations](https://github.com/morluto/rea/blob/main/docs/desktop-applications.md) · [Guide REA pour les assemblages .NET](https://github.com/morluto/rea/blob/main/docs/guides/managed/managed-code-analysis.md)

Avec la CLI REA installée, ces commandes documentées par le dépôt permettent de reprendre l’inventaire et l’inspection des méthodes du lanceur depuis le dossier du paquet.

```powershell
rea inspect-desktop-application . --detail complete --json
rea inspect-managed-members ".\Lancer GTA VI.exe" --json
```
