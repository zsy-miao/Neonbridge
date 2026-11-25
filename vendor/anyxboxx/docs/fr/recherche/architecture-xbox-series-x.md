> English: [xbox-series-x-architecture.md](../../research/xbox-series-x-architecture.md)

# Architecture de la Xbox Series X

Recherche faite le 21 novembre 2026 pour guider le développement d'AnyXboxX. Chaque fait porte sa source. La mention **vérifié** signifie que la page a été lue ; **à confirmer** signifie que le fait vient d'un extrait de recherche ou d'une source secondaire.

## Matériel

| Élément | Xbox Series X (« Anaconda ») | Xbox Series S (« Lockhart ») |
|---|---|---|
| SoC | TSMC N7 amélioré, 360,4 mm², 15,3 milliards de transistors [CB] | à confirmer |
| CPU | 8 cœurs AMD Zen 2 : 3,8 GHz, ou 3,66 GHz avec SMT [W] | 3,6 GHz, ou 3,4 GHz avec SMT [W] |
| Caches CPU | L2 de 512 Kio par cœur ; L3 de 4 Mio par CCX, 2 CCX de 4 cœurs [CB] | à confirmer |
| GPU | AMD RDNA 2, 52 CU actives sur 56, 3 328 shaders, 1,825 GHz fixes, 12,155 TFLOPS [W] | RDNA 2, 20 CU, 4,006 TFLOPS [W] |
| Cache GPU, ray tracing | L2 de 5 Mio ; 380 milliards d'intersections rayon-boîte par seconde [CB] | à confirmer |
| Mémoire | 16 Gio GDDR6, bus 320 bits : 10 Gio à 560 Go/s et 6 Gio à 336 Go/s [W] | 10 Gio GDDR6 dont 8 Gio à 224 Go/s [W] |
| Mémoire pour les jeux | environ 13,5 Gio, le reste est réservé au système [W] | à confirmer |
| Stockage | SSD NVMe 1 To (802 Go utilisables), 2,4 Go/s bruts, 4,8 Go/s avec compression 2:1 [W][XV] | SSD 512 Go (364 Go utilisables) [W] |
| Décompression | bloc matériel : LZ général (zlib) et BCPack pour les textures [W][XV] | à confirmer |

Écarts relevés entre sources : 3,6 ou 3,66 GHz avec SMT ; 15,3 ou 15,4 milliards de transistors ; ComputeBase cite un plafond PCIe 4.0 ×2 de 4 Go/s, différent des 2,4 Go/s annoncés par Microsoft.

## Fonctions graphiques

- **Xbox Velocity Architecture** : SSD, décompression matérielle, DirectStorage et Sampler Feedback Streaming. Microsoft présente le SFS comme une innovation bâtie sur le reste de la Velocity Architecture [XV, vérifié].
- **DirectX 12 Ultimate** : ray tracing DXR, Variable Rate Shading, mesh shaders, Sampler Feedback [XR, à confirmer].
- Côté logiciel, les jeux passent par une variante console de Direct3D 12 (« D3D12.X ») fournie avec le GDK. Elle expose le matériel plus directement que la version PC [à confirmer].

## Sécurité et système

- **Modèle d'attaque physique** : depuis la Xbox One, Microsoft part du principe que la mémoire flash, le disque et même la DRAM peuvent être lus ou modifiés. Le SoC conçu avec AMD chiffre et vérifie ce qu'il charge [TX, vérifié ; source Xbox One].
- **Racine de confiance** : la présentation Hot Chips 32 cite un « HSP/Pluton RoT » et une protection « SPLEAP » contre les élévations de privilèges [HC32, à confirmer].
- **Hyperviseur** : un Hyper-V adapté sépare un système hôte, une partition « système » (interface, applications) et une partition exclusive pour le jeu [VT, à confirmer].
- **Conteneurs XVD/XVC** : en-tête signé, données chiffrées en AES-XTS ; la clé du contenu (CIK) est elle-même protégée par une clé de distribution (ODK) [XVD, vérifié].

## Ce que cela implique pour AnyXboxX

1. **CPU** : le jeu tourne en x86-64 natif. Unicorn suffit pour un prototype ; un émulateur jouable exécuterait le code directement sur le processeur du PC, comme Wine.
2. **GPU** : c'est le gros du travail. Il faut traduire les appels D3D12.X vers Direct3D 12 ou Vulkan, sans pouvoir s'appuyer sur une documentation publique.
3. **Mémoire** : les deux zones de débit (10 + 6 Gio) n'ont pas besoin d'être reproduites ; un tas unique suffit.
4. **Stockage** : DirectStorage et BCPack peuvent être émulés en logiciel, avec un coût en temps de chargement.
5. **Contenu** : les paquets de jeux sont chiffrés par des clés que seule la console détient. AnyXboxX n'exécute donc que des programmes non chiffrés, comme sa démo.

## Sources

- [W] Wikipédia, *Xbox Series X and Series S* : https://en.wikipedia.org/wiki/Xbox_Series_X_and_Series_S (vérifié)
- [CB] ComputeBase, résumé Hot Chips 2020 : https://www.computerbase.de/2020-08/xbox-series-x-hot-chips/ (vérifié)
- [XV] Xbox Wire, *A closer look at Xbox Velocity Architecture* : https://news.xbox.com/en-us/2020/07/14/a-closer-look-at-xbox-velocity-architecture/ (vérifié)
- [XR] Xbox Wire, intégration RDNA 2 : https://news.xbox.com/en-us/2020/10/28/a-closer-look-at-how-xbox-series-xs-integrates-full-amd-rdna-2-architecture/ (à confirmer)
- [HC32] Hot Chips 32, présentation Microsoft : https://hc32.hotchips.org/assets/program/conference/day1/HotChips2020_GPU_Microsoft_Jeff_Andrews_v2.pdf (à confirmer)
- [TX] Tony Chen, BlueHat 2019, *The Xbox One story* : https://xboxoneresearch.github.io/wiki/_files/bluehat-2019-the-xbox-one-story/source.txt (vérifié)
- [VT] Vectree, logiciel système Xbox : https://vectree.io/c/xbox-one-and-xbox-series-xs-system-software (à confirmer)
- [XVD] Wiki Xbox One Research, *Xbox Virtual Drive* : https://xboxoneresearch.github.io/wiki/operating-system/xbox-virtual-drive (vérifié)
