// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Pe;

namespace FiveStar.Loader.Loading;

/// <summary>
/// Writes the "TLS host" DLL of an image: a DLL whose .tls template is a byte copy of the image's template (its zero fill as zero bytes) and whose
/// TLS callbacks are thunks into the image's callbacks. Layout:
/// .text at 0x1000: DllMain (<c>mov eax,1; ret</c>; ntdll delivers THREAD_ATTACH only to modules with an entry
/// point), then one thunk per callback: <c>mov eax,[rip+host _tls_index]; mov r11,&amp;image _tls_index; mov [r11],eax;
/// mov rcx,imageBase; mov rax,callback; jmp rax</c>.
/// .data at 0x2000: IMAGE_TLS_DIRECTORY64, the host's _tls_index (+0x28), the callback array (+0x30), the template,
/// and an import of kernel32!GetLastError (ntdll ignores the TLS directory of a DLL without imports).
/// .reloc: DIR64 fixups for the directory's VAs and the callback array.\n/// For an executable the callbacks start and end with the <see cref="TlsSlotZero"/> routines.
/// </summary>
internal static class TlsHostBuilder
{
    public const uint DataRva = 0x2000;

    /// <summary>The host's own _tls_index inside .data, where ntdll writes the slot it assigned.</summary>
    public const int HostIndexOffset = 0x28;

    private const uint TextRva = 0x1000;
    private const ulong PreferredBase = 0x10000000;
    private const int DirectoryOffset = 0x00;
    private const int CallbackArrayOffset = 0x30;
    private const int MinimumDataSize = 0x40;
    private const int ThunkAlignment = 16;
    private const int SlotZeroCallbacks = 2;
    private const int DataAlignment = 16;
    private const string ImportModule = "kernel32.dll";
    private const string ImportSymbol = "GetLastError";

    private const ushort Characteristics = PeFormat.FileExecutableImage | PeFormat.FileLargeAddressAware | PeFormat.FileDll;
    private const SectionCharacteristics TextFlags = SectionCharacteristics.ContainsCode | SectionCharacteristics.MemoryExecute | SectionCharacteristics.MemoryRead;
    private const SectionCharacteristics DataFlags = SectionCharacteristics.ContainsInitializedData | SectionCharacteristics.MemoryRead | SectionCharacteristics.MemoryWrite;
    private const SectionCharacteristics RelocFlags = SectionCharacteristics.ContainsInitializedData | SectionCharacteristics.MemoryDiscardable | SectionCharacteristics.MemoryRead;

    public static byte[] Build(TlsTemplate template)
    {
        int callbackArraySize = sizeof(ulong) * (CallbackCount(template) + 1);

        if (CallbackArrayOffset + callbackArraySize > PeImageBuilder.SectionAlignment)
        {
            throw new NotSupportedException("too many TLS callbacks");
        }

        var thunkRvas = new List<uint>();
        byte[] text = BuildText(template, thunkRvas);
        var relocations = new BaseRelocationBuilder();
        var (data, importTable) = BuildData(template, thunkRvas, relocations);
        byte[] reloc = relocations.Build();
        uint relocRva = DataRva + (uint)Align(data.Length, PeImageBuilder.SectionAlignment);

        var builder = new PeImageBuilder(PreferredBase, Characteristics);
        builder.SetEntryPoint(TextRva);
        builder.AddSection(new PeSectionContent(".text", TextRva, text, TextFlags));
        builder.AddSection(new PeSectionContent(".data", DataRva, data, DataFlags));
        builder.AddSection(new PeSectionContent(".reloc", relocRva, reloc, RelocFlags));

        builder.SetDirectory(DataDirectoryIndex.Import, importTable.DescriptorRva, importTable.DescriptorSize);
        builder.SetDirectory(DataDirectoryIndex.BaseRelocation, relocRva, (uint)reloc.Length);
        builder.SetDirectory(DataDirectoryIndex.Tls, DataRva + DirectoryOffset, TlsFormat.DirectorySize);

        return builder.Build();
    }

    /// <summary>The host's callbacks: the image's, plus the slot-0 enter and leave routines for an executable.</summary>
    private static int CallbackCount(TlsTemplate template)
    {
        int extra = 0;

        if (template.OwnsSlotZero)
        {
            extra = SlotZeroCallbacks;
        }

        return template.Callbacks.Count + extra;
    }

    /// <summary>
    /// The host's template size: the image's data and zero fill, plus the saved slot-0 pointer of an executable, all
    /// as initialized data. ntdll sizes the block of a LoadLibrary'd module by its raw data alone and ignores
    /// SizeOfZeroFill, so zero fill there would lie past the block's heap allocation.
    /// </summary>
    public static int TemplateSize(TlsTemplate template)
    {
        if (!template.OwnsSlotZero)
        {
            return checked(template.RawData.Length + (int)template.ZeroFill);
        }

        return TlsSlotZero.SaveOffset(template) + TlsSlotZero.SaveSize;
    }

