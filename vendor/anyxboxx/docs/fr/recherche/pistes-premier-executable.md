> English: [first-executable-leads.md](../../research/first-executable-leads.md)

# Pistes pour le premier exécutable Scarlett

État au 22 novembre 2026. Cette note accompagne le [plan de compatibilité](plan-compatibilite-paquet.md). Elle ne contient ni binaire ni extrait de ressource du jeu. La provenance du paquet étudié n'est pas authentifiée.

## Point de départ vérifié

Les deux manifestes du paquet désignent `game_scarlett_master.exe`, que le layout place dans le chunk 1001. Le fichier est absent de sa destination et la recherche récursive locale par nom exact ne l'a pas trouvé. Cela n'exclut pas un fichier renommé ou dans un conteneur. Depuis la racine d'AnyXboxX, `python PrototypeScarlett/scarlett.py --rapport ..` échoue actuellement en signalant l'exécutable absent. Ses imports, ses chemins d'exécution et ses shaders effectivement utilisés restent inconnus.

L'[avancée](../avancee.md) rapporte l'initialisation isolée de 16 DLL du paquet, avec 876 imports résolus sur 1 337 dans la configuration mesurée. Cette mesure porte sur leurs tables d'imports et leur initialisation, pas sur l'exécutable manquant ni sur toutes les fonctions des DLL. Les démonstrations PC ne prouvent pas qu'un jeu Scarlett fonctionne.

## Diagnostic quand un exécutable sera disponible

1. Vérifier d'abord sa provenance, sa version et son empreinte ; comparer sa configuration au paquet, sans considérer le nom comme une preuve. Archiver localement la commande, l'empreinte et la version du projet pour rendre le diagnostic reproductible.
2. Faire un inventaire **statique** du PE : machine, sections, point d'entrée, TLS et imports. `tools/jour_j.py` fait cet inventaire mais lance ensuite aussi des diagnostics dans Unicorn : ce n'est pas une commande exclusivement statique. Le rapport complet reste hors du dépôt ; les règles de celui-ci interdisent d'y ajouter les fichiers du jeu.
3. Employer ensuite le parcours du [guide jour J](../jour-j.md) sur une copie appropriée. Le mode Unicorn facilite le diagnostic des appels invités, mais ne doit pas être considéré comme une garantie de confinement de code non fiable. Le mode natif exécute directement le programme avec les droits de l'utilisateur ; le réserver à un programme de confiance.
4. Consigner le **premier échec observable** : mode et versions, étape, module, appel ou instruction, arguments utiles, erreur, et si possible reproduction minimale. Distinguer absence d'export, fonction qui renvoie `E_NOTIMPL`, ABI incorrecte et résultat incorrect. Ajouter un test synthétique ou public correspondant avant toute correction.

### Limites du tableau de fournisseurs actuel

La lecture de `tools/jour_j.py`, fonctions `inventory`, `provider` et `coverage`, montre que ce tableau est un **indice statique**, pas une preuve de prise en charge :

- Un import par ordinal (`#...`) est attribué à une DLL trouvée sans vérification que cet ordinal existe réellement dans ses exports.
- Les noms `api-ms-` et `ext-ms-` sont attribués à Windows sans vérification de l'export individuel ; une DLL considérée système mais absente de `System32` peut également être attribuée à Windows par hypothèse.
- L'existence d'un export nommé ne prouve ni que son implémentation est complète, ni que ses conventions, structures et valeurs de retour sont correctes. Les imports dynamiques et différés ne sont pas couverts par la liste des imports ordinaires de `inventory`.

Avant d'utiliser le total comme critère de progression, tester ces cas sur des PE synthétiques, distinguer les résultats vérifiés des suppositions et comparer le rapport à une trace d'exécution contrôlée. Les tests `tests/test_jour_j.py` actuels couvrent un paquet synthétique simple ; ils ne démontrent pas les cas ci-dessus.

## Ressources : hypothèses à confronter au moteur

- Les recherches locales sur RDS5 et ETPF documentent des limites de blocs et un décodage zlib sur échantillon. Le traitement des autres blocs et le format RPF complet restent inconnus ; ne pas supposer que les lecteurs de recherche reproduisent les opérations de stockage du moteur.
- Sur un sous-ensemble précis, le visualiseur expérimental local interprète quatre octets par pixel et peut exporter des variantes linéaires ou Morton. Aucune image de référence n'a validé l'agencement ni l'ordre des canaux. Pour une future implémentation, fabriquer un jeu de données synthétique et comparer à une référence connue ; ne pas ajouter d'images extraites au dépôt.
- Les notes [TomBox](tombox.md) rapportent une étude externe de shaders et des limites autour des mesh shaders et des root signatures. Ce sont des observations rapportées, pas une preuve que l'exécutable futur appellera ces chemins ; établir les dépendances réelles à partir de l'exécutable et de tests autorisés.

## Vérification locale de cette note

Le 22 novembre 2026, `python -m pytest tests/test_jour_j.py tests/test_launch.py -q` a donné **9 tests réussis** avec `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1`. Sans cette variable, le plugin tiers `xonsh` installé globalement a échoué au démarrage de pytest faute de console ; aucun test n'avait alors été exécuté. Ces tests portent sur les cas synthétiques du dépôt, pas sur le jeu absent.
