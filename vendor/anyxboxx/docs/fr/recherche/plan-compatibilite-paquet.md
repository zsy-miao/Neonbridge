> English: [package-compatibility-plan.md](../../research/package-compatibility-plan.md)

# Préparer la compatibilité du paquet Scarlett étudié

Cette note sert aux agents qui poursuivent AnyXboxX. Elle décrit une stratégie de recherche et de tests, pas une recette permettant actuellement de lancer le jeu. Elle s'appuie sur la copie locale et sur les [notes d'analyse du paquet](paquet/README.md).

## État vérifié

- Le layout et `MicrosoftGame.Config` désignent `game_scarlett_master.exe`, absent de la copie. Depuis la racine AnyXboxX, `python PrototypeScarlett/scarlett.py --rapport ..` renvoie explicitement cette absence. Les imports, le point d'entrée, les interfaces et les shaders effectivement utilisés par l'exécutable sont inconnus. Les ressources ne permettent pas de reconstituer son code.
- L'initialisation de DLL documentée dans [l'avancée](../avancee.md) et les essais [SimpleTriangle et SDL](../jeu-de-test.md) constituent des tests partiels. Les deux exemples exécutés sont des versions PC : ils ne démontrent pas la compatibilité d'un exécutable Scarlett. Un import résolu ou un `DllMain` réussi ne prouve pas que toutes les fonctions de la DLL fonctionneront.
- Le mode natif exécute du code avec les droits du processus utilisateur. Utiliser des programmes de test de confiance ; le mode Unicorn sert aux diagnostics isolés.
- Les données locales et leur provenance n'ont pas été authentifiées. Les résultats ci-dessous concernent les fichiers examinés, pas nécessairement toutes les versions du jeu.

## Ordre de travail conseillé