    private static int Align(int value, int alignment)
    {
        return (value + alignment - 1) & ~(alignment - 1);
    }

    private static byte[] BuildText(TlsTemplate template, List<uint> thunkRvas)
    {
        var code = new X64Emitter();
        code.MovEax(1);
        code.Ret();
        int saveOffset = TlsSlotZero.SaveOffset(template);

        if (template.OwnsSlotZero)
        {
            code.PadTo(ThunkAlignment);
            thunkRvas.Add(TextRva + (uint)code.Length);
            TlsSlotZero.EmitEnter(code, TextRva, DataRva + HostIndexOffset, saveOffset);
        }

        foreach (ulong callback in template.Callbacks)
        {
            code.PadTo(ThunkAlignment);
            uint rva = TextRva + (uint)code.Length;
            thunkRvas.Add(rva);

            // Copy the host's slot into the image's _tls_index, then enter the callback with rcx = image base.
            code.LoadEaxRipRelative((int)(DataRva + HostIndexOffset - (rva + X64Emitter.MovEaxRipRelativeLength)));
            code.MovR11(template.IndexAddress);
            code.StoreEaxToR11();
            code.MovRcx(template.ImageBase);
            code.MovRax(callback);
            code.JmpRax();
        }

        if (template.OwnsSlotZero)
        {
            code.PadTo(ThunkAlignment);
            thunkRvas.Add(TextRva + (uint)code.Length);
            TlsSlotZero.EmitLeave(code, TextRva, DataRva + HostIndexOffset, saveOffset);
        }

        return code.ToArray();
    }

    private static (byte[] Data, ImportTableImage Imports) BuildData(TlsTemplate template, List<uint> thunkRvas, BaseRelocationBuilder relocations)
    {
        int templateOffset = Align(CallbackArrayOffset + (sizeof(ulong) * (CallbackCount(template) + 1)), DataAlignment);
        int importOffset = Align(templateOffset + TemplateSize(template), DataAlignment);
        var imports = new ImportTableBuilder();
        imports.Add(ImportModule, ImportSymbol);
        var importTable = imports.Build(DataRva + (uint)importOffset);

        var data = new byte[Math.Max(importOffset + importTable.Bytes.Length, MinimumDataSize)];
        WriteDirectory(data, template, templateOffset, relocations);
        template.RawData.CopyTo(data, templateOffset);
        importTable.Bytes.CopyTo(data, importOffset);

        for (int i = 0; i < thunkRvas.Count; i++)
        {
            int slot = CallbackArrayOffset + (sizeof(ulong) * i);
            BinaryPrimitives.WriteUInt64LittleEndian(data.AsSpan(slot), PreferredBase + thunkRvas[i]);
            relocations.AddDir64(DataRva + (uint)slot);
        }

        return (data, importTable);
    }

    private static void WriteDirectory(byte[] data, TlsTemplate template, int templateOffset, BaseRelocationBuilder relocations)
    {
        var directory = data.AsSpan(DirectoryOffset, TlsFormat.DirectorySize);
        ulong start = PreferredBase + DataRva + (uint)templateOffset;

        BinaryPrimitives.WriteUInt64LittleEndian(directory[TlsFormat.StartAddressOfRawDataOffset..], start);
        BinaryPrimitives.WriteUInt64LittleEndian(directory[TlsFormat.EndAddressOfRawDataOffset..], start + (uint)TemplateSize(template));
        BinaryPrimitives.WriteUInt64LittleEndian(directory[TlsFormat.AddressOfIndexOffset..], PreferredBase + DataRva + HostIndexOffset);
        BinaryPrimitives.WriteUInt64LittleEndian(directory[TlsFormat.AddressOfCallBacksOffset..], PreferredBase + DataRva + CallbackArrayOffset);
        BinaryPrimitives.WriteUInt32LittleEndian(directory[TlsFormat.CharacteristicsOffset..], template.Characteristics);

        relocations.AddDir64(DataRva + DirectoryOffset + TlsFormat.StartAddressOfRawDataOffset);
        relocations.AddDir64(DataRva + DirectoryOffset + TlsFormat.EndAddressOfRawDataOffset);
        relocations.AddDir64(DataRva + DirectoryOffset + TlsFormat.AddressOfIndexOffset);
        relocations.AddDir64(DataRva + DirectoryOffset + TlsFormat.AddressOfCallBacksOffset);
    }
}
