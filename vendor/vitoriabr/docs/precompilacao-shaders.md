# Pré-compilação de shaders: `victory warm`

Este documento explica como o VItoriaBr guarda shaders entre execuções, como conferir se o cache está
funcionando e o que o comando `victory warm` acrescenta.

> **Estado:** o código de `victory warm` foi escrito fora do Windows e **ainda não foi compilado
> nem testado no Windows**. As partes portáveis (`Container::bytes_of`, `host_max_wave`) têm teste e
> passam no Linux. Siga a seção [Como testar](#como-testar-no-windows) antes de confiar nos números.

---

## 1. O problema

Os shaders de um jogo de Xbox vêm compilados para a GPU do console (GFX10/RDNA 2). O VItoriaBr
precisa **traduzir cada um para DXIL** quando o jogo cria um pipeline com ele. Depois disso, o
**driver da GPU** ainda compila o pipeline (PSO).

As duas etapas custam caro:
- a tradução dos maiores compute shaders leva ~0,4 s cada;
- algumas compilações no driver passam de 3 s;
- o primeiro frame do mundo 3D pede mais de 1.200 pipelines novos.

Para o jogo não travar, isso roda em threads separadas, com um limite de **500 ms de espera por
frame** (`VICTORY_PIPELINE_WAIT_MS`). Passado o limite, **o draw é descartado**: o objeto some
naquele frame. Na primeira execução, isso aparece como pop-in, objetos faltando e queda de fps.

---

## 2. O que já existia no repositório

O VItoriaBr mantém duas camadas independentes de persistência (além da lista de pré-aquecimento de
pipelines):

| Camada | O que guarda | Onde fica | Código |
|---|---|---|---|
| Cache de traduções DXIL | Cada `HostShader` traduzido (bytecode DXIL + metadados de binding) | `cache_dir\shaders` | `crates/victory-gpu/src/shader_cache.rs` |
| Pipeline Library do D3D12 | PSOs já compilados pelo driver (`host-pipelines.bin`) | `cache_dir\pipelines` | `crates/victory-gpu/src/pipeline/library.rs` |
| Lista "early" | Chaves dos pipelines usados na execução anterior (`early-pipelines.txt`) | `cache_dir\pipelines` | `crates/victory-gpu/src/pipeline/library.rs`, `crates/victory-gpu/src/pipeline/make.rs` |

Na prática, **a primeira execução servia de pré-compilação**: ela enchia o cache de traduções DXIL e
a Pipeline Library de PSOs, e a segunda já encontrava quase tudo pronto.

Quanto aos comandos offline:
- `victory translate` (sem `--warm`) traduz todos os shaders encontrados, mas só gera um
  **relatório** (opcionalmente validando com `--validate` ou testando criação de pipelines com
  `--pipelines`): não grava nada em `cache_dir\shaders`;
- `victory translate --warm <arquivo|all>` **grava sim** no mesmo `cache_dir\shaders` usado pelo
  jogo (via `victory_gpu::shader_cache::warm`), permitindo aquecer apenas os shaders listados por
  hash de 16 dígitos hexadecimais em um arquivo de log — ou todos (`all`) — com suporte a limite de
  tempo (`VICTORY_WARM_SECONDS=<n>`) e 4 threads por padrão (`--jobs N`). Diferente de `victory warm`,
  ele não aplica o recorte `Container::bytes_of` em containers DXBC soltos.

Nenhum comando offline compila nem salva PSOs em `cache_dir\pipelines`.

### A chave do cache de shaders (traduções DXIL)

Uma entrada em `cache_dir\shaders` só é reaproveitada se a chave for idêntica. O cache endereçado
por conteúdo (`victory-pipeline`, conversor `"shader.dxil"` v1) calcula o BLAKE3 sobre o cabeçalho
do conversor e:

```
[impressão digital da tradução (32 bytes)] + [bytes do container do shader do console]
```

Por padrão, a impressão digital da tradução (`build()` em `crates/victory-gpu/src/shader_cache.rs`)
**não** usa o hash do executável `victory.exe`. Ela combina o prefixo de versão da chave
(`victory shader cache key 2`), o código de `crates/victory-gpu/src/shader_cache.rs` (que define a
codificação das entradas) e `victory_shader::emit::translation_fingerprint()`
(`crates/victory-shader/src/emit/mod.rs`), que inclui:
- o hash em tempo de compilação dos fontes em `crates/victory-shader/src/`
  (`VICTORY_SHADER_SOURCE_HASH`, gerado por `crates/victory-shader/build.rs`);
- as flags de compilação `cfg!(feature = "diagnostics")` e `cfg!(debug_assertions)` (`debug` vs
  `release`);
- o valor em tempo de execução de todas as variáveis `VICTORY_*` lidas por
  `crates/victory-shader/src/` (`VICTORY_SHADER_SWITCHES`, coletadas por `build.rs` — incluindo
  switches como `VICTORY_WAVE_WIDE`, `VICTORY_WAVE_DEEP_SKIPS`, `VICTORY_WAVE_OWN_BITS`,
  `VICTORY_WAVE_LOCAL`, `VICTORY_F16_NEAREST`, `VICTORY_FIRST_HALF`, `VICTORY_FIRSTLANE_OWN`,
  `VICTORY_SHADER_LOOP_LIMIT`, `VICTORY_GROUP_SYNC`, `VICTORY_SHADER_OPT` etc.);
- os valores resolvidos de `loop_limit()`, `group_sync()` e `shader_opt()`;
- as capacidades relevantes da GPU do host descobertas ao criar o device D3D12:
  - o limite do heap de views (`view_heap_limit()`, sondado por `probe_view_limit()`);
  - a maior largura de wave da GPU (`host_max_wave()`, ex.: 32 na NVIDIA). Veja a
    [seção 5](#5-correção-incluída-a-wave-da-gpu-na-chave).

A impressão digital é recalculada a cada consulta, evitando gravar traduções feitas antes de o
device informar suas capacidades sob uma chave posterior.

**O que invalida e o que reaproveita o cache de traduções DXIL:**
- **Recompilar o `victory.exe`, por si só, não invalida o cache.** Alterações fora de
  `crates/victory-shader/src/` e `crates/victory-gpu/src/shader_cache.rs` (por exemplo em HLE,
  loader ou partes de `victory-gpu` fora de `shader_cache.rs`) reaproveitam as traduções DXIL já
  gravadas, desde que o perfil/features (`debug`/`release` e `diagnostics`), os switches
  `VICTORY_*` lidos pelo tradutor e as capacidades relevantes da GPU (`host_max_wave`,
  `view_heap_limit`) permaneçam iguais.
- **O cache é invalidado** quando mudam os fontes do tradutor (`crates/victory-shader/src/`), o
  formato em `shader_cache.rs`, o modo de build (`debug_assertions` ou `feature = "diagnostics"`),
  alguma variável `VICTORY_*` lida pelo tradutor ou as capacidades relevantes do host
  (`host_max_wave` / `view_heap_limit`).
- **`VICTORY_SHADER_CACHE_KEY=exe`**: restaura opcionalmente o comportamento antigo, acrescentando o
  BLAKE3 do executável (`current_exe`) à chave em `build()`. Somente com essa variável ativa é que
  qualquer recompilação do binário isola o cache inteiro para aquele executável específico (se o
  binário não puder ser lido, o VItoriaBr registra aviso e volta à chave baseada no tradutor).

### Como a Pipeline Library de PSOs difere

A camada `cache_dir\pipelines\host-pipelines.bin` é separada do cache DXIL: ela guarda PSOs já
compilados pelo driver D3D12, identificados em `library::name()` pelo hash da descrição no host
(tipo do pipeline, root signature serializada, bytes dos shaders traduzidos e estado gráfico).
Recompilar o executável sem mudar o DXIL traduzido, a root signature nem o estado do pipeline também
não invalida a `host-pipelines.bin`; já uma atualização de driver ou troca de adaptador faz o D3D12
rejeitar a biblioteca inteira, enquanto uma mudança no tradutor ou no estado gráfico recompila
apenas os PSOs cuja descrição mudou.

---

## 3. O que `victory warm` faz

```powershell
.\target\release\victory.exe warm <pasta_do_jogo>
```

Sem argumentos, ele usa o `game_dir` e os `test_titles` do `victory.toml`. Opções:
- `--list`: imprime cada shader que não traduz;
- `--jobs N`: número de threads (o padrão usa todos os núcleos).

O comando:

1. Usa a **mesma pasta de cache** da execução normal (`cache_dir\shaders`).
2. Percorre os arquivos do jogo com a mesma busca do `victory translate`: containers DXBC soltos e
   registros SGD5.
3. Recorta cada container no tamanho que o próprio cabeçalho declara (`Container::bytes_of`). São
   esses os bytes que o jogo entrega ao criar um pipeline. Sem o recorte, a chave incluiria o resto
   do arquivo e nunca bateria com a do jogo.
4. Ignora containers repetidos.
5. Para cada container, chama `victory_gpu::shader_cache::warm`, que:
   - **cria o device D3D12 antes de tudo**, como acontece na execução. É isso que define o limite de
     views e a largura de wave, os dois valores que entram na tradução e na chave;
   - traduz e grava pelo mesmo caminho (`cached`) que a execução usa.
6. Mostra um resumo:

```
shader cache D:\victory-data\cache\shaders
containers   25758 distinct: 25629 translated now, 0 already cached, 129 do not translate (412.3 s)
     ...     <motivo>  (e.g. <arquivo>@<offset>)
```

Na segunda vez que rodar — mantendo o tradutor, o modo de build (`release`/`debug` e `diagnostics`),
as capacidades relevantes do host e as variáveis `VICTORY_*` lidas pelo tradutor (ou o mesmo
executável, se `VICTORY_SHADER_CACHE_KEY=exe` estiver definido) — ele deve mostrar quase tudo como
`already cached`.

> Os números acima são ilustrativos. Os reais dependem da sua máquina.

### Regra de ouro

**Rode o `warm` com o mesmo tradutor, o mesmo perfil/features de build (`release` vs `debug`, com ou
sem `diagnostics`), a mesma GPU (mesmos `host_max_wave` e `view_heap_limit`) e as mesmas variáveis
`VICTORY_*` lidas pelo tradutor que vai usar para jogar.**

- Um build `diagnostics` (ou `debug`) usa outro cache não por ser outro `.exe`, mas porque
  `cfg!(feature = "diagnostics")` e `cfg!(debug_assertions)` entram na impressão digital do
  tradutor.
- Mudou código fora de `crates/victory-shader/src/` e `crates/victory-gpu/src/shader_cache.rs` e
  recompilou no mesmo perfil/features? **Não precisa rodar o `warm` de novo** — as traduções DXIL
  continuam válidas.
- Só é necessário reaquecer quando alterar o tradutor (`crates/victory-shader/src/`),
  `shader_cache.rs`, o perfil/features de compilação, algum switch `VICTORY_*` lido pelo tradutor,
  mudar para uma GPU com wave/view limit diferente, ou quando usar `VICTORY_SHADER_CACHE_KEY=exe`
  (caso em que cada novo executável exige seu próprio cache).
- *Nota:* a mensagem impressa no console pelo `victory warm` (`apps/victory/src/warm.rs`) ainda
  exibe um aviso conservador citando `this victory.exe`, mas a chave efetiva em `shader_cache.rs`
  só vincula o cache ao executável quando `VICTORY_SHADER_CACHE_KEY=exe` está definido.

---

## 4. O que `victory warm` NÃO cobre

O alcance de `victory warm` é **exclusivamente o cache de traduções DXIL (`cache_dir\shaders`)**
para containers encontrados nos arquivos do jogo. Ele não cria PSOs no driver nem preenche
`cache_dir\pipelines\host-pipelines.bin` ou `early-pipelines.txt`.

| Caso | Por quê | O que acontece |
|---|---|---|
| Shaders gerados ou carregados só em memória | Não estão nos arquivos | Continuam sendo traduzidos durante o jogo |
| Compilação do PSO no driver | Depende do estado gráfico (root signature, formatos, blend, layout), que só o jogo informa | Continua na 1ª execução; depois fica na Pipeline Library (`host-pipelines.bin`) e na lista `early-pipelines.txt` |
| Jogo que passa um `size` diferente do tamanho do container | A chave muda | Cache miss: o shader é traduzido normalmente, sem erro |
| Troca de driver ou de GPU | A Pipeline Library recusa a biblioteca antiga | Os PSOs são recompilados; o cache de traduções DXIL continua válido se `host_max_wave` e `view_heap_limit` forem os mesmos |

Ou seja, o `warm` elimina a **tradução para DXIL** dos shaders presentes nos arquivos já na primeira
execução. A **compilação do PSO no driver** continua acontecendo na primeira execução e passa a ser
reaproveitada a partir da segunda via Pipeline Library.

---

## 5. Correção incluída: a wave da GPU na chave

A tradução muda conforme a GPU: em placas com wave de no máximo 32 lanes (NVIDIA), programas wave64
são traduzidos de outro jeito (`set_host_max_wave`, chamado ao criar o device). Esse valor **não
estava na impressão digital do cache**. Quem trocasse uma placa AMD por uma NVIDIA, ou o contrário,
recebia traduções feitas para a outra placa.

No código atual, `build()` delega para `victory_shader::emit::translation_fingerprint()`, que inclui
`lower::host_max_wave()` (além de `view_heap_limit()`, do hash dos fontes de `victory-shader`, das
flags de build e dos switches do tradutor) e é avaliada a cada consulta. Alterações nesse conjunto
(ou troca para uma GPU com largura máxima de wave diferente) invalidam as entradas antigas, mas uma
simples recompilação do executável que não altere o tradutor, `shader_cache.rs`, o perfil/features
ou os switches não invalida o cache (salvo com `VICTORY_SHADER_CACHE_KEY=exe`).

---

## 6. Como conferir se o cache está funcionando

A verificação nos logs deve ser feita **separadamente para cada camada**:

### Camada 1: cache de traduções DXIL (`cache_dir\shaders`)

| Linha do log (nível) | Campos | Significado e como ler |
|---|---|---|
| `shaders from the cache and translated so far` (`INFO`, em frames potência de 2 a partir do frame 256: `256`, `512`, `1024`, ...) | `hits`, `misses` | Contadores acumulados desde o início da execução (`crates/victory-gpu/src/queue.rs` / `shader_cache.rs`). `hits` = traduções lidas do disco; `misses` = shaders traduzidos na hora (incluindo entradas ilegíveis retraduzidas). Depois do `warm`, `hits` deve dominar e `misses` ficar perto de zero |
| `host wave lanes` (`INFO`, ao criar o device) | `min`, `max` | Largura mínima e máxima de wave da GPU (`crates/victory-gpu/src/host.rs`); `max` entra na chave como `host_max_wave()` |
| `host view heaps hold <views> views; ...` (`INFO`, na sondagem do heap) | `views` | Capacidade sondada do heap de views (`crates/victory-gpu/src/descriptor/host.rs`); `views - 1` entra na chave como `view_heap_limit()` |
| `shader translated` (`INFO`, 1x por shader distinto na execução) | `stage`, `hash`, `user_data`, `samplers` | **Não indica cache miss.** É registrado em `crates/victory-gpu/src/pipeline.rs` assim que `host_shader()` retorna o shader traduzido, seja ele um **hit do cache em disco** ou um **miss traduzido na hora**. Ver muitas linhas `shader translated` após o `warm` é normal |
| `shader from the cache` / `shader translated and cached` / `unreadable shader cache entry; translating again` (`DEBUG`, por shader) | `path` | Distinção por shader individual entre hit, miss gravado e entrada corrompida (`crates/victory-gpu/src/shader_cache.rs`). Só aparece em builds com `--features diagnostics` (ex.: `VICTORY_LOG=victory_gpu::shader_cache=debug,info`), pois builds `release` padrão limitam o log a `INFO`; lembre que o build `diagnostics` usa chave própria |

Antes mesmo de abrir o jogo, rodar `victory warm` uma segunda vez confirma o reaproveitamento do
cache DXIL no resumo do terminal (`0 translated now` e quase tudo em `already cached`).

Se `misses` continuar alto em `shaders from the cache and translated so far` depois do `warm`, a
causa mais provável é uma destas:
1. o `warm` rodou com outro tradutor (`crates/victory-shader/src/` ou `shader_cache.rs`), outro
   perfil/feature de build (`debug` vs `release`, com/sem `diagnostics`), outras variáveis
   `VICTORY_*` lidas pelo tradutor, outro `cache_dir`, outra GPU (`host_max_wave` /
   `view_heap_limit`), ou outro executável com `VICTORY_SHADER_CACHE_KEY=exe` ativo;
2. o jogo carrega aqueles shaders de outro lugar (memória, arquivo compactado que a busca não abre);
3. o jogo passa um `size` diferente do tamanho do container.

### Camada 2: biblioteca de PSOs do driver (`cache_dir\pipelines`)

| Linha do log (nível) | Campos | Significado e como ler |
|---|---|---|
| `host pipeline library loaded` (`INFO`, ao abrir `host-pipelines.bin`) | `bytes`, `mapped` | Confirma que o driver aceitou a biblioteca salva (`bytes` = tamanho do arquivo; `mapped` = mapeado em memória via `MapViewOfFile`). Sem biblioteca prévia, o log traz `no host pipeline library yet; starting an empty one`; se o driver/GPU mudou ou o arquivo foi rejeitado, traz `host pipeline library rejected; starting an empty one` (`error`) |
| `title pipelines to start early` (`INFO`, ao ler `early-pipelines.txt`) | `pipelines` | Quantidade de chaves de pipelines de execuções anteriores carregadas para início antecipado na criação |
| `host pipelines all made` (`INFO`, quando os workers esvaziam um lote com pico `>= 10`) | `made`, `loaded`, `started_early` | Contadores acumulados na execução (`crates/victory-gpu/src/pipeline/make.rs`): `made` = PSOs compilados pelo driver agora (miss na biblioteca); `loaded` = PSOs carregados de `host-pipelines.bin` (hit); `started_early` = pipelines enfileirados antecipadamente via `early-pipelines.txt` |
| `host pipeline library saved` (`INFO`, a cada ~20 s quando há mudanças e no fim da execução) | `bytes`, `early`, `made`, `loaded`, `started_early` | Além dos mesmos contadores acumulados (`made`, `loaded`, `started_early`), informa `bytes` serializados em `host-pipelines.bin` (`0` quando nenhum PSO novo foi adicionado e só `early-pipelines.txt` foi atualizado) e `early` (total de chaves na lista early) |

Na **1ª execução** (mesmo após `victory warm`), é esperado ver
`no host pipeline library yet; starting an empty one`, com `loaded=0` e `made` subindo, porque o
`warm` não compila PSOs. A partir da **2ª execução** (mesma GPU/driver e mesma tradução DXIL), o
reaproveitamento de PSOs aparece como `host pipeline library loaded`, `loaded` e `started_early`
altos e `made` baixo (restrito a pipelines inéditos da sessão).

---

## Como testar no Windows

```powershell
# 1. Compilar e rodar os testes
cargo build --release -p victory
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# 2. Começar com o cache vazio (guarde uma cópia se quiser)
Remove-Item -Recurse "$env:LOCALAPPDATA\victory\cache\shaders"   # ou o seu cache_dir

# 3. Pré-traduzir duas vezes: a 2ª deve dizer "already cached" para quase tudo
.\target\release\victory.exe warm <pasta_do_jogo>
.\target\release\victory.exe warm <pasta_do_jogo>

# 4. Jogar e olhar o log a partir do frame 256 (camada DXIL) e na 2ª execução (camada PSO)
.\target\release\victory.exe run <pasta_do_jogo>\<executavel> --execute
#    cache DXIL: procure "shaders from the cache and translated so far" hits=... misses=...
#    cache PSO (2ª execução): procure "host pipeline library loaded" e
#    "host pipelines all made" / "host pipeline library saved" (loaded=..., made=..., started_early=...)
```

**O teste passa se:** a 2ª execução do `warm` traduzir ~0 shaders, e no jogo `misses` for uma
fração pequena de `hits` em `shaders from the cache and translated so far` (independentemente de
aparecerem linhas `shader translated`). Compare com uma execução de cache vazio sem `warm`: ali
`misses` é alto. A partir da 2ª execução do título, `host pipeline library loaded` e `loaded` alto /
`made` baixo confirmam o reaproveitamento na camada de PSOs.

---

## Arquivos relevantes

| Arquivo | Papel |
|---|---|
| `apps/victory/src/warm.rs` | Subcomando `victory warm` (pré-tradução de containers com `Container::bytes_of`) |
| `apps/victory/src/translate.rs` | `victory translate` (relatório) e `victory translate --warm <arquivo>` / `all` (aquecimento seletivo/total) |
| `apps/victory/src/main.rs` | Registra os subcomandos `Warm` e `Translate` |
| `crates/victory-gpu/src/shader_cache.rs` | `build()` (chave com `translation_fingerprint()` + `VICTORY_SHADER_CACHE_KEY=exe` opcional), `warm()`, `cached()` (`Outcome`), contadores `hits`/`misses` e logs `DEBUG` |
| `crates/victory-shader/src/emit/mod.rs`, `crates/victory-shader/build.rs` | `translation_fingerprint()`, `host_max_wave()`, `VICTORY_SHADER_SOURCE_HASH` e `VICTORY_SHADER_SWITCHES` |
| `crates/victory-gpu/src/pipeline/library.rs`, `crates/victory-gpu/src/pipeline/make.rs` | Pipeline Library (`host-pipelines.bin`), lista `early-pipelines.txt` e logs `made`/`loaded`/`started_early` |
| `crates/victory-gpu/src/queue.rs`, `crates/victory-gpu/src/pipeline.rs` | Logs `shaders from the cache and translated so far` e `shader translated` |
| `crates/victory-hle/src/modules/d3d12x.rs` | `warm_shader()`, a ponte entre o app e o `victory-gpu` |
| `crates/victory-shader/src/dxbc.rs` | `Container::bytes_of()` + teste |
| `README.md`, `docs/architecture.md` | Mencionam o `victory warm` |
