# The host's view heap limit

The console's descriptor heaps have no 1,000,000-entry ceiling. The host's
do: `D3D12_MAX_SHADER_VISIBLE_DESCRIPTOR_HEAP_SIZE_TIER_1` and `_TIER_2`
are 1,000,000 views, and `D3D12_MAX_SHADER_VISIBLE_SAMPLER_HEAP_SIZE` is
2,048 samplers on every tier. NVIDIA reports tier 2, so a title heap larger
than that cannot exist on the host as one heap.

`descriptor::host::HostHeap::new` creates the largest heap it can and keeps
`PORTABLE_VIEWS - DYNAMIC` of the title's slots. A sampler heap over 2,048
fails outright and `new` returns `None`, losing the heap entirely.

Reported as a device hang on the first-boot screens (#11, RTX 4060) and a
lost device with black frames in the world (#16, RTX 5070 Ti). The cap
warning in `HostHeap::new` (what the title asked for, what the host made,
what was lost) is the only piece this branch adds. `mirror` already reports
the highest title slot past the heap on `main`, and #21 remaps the root
tables that reach it, so this branch deliberately reports neither again.

## What the title does with a heap

Two different things, and they constrain any further fix differently.

### 1. Root descriptor tables

`SetGraphicsRootDescriptorTable(param, handle)` with `handle` the GPU
address of a slot. `draw.rs` maps a table handle back to a title index with
`Heap::index`. `State::tables` makes a table afresh in the per-list dynamic
area when it holds a constant buffer (the host wants 256-byte alignment,
the console only 16) or when any of its entries reaches past the host
heap's title slots. The second case is #21's fix and covers a capped
NVIDIA view heap.

Chunking can go further if a title needs the slots: with chunks of `CHUNK`
slots, a table whose range lies in one chunk binds at that chunk's base,
and a table crossing a boundary is copied into the dynamic area exactly as
a table with a constant buffer is today. The byte offset `user_value` hands
a shader becomes chunk-relative, which translated shaders treat as opaque.

### 2. Directly indexed heaps (SM 6.6 `ResourceDescriptorHeap`)

Here the shader indexes the descriptor heap itself, and the index space the
driver presents is the single heap the stage has bound. Two bound view heaps
do not merge into one index space, so chunking cannot serve a shader that
indexes past the cap. Two options, both real:

- **Compact used descriptors.** Every slot the title writes is a CPU-side
  `Create*View` at a host slot we choose; the index is visible only to the
  title's own code (which writes by slot) and to shaders (which read by
  index). Compacting means rewriting the index a shader computes, i.e.
  patching the index arithmetic in `lower.rs` where it reads the table's
  byte offset: a per-shader ALU addition plus a mapping table in root
  constants. Costly, and it touches every direct-index path.
- **Move the working set.** Keep a 1M window and, when the title writes or
  binds slots outside it, migrate the window and invalidate the cache.
  Correct only if usage is temporally clustered, which a streaming world is
  not.

Neither is a small change. Neither #21 nor this branch addresses direct
indexing past the cap.

## Samplers

`D3D12_MAX_SHADER_VISIBLE_SAMPLER_HEAP_SIZE` is 2,048 on every tier. A
sampler heap over that fails `CreateDescriptorHeap` today and `HostHeap::new`
returns `None`, losing the heap outright rather than capping it. That is a
different failure from the view heap cap and no open PR touches it; it
deserves its own issue.

## Where the cap was measured

#16 is an NVIDIA RTX 5070 Ti; #11 an RTX 4060. On the AMD RX 7600 this was
built on, the host accepted a 1,100,001-slot view heap whole, so the cap is
the NVIDIA tier-2 limit, not a portable one. The warning added here needs a
tier-2 NVIDIA host to be seen live; it is a static change on this machine.

## Order of work

1. Report the title's heap size, the host's cap, and the slots lost at
   creation (this branch).
2. Remap root tables past the cap (#21, landed).
3. Decide what to do with directly indexed heaps past the cap.
4. Handle the sampler heap's 2,048 limit without losing the heap.
