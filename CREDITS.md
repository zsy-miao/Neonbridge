# Credits

Neonbridge is a launcher around other people's work. Each vendored tree under `vendor/` is copied unchanged from its upstream at the hash in its `UPSTREAM` file (see its `VENDORED.md`), keeps its own licence, and all credit for it goes to its authors. Authors are listed as their upstream history names them.

## Vendored runtimes

| Tree | Project | Authors | Licence | Upstream hash |
|---|---|---|---|---|
| `vendor/scarlettport` | [ScarlettPort](https://github.com/ZigZagBoi1/ScarlettPort) | Ziggy (ZigZagBoi1), Vlad Bondarenko (vladbondarenko97), grooving6; its `runtime/victory` is the VIctory authors' and its `runtime/xcl` the xcl authors' | GPL-3.0-or-later; `runtime/xcl` GPL-2.0-only (its `LICENSE.md`) | `29b13db5848d23755f6ff949cc9ebe592f154c3d` |
| `vendor/victory` | [VIctory-fxceless](https://github.com/fxceless/victory-fork) | fxceless, Marvis (Marvisak), Duff, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `a25370661ed1343e2f2d5e79669ec8db4e60dc68` |
| `vendor/vitoriabr` | [VItoriaBr](https://github.com/uiliannn/VItoriaBr) | uiliannn, fxceless, Marvis (Marvisak), Duff and the VIctory contributors | GPL-3.0 | `2b3c339ef155d5798312d653db0d1a4fa0eee371` |
| `vendor/ztrx-victory` | [ztrx](https://github.com/Zettrox/ztrx) | Zettrox, Marvis (Marvisak), Duff, fxceless, 9hvn9yt2vk-netizen and the VIctory contributors | GPL-3.0 | `90b529f878a470fec9bf07b7be316bc769a12232` |
| `vendor/xcl` | [xcl](https://github.com/ivall/xcl) | ivall, bmjask, gefydev, raylitebull and the xcl contributors | GPL-2.0-only | `1a0bbe3a2ba91ea8f5b6fc29130a053868d3c9e6` |

All of the VIctory forks build on [VIctory](https://github.com/Marvisak/VIctory) by Marvisak and its contributors.

## Neonbridge's own build (`neonbridge-own`)

- `vendor/neonbridge-own`: [TuxBox](https://github.com/brcly/TuxBox) by brcly at `938c402`, with Neonbridge's runtime fixes on top. `runtime/dxbc` is Apache-2.0 (`vendor/neonbridge-own/LICENSE`); the rest of TuxBox ships no top-level licence.
- `vendor/ztrx/victory-shader`: the victory-shader translator from VItoriaBr `2b3c339` (ztrx and VIctory lineage), GPL-3.0-or-later, used by Neonbridge's shader path.
- Code and ideas in Neonbridge's own sources are credited in the code where they are used: [VIctory](https://github.com/Marvisak/VIctory) (Marvisak), [xcl](https://github.com/ivall/xcl) (ivall), [Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) (R0Y00), [xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (omonkeyz), [OpenScarlett](https://github.com/californiaAlmonds/OpenScarlett) (californiaAlmonds), [rebox](https://github.com/puffnfresh/rebox) (puffnfresh, GPL-2.0-only), [palmetto](https://github.com/luizwidmer/palmetto) (luizwidmer), [leonida-ce](https://github.com/IAmNotSiob/leonida-ce) (IAmNotSiob), [five-star-recomp](https://github.com/ThatRecompDev/five-star-recomp) (ThatRecompDev; xshader's DXIL writer is ported from theirs), [KestrelX](https://github.com/Batushn/KestrelX) (Batushn), [GTA6-BootTest](https://github.com/OnlyMoisties/GTA6-BootTest) (OnlyMoisties), [Leonida](https://github.com/ubiquitarianism/leonida) (ubiquitarianism), and Microsoft (GDK headers, dxc, the D3D12 Agility SDK).
