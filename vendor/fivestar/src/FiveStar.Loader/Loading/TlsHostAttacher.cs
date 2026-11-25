// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Implicit TLS (__declspec(thread), thread_local) for mapped images. Compiled code reads TLS through
/// gs:[58h][_tls_index], an array only ntdll can grow for current and future threads, so ntdll does the work: the
/// image's generated TLS host DLL is loaded with LoadLibrary, ntdll assigns the slot, allocates every thread's block
/// and runs the callbacks for PROCESS/THREAD attach and detach, and the slot is copied into the image's _tls_index.
/// </summary>
internal sealed unsafe class TlsHostAttacher
{
    /// <summary>LDR_DATA_TABLE_ENTRY.TlsIndex of a module with TLS whose slot ntdll manages dynamically.</summary>
    private const ushort DynamicTlsIndex = 0xFFFF;

    private readonly string _workDirectory;
    private readonly int _processId;
    private readonly LoaderLog _log;

    public TlsHostAttacher(string workDirectory, int processId, LoaderLog log)
    {
        _workDirectory = workDirectory;
        _processId = processId;
        _log = log;
    }

    public void Attach(LoadedImage image)
    {
        var directory = image.Pe.Directory(DataDirectoryIndex.Tls);

        if (!directory.IsPresent)
        {
            return;
        }

        var template = ReadTemplate(image, (byte*)image.Va(directory.Rva));

        // A NativeAOT host keeps its own thread statics (the managed thread, its id, lock ownership) in slot 0, which
        // TlsSlotZero hands to the mapped executable: every thread would lose them (docs/aot.md).
        if (template.OwnsSlotZero && !RuntimeFeature.IsDynamicCodeSupported)
        {
            throw new NotSupportedException($"{image.Name} has implicit TLS in slot 0, which the NativeAOT loader's runtime owns; use the JIT build");
        }

        string path = Path.Combine(_workDirectory, $"tlshost_{Path.GetFileNameWithoutExtension(image.Name)}_{_processId}.dll");
        System.IO.Directory.CreateDirectory(_workDirectory);
        File.WriteAllBytes(path, TlsHostBuilder.Build(template));
        nint host = LoadHost(path);

        // The callback thunks already copied the slot before PROCESS_ATTACH; images without callbacks need it here.
        uint index = *(uint*)(host + (nint)(TlsHostBuilder.DataRva + TlsHostBuilder.HostIndexOffset));

        if (index == 0 && ProcessEnvironmentBlock.LoaderTlsIndexOf(host) != DynamicTlsIndex)
        {
            throw new InvalidOperationException("ntdll did not process the TLS host's TLS directory");
        }

        *(uint*)template.IndexAddress = index;
        image.TlsHost = host;
        image.TlsHostPath = path;
        image.TlsIndex = index;
        image.TlsCallbackCount = template.Callbacks.Count;
        _log.Info($"  TLS: {template.RawData.Length} template bytes + {template.ZeroFill} zero-fill, {template.Callbacks.Count} callback(s); host {Path.GetFileName(path)} @ 0x{host:x}, _tls_index={index}");

        if (template.OwnsSlotZero)
        {
            image.TlsSlotZeroSaveOffset = TlsSlotZero.SaveOffset(template);
            int threads = TlsSlotZeroThreads.EnterExisting(index, image.TlsSlotZeroSaveOffset);
            _log.Info($"  TLS: slot 0 of {threads} thread(s) started before the host points at the executable's block");
        }
    }

    private static TlsTemplate ReadTemplate(LoadedImage image, byte* directory)
    {
        ulong start = *(ulong*)(directory + TlsFormat.StartAddressOfRawDataOffset);
        ulong end = *(ulong*)(directory + TlsFormat.EndAddressOfRawDataOffset);
        ulong indexAddress = *(ulong*)(directory + TlsFormat.AddressOfIndexOffset);
        ulong callbacksAddress = *(ulong*)(directory + TlsFormat.AddressOfCallBacksOffset);
        int length = checked((int)(end - start));
        var raw = new byte[length];

        if (length > 0)
        {
            new ReadOnlySpan<byte>((void*)(nint)start, length).CopyTo(raw);
        }

        var callbacks = new List<ulong>();

        if (callbacksAddress != 0)
        {
            for (ulong* callback = (ulong*)callbacksAddress; *callback != 0; callback++)
            {
                callbacks.Add(*callback);
            }
        }

        uint zeroFill = *(uint*)(directory + TlsFormat.SizeOfZeroFillOffset);
        uint characteristics = *(uint*)(directory + TlsFormat.CharacteristicsOffset);

        return new TlsTemplate(raw, zeroFill, characteristics, callbacks, (ulong)image.Base, indexAddress, !image.Pe.IsDll);
    }

    private static nint LoadHost(string path)
    {
        nint host;

        fixed (char* name = path)
        {
            host = Kernel32.LoadLibraryExW(name, 0, 0);
        }

        if (host == 0)
        {
            throw new InvalidOperationException($"LoadLibrary(TLS host) failed: {Marshal.GetLastPInvokeError()}");
        }

        return host;
    }
}
