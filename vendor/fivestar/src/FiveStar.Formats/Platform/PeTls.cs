// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Platform;

/// <summary>
/// Reads the TLS callback table of a PE image (PE/COFF specification, IMAGE_TLS_DIRECTORY). The directory holds a
/// pointer to a NULL-terminated array of callback addresses; the addresses are virtual addresses, reported here as
/// RVAs. An image with no TLS directory, or an empty callback array, has no callbacks.
/// </summary>
public static class PeTls
{
    // IMAGE_TLS_DIRECTORY: AddressOfCallBacks sits at offset 12 (PE32) or 24 (PE32+); the pointers are 4 or 8 bytes.
    private const uint CallbackPointer32 = 12;
    private const uint CallbackPointer64 = 24;
    private const int MaxCallbacks = 4096;

    /// <summary>The RVAs of the TLS callbacks, in order.</summary>
    public static IReadOnlyList<uint> Callbacks(PeImage image)
    {
        var callbacks = new List<uint>();
        var (rva, _) = image.Directory(PeDirectory.Tls);

        if (rva == 0)
        {
            return callbacks;
        }

        var pointerField = image.Is64Bit ? CallbackPointer64 : CallbackPointer32;
        var arrayVa = PointerAt(image, rva + pointerField);

        if (arrayVa == 0)
        {
            return callbacks;
        }

        ReadArray(image, ToRva(image, arrayVa), callbacks);

        return callbacks;
    }

    private static void ReadArray(PeImage image, uint arrayRva, List<uint> callbacks)
    {
        var size = image.Is64Bit ? sizeof(ulong) : sizeof(uint);

        for (var index = 0; index < MaxCallbacks; index++)
        {
            var entry = PointerAt(image, (uint)(arrayRva + (index * size)));

            if (entry == 0)
            {
                break;
            }

            callbacks.Add(ToRva(image, entry));
        }
    }

    private static ulong PointerAt(PeImage image, uint rva)
    {
        if (image.Is64Bit)
        {
            return image.U64At(rva);
        }

        return image.U32At(rva);
    }

    private static uint ToRva(PeImage image, ulong virtualAddress)
    {
        return (uint)(virtualAddress - image.ImageBase);
    }
}
