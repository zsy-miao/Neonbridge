# Module Task de libHttpClient (MIT)

Implémentation de référence de `XTaskQueue` et `XAsync` publiée par Microsoft sous licence MIT (voir `LICENSE.md`), reprise sans modification depuis :

- dépôt : https://github.com/microsoft/libHttpClient
- commit : `c5007fd9cbdc785518ea6dccc5751b125d451cc1` (2026-11-20)
- fichiers : `Include/XTaskQueue.h`, `XAsync.h`, `XAsyncProvider.h`, `Source/Task/*` (variantes Windows), `Source/Common/ResultMacros.h`

Seul `pch.h` est propre à AnyXboxX : il remplace l'en-tête précompilé de libHttpClient par le strict nécessaire au module Task.
