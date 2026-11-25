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

| Camada | O que guarda | Onde fica | Código |
|---|---|---|---|
| Cache de shaders traduzidos | O DXIL de cada shader do console | `cache_dir\shaders` | `crates/victory-gpu/src/shader_cache.rs` |
| Pipeline Library do D3D12 | O PSO já compilado pelo driver | `cache_dir\pipelines` | `crates/victory-gpu/src/pipeline/library.rs` |
| Lista "early" | Quais pipelines a execução anterior usou | `cache_dir\pipelines` | `crates/victory-gpu/src/pipeline/make.rs` |

Na prática, **a primeira execução servia de pré-compilação**: ela enchia os caches, e a segunda já
encontrava quase tudo pronto. O comando `victory translate` também traduz todos os shaders, mas só
gera um **relatório**: não grava nada no cache que o jogo usa.

### A chave do cache de shaders

Uma entrada só é reaproveitada se a chave for idêntica. A chave é o BLAKE3 de:

```
[impressão digital do build (32 bytes)] + [bytes do shader do console]
```

A impressão digital do build (`build()` em `shader_cache.rs`) inclui:
- o hash do próprio `victory.exe`. **Qualquer recompilação invalida o cache inteiro.**
- `VICTORY_SHADER_LOOP_LIMIT`, `VICTORY_WAVE_WIDE`, `VICTORY_WAVE_DEEP_SKIPS`, `VICTORY_WAVE_OWN_BITS`,
  `VICTORY_F16_NEAREST`, as passadas de limpeza e o modo das barreiras;
- o limite do heap de views da GPU, descoberto ao criar o device D3D12;
- **(novo)** a maior wave da GPU (32 na NVIDIA). Veja a [seção 5](#5-correção-incluída-a-wave-da-gpu-na-chave).

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

Na segunda vez que rodar, com o mesmo exe e as mesmas configurações, ele deve mostrar quase tudo
como `already cached`.

> Os números acima são ilustrativos. Os reais dependem da sua máquina.

### Regra de ouro

**Rode o `warm` com o mesmo `victory.exe` e as mesmas variáveis `VICTORY_*` que vai usar para jogar.**
Um build `diag` é outro executável, então tem outro cache. Mudou o código e recompilou? Rode o
`warm` de novo.

---

## 4. O que `victory warm` NÃO cobre

| Caso | Por quê | O que acontece |
|---|---|---|
| Shaders gerados ou carregados só em memória | Não estão nos arquivos | Continuam sendo traduzidos durante o jogo |
| Compilação do PSO no driver | Depende do estado gráfico (formatos, blend, layout), que só o jogo informa | Continua na 1ª execução; depois fica na Pipeline Library |
| Jogo que passa um `size` diferente do tamanho do container | A chave muda | Cache miss: o shader é traduzido normalmente, sem erro |
| Troca de driver ou de GPU | A Pipeline Library recusa a biblioteca antiga | Os PSOs são recompilados; o cache de shaders vale se a largura de wave e o limite de views forem os mesmos |

Ou seja, o `warm` elimina a **tradução** na primeira execução. A **compilação no driver** continua
acontecendo uma vez.

---

## 5. Correção incluída: a wave da GPU na chave

A tradução muda conforme a GPU: em placas com wave de no máximo 32 lanes (NVIDIA), programas wave64
são traduzidos de outro jeito (`set_host_max_wave`, chamado ao criar o device). Esse valor **não
estava na impressão digital do cache**. Quem trocasse uma placa AMD por uma NVIDIA, ou o contrário,
recebia traduções feitas para a outra placa.

Agora `build()` inclui `victory_shader::emit::host_max_wave()`. O efeito colateral é que os caches
existentes são invalidados uma vez, o que já acontece a cada recompilação de qualquer forma.

---

## 6. Como conferir se o cache está funcionando

A execução normal já registra no log:

| Linha do log | Campos | Como ler |
|---|---|---|
| `shaders from the cache and translated so far` (a partir do frame 256) | `hits`, `misses` | Depois do `warm`, `misses` deve ficar perto de zero, sobrando só os shaders gerados em memória |
| `host pipelines all made` / `host pipeline library saved` | `made`, `loaded`, `started_early` | Na 2ª execução, `loaded` e `started_early` altos e `made` baixo |
| `shader translated` (por shader) | `hash` | Shader traduzido na hora, ou seja, um miss |

Se `misses` continuar alto depois do `warm`, a causa mais provável é uma destas:
1. o `warm` rodou com outro exe ou com outras variáveis `VICTORY_*`;
2. o jogo carrega aqueles shaders de outro lugar (memória, arquivo compactado que a busca não abre);
3. o jogo passa um `size` diferente do tamanho do container.

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

# 4. Jogar e olhar o log a partir do frame 256
.\target\release\victory.exe run <pasta_do_jogo>\<executavel> --execute
#    procure: "shaders from the cache and translated so far" hits=... misses=...
```

**O teste passa se:** a 2ª execução do `warm` traduzir ~0 shaders, e no jogo `misses` for uma
fração pequena de `hits`. Compare com uma execução de cache vazio sem `warm`: ali `misses` é alto.

---

## Arquivos alterados

| Arquivo | Mudança |
|---|---|
| `apps/victory/src/warm.rs` | Novo comando `victory warm` |
| `apps/victory/src/main.rs` | Registra o subcomando `Warm` |
| `crates/victory-gpu/src/shader_cache.rs` | `warm()`; `cached()` informa se foi hit ou miss (`Outcome`); wave da GPU na impressão digital; testes atualizados |
| `crates/victory-hle/src/modules/d3d12x.rs` | `warm_shader()`, a ponte entre o app e o `victory-gpu` |
| `crates/victory-shader/src/dxbc.rs` | `Container::bytes_of()` + teste |
| `crates/victory-shader/src/emit/{lower,mod}.rs` | `host_max_wave()` público |
| `README.md`, `docs/architecture.md` | Mencionam o `victory warm` |
