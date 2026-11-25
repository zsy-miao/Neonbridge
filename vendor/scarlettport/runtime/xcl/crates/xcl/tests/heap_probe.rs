//! What D3D12 accepts as a heap over memory a title addresses: a pagefile-backed section
//! reserved (SEC_RESERVE) at an address XCL picks, committed only in part – and whether pages
//! committed after the heap was opened are seen by the GPU. GPU copies only, of committed
//! pages only. `cargo test -p xcl --test heap_probe -- --ignored --nocapture`

use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Memory::*;

unsafe fn buffer(dev: &ID3D12Device, heap: D3D12_HEAP_TYPE, bytes: u64, state: D3D12_RESOURCE_STATES) -> ID3D12Resource {
    let mut r: Option<ID3D12Resource> = None;
    dev.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() }, D3D12_HEAP_FLAG_NONE, &desc(bytes, D3D12_RESOURCE_FLAG_NONE), state, None, &mut r).unwrap();
    r.unwrap()
}

fn desc(bytes: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    }
}

/// GPU: copy `n` bytes from `src` at `so` to `dst` at `doff`, and wait
unsafe fn gpu_copy(dev: &ID3D12Device, q: &ID3D12CommandQueue, dst: &ID3D12Resource, doff: u64, src: &ID3D12Resource, so: u64, n: u64) {
    let alloc: ID3D12CommandAllocator = dev.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).unwrap();
    let list: ID3D12GraphicsCommandList = dev.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None).unwrap();
    list.CopyBufferRegion(dst, doff, src, so, n);
    list.Close().unwrap();
    q.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
    let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
    q.Signal(&fence, 1).unwrap();
    while fence.GetCompletedValue() < 1 {
        std::thread::yield_now();
    }
}

