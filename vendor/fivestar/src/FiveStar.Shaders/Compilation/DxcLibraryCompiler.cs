// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using System.Text;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Compiles translated HLSL inside this process through the DXC API of dxcompiler.dll (IDxcCompiler3), with the
/// arguments of <see cref="DxcCompiler"/>, so the DXIL is the one dxc.exe of the same build writes. dxil.dll beside it is
/// loaded first, so the DXIL is signed. A compile cannot be stopped: the time limit is not applied, and callers pick -Od
/// for programs known to be slow. Each compile has its own compiler object, so compiles may run on any threads.
/// </summary>
public sealed unsafe class DxcLibraryCompiler : IShaderCompiler
{
    /// <summary>The DXC library file.</summary>
    public const string LibraryFile = "dxcompiler.dll";

    private const string ValidatorFile = "dxil.dll";
    private const uint Utf8CodePage = 65001;

    // IUnknown::Release; IDxcCompiler3::Compile; IDxcUtils::CreateDefaultIncludeHandler; IDxcOperationResult GetStatus,
    // GetResult, GetErrorBuffer; IDxcBlob GetBufferPointer, GetBufferSize (dxcapi.h).
    private const int ReleaseSlot = 2;
    private const int CompileSlot = 3;
    private const int IncludeHandlerSlot = 9;
    private const int StatusSlot = 3;
    private const int ResultSlot = 4;
    private const int ErrorSlot = 5;
    private const int PointerSlot = 3;
    private const int SizeSlot = 4;

    private static readonly Guid CompilerClass = new("73e22d93-e6ce-47f3-b5bf-f0664f39c1b0");
    private static readonly Guid UtilsClass = new("6245d6af-66e0-48fd-80b4-4d271796748c");
    private static readonly Guid Compiler3Interface = new("228b4687-5a6a-4730-900c-9702b2203f54");
    private static readonly Guid UtilsInterface = new("4605c4cb-2019-492a-ada4-65f20bb7d67f");
    private static readonly Guid ResultInterface = new("58346cda-dde7-4497-9461-6f87af5e0659");

    private readonly DxcOptions _options;
    private readonly string _library;
    private readonly Lazy<string> _identity;
    private readonly Lazy<nint> _create;

    /// <param name="options">Limits as for dxc.exe; <see cref="DxcOptions.Path"/> names dxc.exe, dxcompiler.dll or their folder.</param>
    public DxcLibraryCompiler(DxcOptions options)
    {
        _options = options;
        _library = LibraryPath(options.Path);
        _identity = new Lazy<string>(ComputeIdentity);
        _create = new Lazy<nint>(LoadLibrary);
    }

    public DxcOptions Options => _options;

    public string Identity => _identity.Value;

    /// <summary>The dxcompiler.dll of a dxc.exe, a dxcompiler.dll or a folder holding them.</summary>
    public static string LibraryPath(string path)
    {
        if (Directory.Exists(path))
        {
            return Path.Combine(path, LibraryFile);
        }

        return Path.Combine(Path.GetDirectoryName(Path.GetFullPath(path)) ?? ".", LibraryFile);
    }

    /// <summary>Compiles one source and writes <see cref="CompileRequest.DxilPath"/> (through a temporary file) when it succeeds.</summary>
    public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
    {
        var (dxil, messages) = CompileBytes(request);

        if (dxil is null)
        {
            return CompileOutcome.Failure(messages);
        }

        var temporary = request.DxilPath + ".tmp";
        File.WriteAllBytes(temporary, dxil);
        File.Move(temporary, request.DxilPath, overwrite: true);

        return CompileOutcome.Success with { Error = messages };
    }

