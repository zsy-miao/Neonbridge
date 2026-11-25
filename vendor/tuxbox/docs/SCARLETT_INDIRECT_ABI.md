# Independent Scarlett indirect command ABI control

The existing indirect compute guest supports a second mode:

    d3d12-indirect.exe --adapter VVVV:DDDD --scarlett

It requests Device11 at creation (not merely QueryInterface afterward),
uses list slot 61 rather than the older 59, and calls DriverHintX at
slot 101 with feature 2/value 1. It expects a zero return and then checks
that recording and dependent dispatches still work. Queue fence signal
uses slot 16 for this interface instead of 14. The default invocation
retains the earlier layout.

Both paths exercise direct count, count-buffer one/zero, four invalid
indirect calls, dependent compute and cache flushes, and compare all 64
words including guards against the CPU reference.

Local RTX 4080 / Proton Experimental, fresh provider from 95af9d3:
```
d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=59 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000
d3d12-indirect PASS pci=10de:2704 sdk=0x20004 device=41 list=61 words=64/64 counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x00000000 rays=0x00000000
```

No title execution. The synthetic control does not validate all indirect
signature layouts or graphics/mesh/ray execution.
