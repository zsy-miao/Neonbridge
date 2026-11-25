# Credits

Neonbridge is a launcher around other people's work. Each vendored tree under `vendor/` is copied unchanged from its upstream at the hash in its `UPSTREAM` file (see its `VENDORED.md`), keeps its own licence, and all credit for it goes to its authors. Authors are listed as their upstream history names them.

## Vendored runtimes

| Tree | Project | Authors | Licence | Upstream hash |
|---|---|---|---|---|
| `vendor/scarlettport` | [ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort) | Ziggy (ZigZagBoi1), Vlad Bondarenko (vladbondarenko97), grooving6; its `runtime/victory` is the VIctory authors' and its `runtime/xcl` the xcl authors' | GPL-3.0-or-later; `runtime/xcl` GPL-2.0-only (its `LICENSE.md`) | `925f90a17b5897e34a4d8d3ce5e628d1dca08904` |
| `vendor/victory` | [VIctory-fxceless](https://github.com/fxceless/victory-fork) | fxceless, Marvis (Marvisak), Duff, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `9c184b35b43de3455b74ac1770e1c8734a047810` |
| `vendor/vitoriabr` | [VItoriaBr](https://github.com/uiliannn/VItoriaBr) | uiliannn, fxceless, Marvis (Marvisak), Duff and the VIctory contributors | GPL-3.0 | `081a89d2e5767beb38c94e2f4adc7eeca0ad69db` |
| `vendor/ztrx-victory` (archived, in git history) | [ztrx](https://github.com/Zettrox/ztrx) | Zettrox, Marvis (Marvisak), Duff, fxceless, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `90b529f878a470fec9bf07b7be316bc769a12232` |
| `vendor/vibrport` | [VI-BR-PORT](https://github.com/edsuuu/VI-BR-PORT) | edsuuu; built on VIctory (Marvisak) and victory-fork (fxceless) | GPL-3.0 | `96135bda29dc3048253d4ea66f02da6c60ec21a8` |
| `vendor/flamingo` | [Flamingo](https://github.com/realblastex/flamingo) | realblastex and the VIctory contributors | GPL-3.0 | `78bb903518014999110f70d51f2c7532b9980258` |
| `vendor/klbf` | [KLBF Lab](https://github.com/PastiiJay/klbf-lab) | PastiiJay and the VIctory contributors | GPL-3.0 | `94cf8a7af9ce364175774cfdd5365f6d4220ab86` |
| `vendor/belobug` | [VIctory-Belobug-AMD](https://github.com/Belobug/VIctory-Belobug-AMD) | Belobug and the VIctory contributors | GPL-3.0 | `4f62c2a5e27e7beebf3e8d68f77898e43dc7b77c` |
| `vendor/unified` | [victory-unified](https://github.com/Itsbatt/victory-unified) | Itsbatt, with fxceless, Belobug, DexNom, Zettrox and the VIctory contributors | GPL-3.0-or-later | `d5b5fca6cf3c325634f90eb897aa43d0eb665847` |
| `vendor/anyxboxx` | [AnyXboxX](https://github.com/LooperSalty/AnyXboxX) | LooperSalty and the authors in its THIRD_PARTY.md | GPL-3.0-or-later | `9a06443178ccf3dd84fbb25804e7c844a3184bd8` |

All of the VIctory forks build on [VIctory](https://github.com/Marvisak/VIctory) by Marvisak and its contributors.

## Neonbridge's own build (`neonbridge-own`)

- `vendor/neonbridge-own`: [TuxBox](https://github.com/brcly/TuxBox) by brcly at `938c402`, with Neonbridge's runtime fixes on top. `runtime/dxbc` is Apache-2.0 (`vendor/neonbridge-own/LICENSE`); the rest of TuxBox ships no top-level licence.
- `vendor/ztrx/victory-shader`: the victory-shader translator from VItoriaBr `2b3c339` (ztrx and VIctory lineage), GPL-3.0-or-later, used by Neonbridge's shader path.
- Code and ideas in Neonbridge's own sources are credited in the code where they are used: [VIctory](https://github.com/Marvisak/VIctory) (Marvisak), [xcl](https://github.com/ivall/xcl) (ivall), [Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) (R0Y00), [xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (omonkeyz), [OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett) (californiaAlmonds), [rebox](https://github.com/puffnfresh/rebox) (puffnfresh, GPL-2.0-only), [palmetto](https://github.com/luizwidmer/palmetto) (luizwidmer), [leonida-ce](https://github.com/IAmNotSiob/leonida-ce) (IAmNotSiob), [five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp) (ThatRecompDev; xshader's DXIL writer is ported from theirs), [KestrelX](https://github.com/Batushn/KestrelX) (Batushn), [GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest) (OnlyMoisties), [Leonida](https://github.com/ubiquitarianism/leonida) (ubiquitarianism), and Microsoft (GDK headers, dxc, the D3D12 Agility SDK).