#[test]
#[ignore]
fn heaps_over_partly_committed_sections() {
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut dev).is_err() {
            return;
        }
        let dev = dev.unwrap();
        let dev3: ID3D12Device3 = dev.cast().unwrap();
        let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        // (size, SEC_RESERVE, commit it all before opening)
        for (size, reserve, all) in [(64usize << 20, false, false), (64 << 20, true, true), (64 << 20, true, false), ((4096usize << 20) - 0x1_0000, true, false), (4096 << 20, true, false)] {
            let flags = if reserve { SEC_RESERVE } else { SEC_COMMIT };
            let size_label = format!("{} MiB {}{}", size >> 20, if reserve { "SEC_RESERVE" } else { "SEC_COMMIT" }, if all { " all committed" } else { "" });
            let h = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE | flags, (size >> 32) as u32, size as u32, std::ptr::null());
            if h.is_null() {
                println!("{} MiB: CreateFileMapping failed", size >> 20);
                continue;
            }
            // at a 4 GiB-aligned address of our choosing
            let mut view = std::ptr::null_mut();
            for slot in 64usize..4096 {
                let v = MapViewOfFileEx(h, FILE_MAP_ALL_ACCESS, 0, 0, size, (slot << 32) as *const c_void);
                if !v.Value.is_null() {
                    view = v.Value;
                    break;
                }
            }
            if view.is_null() {
                println!("{} MiB: no view", size >> 20);
                CloseHandle(h);
                continue;
            }
            // commit the first 1 MiB and fill it
            let mb = 1usize << 20;
            if reserve {
                assert!(!VirtualAlloc(view, if all { size } else { mb }, MEM_COMMIT, PAGE_READWRITE).is_null());
            }
            for i in 0..mb / 4 {
                *(view as *mut u32).add(i) = i as u32 ^ 0xA5A5_0000;
            }
            let heap: windows::core::Result<ID3D12Heap> = dev3.OpenExistingHeapFromFileMapping(HANDLE(h));
            let heap = match heap {
                Ok(hp) => hp,
                Err(e) => {
                    println!("{size_label} at {view:p}: OpenExistingHeapFromFileMapping: {e}");
                    UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view });
                    CloseHandle(h);
                    continue;
                }
            };
            let hd = heap.GetDesc();
            // a buffer over the first 4 MiB (only 1 MiB committed: the GPU touches only that)
            let mut placed: Option<ID3D12Resource> = None;
            let hr = dev.CreatePlacedResource(&heap, 0, &desc(4 * mb as u64, D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER), D3D12_RESOURCE_STATE_COMMON, None, &mut placed);
            let Some(placed) = placed.filter(|_| hr.is_ok()) else {
                println!("{size_label}: heap {} bytes, CreatePlacedResource: {hr:?}", hd.SizeInBytes);
                continue;
            };
            let rb = buffer(&dev, D3D12_HEAP_TYPE_READBACK, mb as u64, D3D12_RESOURCE_STATE_COPY_DEST);
            gpu_copy(&dev, &q, &rb, 0, &placed, 0, mb as u64);
            let mut p = std::ptr::null_mut();
            rb.Map(0, None, Some(&mut p)).unwrap();
            let read_ok = (0..mb / 4).all(|i| *(p as *const u32).add(i) == i as u32 ^ 0xA5A5_0000);
            rb.Unmap(0, None);
            // commit 1 MiB more after the heap was opened; the CPU writes it, the GPU reads it
            if reserve {
                assert!(!VirtualAlloc((view as usize + 2 * mb) as *const c_void, mb, MEM_COMMIT, PAGE_READWRITE).is_null());
            }
            for i in 0..mb / 4 {
                *((view as usize + 2 * mb) as *mut u32).add(i) = i as u32 ^ 0x5A5A_0000;
            }
            gpu_copy(&dev, &q, &rb, 0, &placed, 2 * mb as u64, mb as u64);
            rb.Map(0, None, Some(&mut p)).unwrap();
            let later_ok = (0..mb / 4).all(|i| *(p as *const u32).add(i) == i as u32 ^ 0x5A5A_0000);
            rb.Unmap(0, None);
            // the GPU writes, the CPU reads at the title's address
            let up = buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, 4096, D3D12_RESOURCE_STATE_GENERIC_READ);
            up.Map(0, None, Some(&mut p)).unwrap();
            for i in 0..1024 {
                *(p as *mut u32).add(i) = 0xC0DE_0000 | i as u32;
            }
            up.Unmap(0, None);
            gpu_copy(&dev, &q, &placed, 0x1000, &up, 0, 4096);
            let write_ok = (0..1024).all(|i| *((view as usize + 0x1000) as *const u32).add(i) == 0xC0DE_0000 | i as u32);
            println!(
                "{size_label} at {view:p}: heap {} bytes; GPU reads what the CPU wrote: {read_ok}; pages committed after opening: {later_ok}; CPU sees GPU writes: {write_ok}",
                hd.SizeInBytes
            );
            drop((placed, heap));
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view });
            CloseHandle(h);
        }
    }
}

#[allow(non_snake_case)]
unsafe fn CloseHandle(h: *mut c_void) {
    windows_sys::Win32::Foundation::CloseHandle(h);
}

