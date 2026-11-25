// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Module handles for modules that exist only as shims (Xbox-only DLLs such as xgameruntime.dll, xmem.dll and
/// d3d12_xs.dll). LoadLibrary and GetModuleHandle from mapped code return one of these, and GetProcAddress on it
/// answers from the shim table. Each handle is a read-only page holding a minimal, well-formed PE header (no sections
/// or directories), so code that peeks at the header sees a plausible module.
/// </summary>
internal sealed unsafe class VirtualModules
{
    /// <summary>e_lfanew of the generated header.</summary>
    private const int NtHeaderOffset = 0x80;

    private const ushort Characteristics = PeFormat.FileExecutableImage | PeFormat.FileLargeAddressAware | PeFormat.FileDll;
    private const uint FileAlignment = 0x200;
    private const uint SizeOfHeaders = 0x200;

    private readonly object _gate = new();
    private readonly Dictionary<string, nint> _byName = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<nint, string> _byHandle = [];
    private readonly LoaderLog _log;

    public VirtualModules(LoaderLog log)
    {
        _log = log;
    }

    public nint GetOrCreate(string moduleKey)
    {
        lock (_gate)
        {
            if (_byName.TryGetValue(moduleKey, out var existing))
            {
                return existing;
            }

            nint handle = Kernel32.VirtualAlloc(0, Win32Constants.PageSize, Win32Constants.MemReserveCommit, Win32Constants.PageReadWrite);

            if (handle == 0)
            {
                throw new InvalidOperationException("VirtualModules: VirtualAlloc failed");
            }

            WriteHeader((byte*)handle, handle);
            uint previous;
            _ = Kernel32.VirtualProtect(handle, Win32Constants.PageSize, Win32Constants.PageReadOnly, &previous);
            _byName[moduleKey] = handle;
            _byHandle[handle] = moduleKey;
            _log.Info($"  [loader] virtual module {moduleKey} = 0x{handle:x} (shim-only)");

            return handle;
        }
    }

    /// <summary>The module name of a virtual handle, or null.</summary>
    public string? NameOf(nint handle)
    {
        lock (_gate)
        {
            return _byHandle.GetValueOrDefault(handle);
        }
    }

    /// <summary>The handle of an existing virtual module, or 0.</summary>
    public nint Find(string moduleKey)
    {
        lock (_gate)
        {
            return _byName.GetValueOrDefault(moduleKey);
        }
    }

    public bool Contains(nint address)
    {
        lock (_gate)
        {
            return _byHandle.Keys.Any(h => (ulong)address >= (ulong)h && (ulong)address < (ulong)h + Win32Constants.PageSize);
        }
    }

    private static void WriteHeader(byte* page, nint handle)
    {
        byte* nt = page + NtHeaderOffset;
        byte* optional = nt + PeFormat.NtSignatureSize + PeFormat.FileHeaderSize;
        byte* fileHeader = nt + PeFormat.NtSignatureSize;

        // DOS header, signature and COFF header.
        *(ushort*)page = PeFormat.DosSignature;
        *(int*)(page + PeFormat.NewHeaderPointerOffset) = NtHeaderOffset;
        *(uint*)nt = PeFormat.NtSignature;
        *(ushort*)(fileHeader + PeFormat.MachineOffset) = PeFormat.MachineAmd64;
        *(ushort*)(fileHeader + PeFormat.SizeOfOptionalHeaderOffset) = PeFormat.OptionalHeaderSize;
        *(ushort*)(fileHeader + PeFormat.CharacteristicsOffset) = Characteristics;

        // Optional header.
        *(ushort*)optional = PeFormat.Pe32PlusMagic;
        *(ulong*)(optional + PeFormat.ImageBaseOffset) = (ulong)handle;
        *(uint*)(optional + PeFormat.SectionAlignmentOffset) = Win32Constants.PageSize;
        *(uint*)(optional + PeFormat.FileAlignmentOffset) = FileAlignment;
        *(uint*)(optional + PeFormat.SizeOfImageOffset) = Win32Constants.PageSize;
        *(uint*)(optional + PeFormat.SizeOfHeadersOffset) = SizeOfHeaders;

        *(ushort*)(optional + PeFormat.SubsystemOffset) = PeFormat.SubsystemWindowsCui;
        *(uint*)(optional + PeFormat.NumberOfRvaAndSizesOffset) = PeFormat.MaxDataDirectories;
    }
}
