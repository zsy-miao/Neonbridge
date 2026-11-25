# Credits

Neonbridge is a launcher around other people's work. Each vendored tree under `vendor/` is copied unchanged from its upstream at the hash in its `UPSTREAM` file (see its `VENDORED.md`), keeps its own licence, and all credit for it goes to its authors. Authors are listed as their upstream history names them.

## Vendored runtimes

| Tree | Project | Authors | Licence | Upstream hash |
|---|---|---|---|---|
| `vendor/scarlettport` | [ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort) | Ziggy (ZigZagBoi1), Vlad Bondarenko (vladbondarenko97), grooving6; its `runtime/victory` is the VIctory authors' and its `runtime/xcl` the xcl authors' | GPL-3.0-or-later; `runtime/xcl` GPL-2.0-only (its `LICENSE.md`) | `925f90a17b5897e34a4d8d3ce5e628d1dca08904` |
| `vendor/victory` | [VIctory-fxceless](https://github.com/fxceless/victory-fork) | fxceless, Marvis (Marvisak), Duff, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `085ee7ca9b635bcc345207a73ccb4c4fceb8228c` |
| `vendor/vitoriabr` | [VItoriaBr](https://github.com/uiliannn/VItoriaBr) | uiliannn, fxceless, Marvis (Marvisak), Duff and the VIctory contributors | GPL-3.0 | `9ceb7e71e10c736ed940cc454c75f46dc8369073` |
| `vendor/ztrx-victory` (archived, in git history) | [ztrx](https://github.com/Zettrox/ztrx) | Zettrox, Marvis (Marvisak), Duff, fxceless, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `90b529f878a470fec9bf07b7be316bc769a12232` |
| `vendor/xcl` | [xcl](https://github.com/ivall/xcl) | ivall, bmjask, gefydev, raylitebull and the xcl contributors | GPL-2.0-only | `6d12f5846f5fe99cc71c0bd1e890aa2c9a8f9109` |
| `vendor/flamingo` | [Flamingo](https://github.com/realblastex/flamingo) | realblastex and the VIctory contributors | GPL-3.0 | `866d277917bb572d9f9c8b464e6f24fec1b2572d` |
| `vendor/klbf` | [KLBF Lab](https://github.com/PastiiJay/klbf-lab) | PastiiJay and the VIctory contributors | GPL-3.0 | `39b99cd66faafe6701d86090d5e7e53957f5c88c` |
| `vendor/belobug` | [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) | Belobug and the VIctory contributors | GPL-3.0 | `000ffdd4ee0ff085596b798958bce534a69b4f07` |

All of the VIctory forks build on [VIctory](https://github.com/Marvisak/VIctory) by Marvisak and its contributors.

## Neonbridge's own build (`neonbridge-own`)

- `vendor/neonbridge-own`: [TuxBox](https://github.com/brcly/TuxBox) by brcly at `938c402`, with Neonbridge's runtime fixes on top. `runtime/dxbc` is Apache-2.0 (`vendor/neonbridge-own/LICENSE`); the rest of TuxBox ships no top-level licence.
- `vendor/ztrx/victory-shader`: the victory-shader translator from VItoriaBr `2b3c339` (ztrx and VIctory lineage), GPL-3.0-or-later, used by Neonbridge's shader path.
- Code and ideas in Neonbridge's own sources are credited in the code where they are used: [VIctory](https://github.com/Marvisak/VIctory) (Marvisak), [xcl](https://github.com/ivall/xcl) (ivall), [Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) (R0Y00), [xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (omonkeyz), [OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett) (californiaAlmonds), [rebox](https://github.com/puffnfresh/rebox) (puffnfresh, GPL-2.0-only), [palmetto](https://github.com/luizwidmer/palmetto) (luizwidmer), [leonida-ce](https://github.com/IAmNotSiob/leonida-ce) (IAmNotSiob), [five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp) (ThatRecompDev; xshader's DXIL writer is ported from theirs), [KestrelX](https://github.com/Batushn/KestrelX) (Batushn), [GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest) (OnlyMoisties), [Leonida](https://github.com/ubiquitarianism/leonida) (ubiquitarianism), and Microsoft (GDK headers, dxc, the D3D12 Agility SDK).