    /// <summary>The DXIL of a request (null when the compile failed) and the compiler's messages.</summary>
    public (byte[]? Dxil, string Messages) CompileBytes(CompileRequest request)
    {
        var arguments = DxcCompiler.Arguments(request);
        var text = Encoding.UTF8.GetBytes(request.Hlsl);
        var pointers = arguments.Select(Marshal.StringToHGlobalUni).ToArray();
        nint compiler = 0, utils = 0, include = 0, result = 0;

        try
        {
            compiler = Create(CompilerClass, Compiler3Interface);
            utils = Create(UtilsClass, UtilsInterface);
            Check(((delegate* unmanaged<nint, nint*, int>)Slot(utils, IncludeHandlerSlot))(utils, &include), "CreateDefaultIncludeHandler");

            fixed (byte* source = text)
            fixed (nint* argv = pointers)
            {
                var buffer = new SourceBuffer { Pointer = source, Size = (nuint)text.Length, Encoding = Utf8CodePage };
                var iid = ResultInterface;
                var compile = (delegate* unmanaged<nint, SourceBuffer*, nint*, uint, nint, Guid*, nint*, int>)Slot(compiler, CompileSlot);
                Check(compile(compiler, &buffer, argv, (uint)pointers.Length, include, &iid, &result), "Compile");
            }

            return Outcome(result);
        }
        finally
        {
            foreach (var pointer in pointers)
            {
                Marshal.FreeHGlobal(pointer);
            }

            Release(result);
            Release(include);
            Release(utils);
            Release(compiler);
        }
    }

    private static (byte[]? Dxil, string Messages) Outcome(nint result)
    {
        int status;
        Check(((delegate* unmanaged<nint, int*, int>)Slot(result, StatusSlot))(result, &status), "GetStatus");
        var messages = ErrorText(result);

        if (status < 0)
        {
            return (null, messages);
        }

        nint blob = 0;
        Check(((delegate* unmanaged<nint, nint*, int>)Slot(result, ResultSlot))(result, &blob), "GetResult");

        try
        {
            return (Bytes(blob), messages);
        }
        finally
        {
            Release(blob);
        }
    }

    private static string ErrorText(nint result)
    {
        nint blob = 0;

        if (((delegate* unmanaged<nint, nint*, int>)Slot(result, ErrorSlot))(result, &blob) < 0 || blob == 0)
        {
            return string.Empty;
        }

        try
        {
            return Encoding.UTF8.GetString(Bytes(blob)).TrimEnd('\0');
        }
        finally
        {
            Release(blob);
        }
    }

    private static byte[] Bytes(nint blob)
    {
        if (blob == 0)
        {
            return [];
        }

        var pointer = ((delegate* unmanaged<nint, nint>)Slot(blob, PointerSlot))(blob);
        var size = ((delegate* unmanaged<nint, nuint>)Slot(blob, SizeSlot))(blob);

        return new ReadOnlySpan<byte>((void*)pointer, checked((int)size)).ToArray();
    }

    private nint Create(Guid type, Guid iid)
    {
        nint instance = 0;
        var create = (delegate* unmanaged<Guid*, Guid*, nint*, int>)_create.Value;
        Check(create(&type, &iid, &instance), "DxcCreateInstance");

        return instance;
    }

    /// <summary>Loads dxil.dll (when it is there) and then dxcompiler.dll, whose validator lookup finds the loaded one.</summary>
    private nint LoadLibrary()
    {
        var validator = Path.Combine(Path.GetDirectoryName(_library) ?? ".", ValidatorFile);

        if (File.Exists(validator))
        {
            NativeLibrary.Load(validator);
        }

        return NativeLibrary.GetExport(NativeLibrary.Load(_library), "DxcCreateInstance");
    }

    /// <summary>The DXC library files, the fixed arguments and the runtime headers the sources include.</summary>
    private string ComputeIdentity()
    {
        var folder = Path.GetDirectoryName(_library) ?? string.Empty;
        string[] files = [_library, Path.Combine(folder, ValidatorFile)];

        return ContentHash.OfText([ContentHash.OfFiles(files), DxcCompiler.FixedArguments, .. RuntimeIncludes.Names.Select(RuntimeIncludes.Read)]);
    }

    private static nint Slot(nint instance, int slot)
    {
        return (*(nint**)instance)[slot];
    }

    private static void Release(nint instance)
    {
        if (instance != 0)
        {
            ((delegate* unmanaged<nint, uint>)Slot(instance, ReleaseSlot))(instance);
        }
    }

    private static void Check(int hr, string call)
    {
        if (hr < 0)
        {
            throw new InvalidOperationException($"{call} failed: 0x{hr:x8}");
        }
    }

    /// <summary>DxcBuffer.</summary>
    [StructLayout(LayoutKind.Sequential)]
    private struct SourceBuffer
    {
        public byte* Pointer;
        public nuint Size;
        public uint Encoding;
    }
}