/// A reservation as a placeholder; chunks of it replaced by fully committed sections mapped at
/// their addresses, each opened as a heap: the GPU reads and writes each chunk at the title's
/// address, neighbouring chunks independent.
#[test]
#[ignore]
fn heaps_over_sections_in_a_placeholder() {
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut dev).is_err() {
            return;
        }
        let dev = dev.unwrap();
        let dev3: ID3D12Device3 = dev.cast().unwrap();
        let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        let (total, chunk) = (256usize << 20, 2usize << 20);
        let proc_ = windows_sys::Win32::System::Threading::GetCurrentProcess();
        let base = VirtualAlloc2(proc_, std::ptr::null(), total, MEM_RESERVE | MEM_RESERVE_PLACEHOLDER, PAGE_NOACCESS, std::ptr::null_mut(), 0) as usize;
        assert!(base != 0, "placeholder reservation");
        let mut keep = Vec::new();
        for (k, at) in [(0usize, base + 10 * chunk), (1, base + 11 * chunk), (2, base + 40 * chunk)] {
            // split the chunk off the placeholder (the first split of a range: VirtualFree with
            // MEM_PRESERVE_PLACEHOLDER)
            let ok = VirtualFree(at as *mut c_void, chunk, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER);
            let h = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE | SEC_COMMIT, 0, chunk as u32, std::ptr::null());
            let v = MapViewOfFile3(h, proc_, at as *const c_void, 0, chunk, MEM_REPLACE_PLACEHOLDER, PAGE_READWRITE, std::ptr::null_mut(), 0);
            if v.Value as usize != at {
                println!("chunk {k}: split {ok}, view {:p} (wanted {at:#x}), error {}", v.Value, windows_sys::Win32::Foundation::GetLastError());
                continue;
            }
            for i in 0..chunk / 4 {
                *(at as *mut u32).add(i) = (k as u32) << 28 | i as u32;
            }
            let heap: ID3D12Heap = match dev3.OpenExistingHeapFromFileMapping(HANDLE(h)) {
                Ok(hp) => hp,
                Err(e) => {
                    println!("chunk {k}: OpenExistingHeapFromFileMapping: {e}");
                    continue;
                }
            };
            let mut placed: Option<ID3D12Resource> = None;
            dev.CreatePlacedResource(&heap, 0, &desc(chunk as u64, D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS), D3D12_RESOURCE_STATE_COMMON, None, &mut placed).unwrap();
            let placed = placed.unwrap();
            let rb = buffer(&dev, D3D12_HEAP_TYPE_READBACK, chunk as u64, D3D12_RESOURCE_STATE_COPY_DEST);
            gpu_copy(&dev, &q, &rb, 0, &placed, 0, chunk as u64);
            let mut p = std::ptr::null_mut();
            rb.Map(0, None, Some(&mut p)).unwrap();
            let read_ok = (0..chunk / 4).all(|i| *(p as *const u32).add(i) == (k as u32) << 28 | i as u32);
            rb.Unmap(0, None);
            let up = buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, 4096, D3D12_RESOURCE_STATE_GENERIC_READ);
            up.Map(0, None, Some(&mut p)).unwrap();
            for i in 0..1024 {
                *(p as *mut u32).add(i) = 0xBEEF_0000 | (k as u32) << 12 | i as u32;
            }
            up.Unmap(0, None);
            gpu_copy(&dev, &q, &placed, chunk as u64 - 4096, &up, 0, 4096);
            let write_ok = (0..1024).all(|i| *((at + chunk - 4096) as *const u32).add(i) == 0xBEEF_0000 | (k as u32) << 12 | i as u32);
            println!("chunk {k} at {at:#x}: GPU reads it: {read_ok}, CPU sees GPU writes: {write_ok}");
            keep.push((h, heap, placed));
        }
        // neighbours untouched: chunk 0's last page was written by the GPU, chunk 1's first word is chunk 1's
        println!("chunk 1 first word {:#x}", *((base + 11 * chunk) as *const u32));
        for (h, heap, placed) in keep {
            drop((placed, heap));
            CloseHandle(h);
        }
    }
}


