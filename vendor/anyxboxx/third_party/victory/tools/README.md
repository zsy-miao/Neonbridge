# Neptune test tools (zm0kie)

- `start-neptune.ps1`: launcher with Neptune's switch set plus our options
  (`-Built`, `-Stats -Tag X`, `-AllNew`, `-Ocean`, `-DescData`, `-Off ...`, ...).
  Run from `D:\Neptune-VE`; it expects the game at `D:\ProjectAmerica`.
- `memwatch.ps1`: samples process/system memory, paging and VRAM every 2 s
  while `victory.exe` runs (started by the launcher with `-Stats`).

Branches: `test/combo-water` is the integration branch; `opt/*` and `ram/*`
hold the single features; the root commit is Neptune's source as shipped.