1. **Maintenir une référence reproductible.** Lancer les tests du dépôt avec `python -m pytest` depuis sa racine. Conserver les exemples PC documentés et ajouter, lorsque disponible, un programme Scarlett de test autorisé et connu. Pour chaque résultat, noter la version de l'hôte, le mode, la commande, le binaire et son empreinte. Ne pas extrapoler les résultats PC au jeu absent.
2. **Chargeur et appels système.** Vérifier sections, relocations, résolution d'imports, TLS, initialisation et déchargement avec des PE de test contrôlés. Étendre progressivement les fonctions HLE à partir de traces de programmes de test ; respecter pour chaque API sa convention d'appel, ses structures, son type de retour et ses erreurs. Éviter de compter un import simplement résolu comme une implémentation fonctionnelle. Les limites actuelles concernant `E_NOTIMPL`, exceptions, threads et chargement dynamique sont décrites dans [l'avancée](../avancee.md) et le README.
3. **Graphisme et calcul.** Définir les interfaces D3D12.X réellement requises à partir de traces ou d'un binaire de test approprié ; construire des tests minimaux de ressources, synchronisation et commandes avant de généraliser. La compatibilité des shaders console, de leur format et de leur exécution reste à établir. Les chaînes FidelityFX de cette copie ne prouvent pas les fonctions activées par le jeu. Voir [REA_RECHERCHE.md](paquet/REA_RECHERCHE.md).
4. **Stockage et ressources.** Développer les lecteurs hors de l'émulateur avec des fichiers d'essai bornés et des validations de longueurs. Les tables RDS5 et ETPF sont décrites dans [ANALYSE_JEU.md](paquet/ANALYSE_JEU.md) et [ANALYSE_ETPF.md](paquet/ANALYSE_ETPF.md). La correspondance entre groupes et blocs a été vérifiée sur cette copie ; les formats de pixels, le codec des blocs sans drapeau zlib et la structure interne RPF restent ouverts. Ne pas supposer que ces outils de lecture reproduisent le système de fichiers du jeu.
5. **Services.** Ajouter audio, entrées, sauvegardes, fichiers et interfaces GDK par scénarios de test indépendants, en comparant les erreurs et les effets observables à une référence appropriée. Les appels réseau et services en ligne ne sont pas caractérisés par les fichiers étudiés.
6. **Si un exécutable approprié devient disponible**, commencer par son inventaire statique et son empreinte, puis les rapports et essais contrôlés. Relever la première divergence concrète, ajouter un test réduit et une correction, puis répéter. La disponibilité d'un exécutable ne garantit pas la compatibilité.

## Commandes locales utiles

Depuis la racine du paquet :

```powershell
python .\analyse_paquet.py inventaire .
python .\inspect_etpf.py .\data\ttdb\graphics\ttdb_graphics.ttpf --debut 485 --limite 1
python .\stats_etpf.py .
```

Depuis la racine AnyXboxX :

```powershell
python -m pytest
python PrototypeScarlett\scarlett.py --rapport ..
```

Ces commandes d'inspection ne lancent pas l'exécutable du jeu. Les tests du dépôt exécutent leurs propres programmes et fixtures de test.

Pour les ressources, les hypothèses d'arrondi de taille du document ETPF sont vérifiables automatiquement mais ne constituent pas un décodeur d'images. La plupart des blocs dont le bit 29 est inactif sont plus petits sur disque que leur taille logique ; copier leurs octets ne suffit pas. Traiter leur transformation comme inconnue tant qu'un décodage complet et des sorties vérifiables n'ont pas été obtenus.

Les essais de continuité spatiale documentés dans [ANALYSE_ETPF.md](paquet/ANALYSE_ETPF.md) rendent des parcours Morton candidats pour certaines petites ressources à quatre octets par pixel ; les résultats varient selon les blocs et ne prouvent pas une disposition universelle. Pour une implémentation, chercher une image de référence connue et ajouter un test avec dimensions, octets et résultat attendus avant d'adopter une transformation.

Les tailles des derniers blocs de la plupart des groupes de trois ou quatre suivent une progression par quatre. Elles sont compatibles avec plusieurs échelles d'une texture, mais le contenu du premier bloc reste inconnu et des exceptions existent. Ne pas déduire une table de mipmaps complète de ces seules tailles ; conserver les bornes des blocs et tester les hypothèses sur des ressources de référence.

Pour une catégorie précise de 153 groupes de dimensions candidates 512 × 512, des comparaisons de contenu suggèrent que le premier bloc réunit de petites résolutions dans cinq intervalles séparés par des zéros. Le test `comparer_niveaux_etpf.py` et ses hypothèses sont décrits dans [l'analyse ETPF](paquet/ANALYSE_ETPF.md) ; il demande NumPy. Cette observation n'établit pas une disposition valable pour les autres catégories.

La comparaison de 16 agencements pour ces 153 groupes donne le meilleur score à des tuiles Morton 128 dans 142 cas ; 11 groupes favorisent des variantes linéaires. Utiliser ce résultat comme piste de test, pas comme conversion universelle. Le contrôle des indices sur des données synthétiques confirme seulement que l'outil applique des permutations valides.

L'index ETPF comprend aussi une table candidate de valeurs 64 bits triées. Des comparaisons entre fichiers appuient leur association aux lignes de même rang, mais trois paires de valeurs identiques ont donné des contenus zlib différents. Si un futur lecteur les utilise pour la recherche, conserver le contexte du fichier et ne pas dédupliquer les ressources uniquement sur cette valeur. La commande `python ..\comparer_cles_etpf.py .. --comparer-blocs` reproduit les comparaisons depuis la racine AnyXboxX.

Pour examiner une valeur dans un fichier précis, `inspect_etpf.py --cle` fournit son rang et la ligne candidate correspondante, après vérification de l'ordre de la table. Voir l'exemple et les limites dans l'analyse ETPF ; cet outil ne met pas encore en œuvre le chargement de ressources du jeu.

La documentation publique de Microsoft décrit des étapes de [décompression et réarrangement DirectStorage sur Xbox Series](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/console/storage/directstorage/directstorage-overview), dont BCPack. Cela justifie d'évaluer ces possibilités pour les textures, sans attribuer un codec aux blocs de ce paquet avant vérification indépendante.