/// A reserved (tiled) buffer of a whole memory window with some 64 KiB tiles mapped onto a
/// default heap (video memory): uploaded data reads back through the window buffer.
/// Standard tiled-resource use; GPU copies only.
#[test]
#[ignore]
fn window_sized_reserved_buffer_on_video_memory_tiles() {
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut dev).is_err() {
            return;
        }
        let dev = dev.unwrap();
        let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).unwrap();
        for size in [4096u64 << 20, (4096u64 << 20) - 0x1_0000] {
            let mut rbuf: Option<ID3D12Resource> = None;
            let hr = dev.CreateReservedResource(&desc(size, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS), D3D12_RESOURCE_STATE_COMMON, None, &mut rbuf);
            let Some(rbuf) = rbuf.filter(|_| hr.is_ok()) else {
                println!("reserved buffer of {} MiB: {hr:?}", size >> 20);
                continue;
            };
            let pool_desc = D3D12_HEAP_DESC {
                SizeInBytes: 64 << 20,
                Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                Alignment: 0,
                Flags: D3D12_HEAP_FLAG_DENY_RT_DS_TEXTURES | D3D12_HEAP_FLAG_DENY_NON_RT_DS_TEXTURES,
            };
            let mut pool: Option<ID3D12Heap> = None;
            dev.CreateHeap(&pool_desc, &mut pool).unwrap();
            let pool = pool.unwrap();
            // tiles at the start, in the middle and the last one of the window
            let last = (size / 0x1_0000 - 1) as u32;
            let at = [0u32, 30_000, last];
            for (k, &x) in at.iter().enumerate() {
                let start = D3D12_TILED_RESOURCE_COORDINATE { X: x, ..Default::default() };
                let region = D3D12_TILE_REGION_SIZE { NumTiles: 1, UseBox: false.into(), ..Default::default() };
                let (flags, heap_start, n) = (D3D12_TILE_RANGE_FLAG_NONE, k as u32, 1u32);
                q.UpdateTileMappings(&rbuf, 1, Some(&start), Some(&region), &pool, 1, Some(&flags), Some(&heap_start), Some(&n), D3D12_TILE_MAPPING_FLAG_NONE);
            }
            let up = buffer(&dev, D3D12_HEAP_TYPE_UPLOAD, 3 * 0x1_0000, D3D12_RESOURCE_STATE_GENERIC_READ);
            let mut p = std::ptr::null_mut();
            up.Map(0, None, Some(&mut p)).unwrap();
            for i in 0..3 * 0x1_0000 / 4 {
                *(p as *mut u32).add(i) = 0x7000_0000 | i as u32;
            }
            up.Unmap(0, None);
            for (k, &x) in at.iter().enumerate() {
                gpu_copy(&dev, &q, &rbuf, x as u64 * 0x1_0000, &up, k as u64 * 0x1_0000, 0x1_0000);
            }
            let rb = buffer(&dev, D3D12_HEAP_TYPE_READBACK, 3 * 0x1_0000, D3D12_RESOURCE_STATE_COPY_DEST);
            for (k, &x) in at.iter().enumerate() {
                gpu_copy(&dev, &q, &rb, k as u64 * 0x1_0000, &rbuf, x as u64 * 0x1_0000, 0x1_0000);
            }
            rb.Map(0, None, Some(&mut p)).unwrap();
            let ok = (0..3 * 0x1_0000 / 4).all(|i| *(p as *const u32).add(i) == 0x7000_0000 | i as u32);
            rb.Unmap(0, None);
            println!("reserved buffer of {} MiB, GPU VA {:#x}: tiles 0 / 30000 / {last} on video memory read back: {ok}", size >> 20, rbuf.GetGPUVirtualAddress());
        }
    }
}

/// How large a shader-visible view heap this GPU takes (OPTIONS19 and a real one): console
/// titles' view heaps together go past the 1 000 000 of resource binding tier 2.
#[test]
#[ignore]
fn view_heap_capacity() {
    unsafe {
        let mut dev: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut dev).unwrap();
        let dev = dev.unwrap();
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS19::default();
        let ok = dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS19, &mut o as *mut _ as *mut c_void, std::mem::size_of_val(&o) as u32).is_ok();
        println!("OPTIONS19 {ok}: max view heap {} samplers {}", o.MaxViewDescriptorHeapSize, o.MaxSamplerDescriptorHeapSize);
        for n in [1_000_000u32, 1_200_000, 2_000_000, 4_000_000] {
            let d = D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: n, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 };
            let r: windows::core::Result<ID3D12DescriptorHeap> = dev.CreateDescriptorHeap(&d);
            println!("shader-visible view heap of {n}: {:?}", r.as_ref().map(|_| ()).map_err(|e| e.code()));
        }
    }
}
