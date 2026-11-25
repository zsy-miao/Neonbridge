// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;

namespace FiveStar.Shaders.Dxil.Validation;

/// <summary>
/// The DXIL validator of dxil.dll (<c>IDxcValidator</c>), for tests and coverage runs; translated programs do not need
/// it, since the backend computes the container digest itself. Validation signs a copy in place, so the caller can
/// compare the validator's digest with ours. Calls are serialized.
/// </summary>
public sealed unsafe class DxilValidator : IDisposable
{
    private const uint InPlaceEdit = 1;
    private const int ValidateSlot = 3;
    private const int StatusSlot = 3;
    private const int ErrorsSlot = 5;
    private const int PointerSlot = 3;
    private const int SizeSlot = 4;
    private const int ReleaseSlot = 2;
    private const int NoInterface = unchecked((int)0x8000_4002);
    private const int BlobSlots = 5;
    private const int BlobHeader = 3;

    private static readonly Guid ValidatorClass = new("8ca3e215-f728-4cf3-8cdd-88af917587a1");
    private static readonly Guid ValidatorInterface = new("a6e82bd2-1fd7-4826-9811-2857e797f49a");
    private static readonly Guid UnknownInterface = new("00000000-0000-0000-c000-000000000046");
    private static readonly Guid BlobInterface = new("8ba5fb08-5195-40e2-ac58-0d989c3a0102");

    private readonly Lock _gate = new();
    private readonly nint _library;
    private void* _validator;

    private DxilValidator(nint library, void* validator)
    {
        _library = library;
        _validator = validator;
    }

    /// <summary>Loads dxil.dll from <paramref name="path"/>; throws when it cannot be loaded.</summary>
    public static DxilValidator Load(string path)
    {
        var library = NativeLibrary.Load(path);
        var create = (delegate* unmanaged[Stdcall]<Guid*, Guid*, void**, int>)NativeLibrary.GetExport(library, "DxcCreateInstance");
        void* validator = null;
        var classId = ValidatorClass;
        var interfaceId = ValidatorInterface;
        var result = create(&classId, &interfaceId, &validator);

        if (result < 0 || validator is null)
        {
            throw new InvalidOperationException($"DxcCreateInstance(validator) failed: 0x{result:x8}");
        }

        return new DxilValidator(library, validator);
    }

    /// <summary>Validates and signs <paramref name="container"/> in place; the validator's messages when it is rejected, else null.</summary>
    public string? Validate(byte[] container)
    {
        lock (_gate)
        {
            fixed (byte* data = container)
            {
                return ValidatePinned(data, container.Length);
            }
        }
    }

    public void Dispose()
    {
        if (_validator is not null)
        {
            Release(_validator);
            _validator = null;
        }

        NativeLibrary.Free(_library);
    }

    private string? ValidatePinned(byte* data, int length)
    {
        var blob = (nint*)NativeMemory.Alloc((nuint)(sizeof(nint) * (BlobSlots + BlobHeader)));
        var table = blob + BlobHeader;
        table[0] = (nint)(delegate* unmanaged[Stdcall]<nint*, Guid*, void**, int>)&QueryInterface;
        table[1] = (nint)(delegate* unmanaged[Stdcall]<nint*, uint>)&Reference;
        table[ReleaseSlot] = (nint)(delegate* unmanaged[Stdcall]<nint*, uint>)&Reference;
        table[PointerSlot] = (nint)(delegate* unmanaged[Stdcall]<nint*, nint>)&BufferPointer;

        table[SizeSlot] = (nint)(delegate* unmanaged[Stdcall]<nint*, nint>)&BufferSize;
        blob[0] = (nint)table;
        blob[1] = (nint)data;
        blob[ReleaseSlot] = length;

        try
        {
            void* result = null;
            var validate = (delegate* unmanaged[Stdcall]<void*, void*, uint, void**, int>)Method(_validator, ValidateSlot);
            var code = validate(_validator, blob, InPlaceEdit, &result);

            if (code < 0 || result is null)
            {
                return $"Validate failed: 0x{code:x8}";
            }

            var text = Status(result);
            Release(result);

            return text;
        }
        finally
        {
            NativeMemory.Free(blob);
        }
    }

    private static string? Status(void* result)
    {
        var status = 0;
        ((delegate* unmanaged[Stdcall]<void*, int*, int>)Method(result, StatusSlot))(result, &status);

        if (status >= 0)
        {
            return null;
        }

        void* errors = null;
        var text = $"rejected (0x{status:x8})";

        if (((delegate* unmanaged[Stdcall]<void*, void**, int>)Method(result, ErrorsSlot))(result, &errors) >= 0 && errors is not null)
        {
            var pointer = ((delegate* unmanaged[Stdcall]<void*, byte*>)Method(errors, PointerSlot))(errors);
            var size = ((delegate* unmanaged[Stdcall]<void*, nint>)Method(errors, SizeSlot))(errors);
            var message = Encoding.UTF8.GetString(pointer, (int)size).TrimEnd('\0').Trim();
            Release(errors);

            if (message.Length > 0)
            {
                text = message;
            }
        }

        return text;
    }

    private static void* Method(void* instance, int slot)
    {
        return (*(void***)instance)[slot];
    }

    private static void Release(void* instance)
    {
        ((delegate* unmanaged[Stdcall]<void*, uint>)Method(instance, ReleaseSlot))(instance);
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvStdcall)])]
    private static int QueryInterface(nint* self, Guid* id, void** output)
    {
        if (*id == UnknownInterface || *id == BlobInterface)
        {
            *output = self;

            return 0;
        }

        *output = null;

        return NoInterface;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvStdcall)])]
    private static uint Reference(nint* self)
    {
        return 1;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvStdcall)])]
    private static nint BufferPointer(nint* self)
    {
        try
        {
            return self[1];
        }
        catch (Exception)
        {
            return 0;
        }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvStdcall)])]
    private static nint BufferSize(nint* self)
    {
        try
        {
            return self[ReleaseSlot];
        }
        catch (Exception)
        {
            return 0;
        }
    }
}
