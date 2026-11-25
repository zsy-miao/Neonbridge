// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Cli.Archive;
using FiveStar.Formats.Common;
using FiveStar.Formats.Platform;

namespace FiveStar.Cli.Platform;

/// <summary>The platform reports: DLL import census, PE summaries, the XVD header and registry hives.</summary>
internal sealed class PlatformReports
{
    private const int FunctionsShown = 60;
    private const int ExportsShown = 25;
    private const int SectionNameWidth = 10;
    private const int TwoDecimals = 2;
    private const int IndentWidth = 2;
    private const int HexPairWidth = 2;
    private const ulong FileTimeTicksPerMicrosecond = 10;
    private const double BytesPerMebibyte = 1 << 20;

    // System DLLs listed after the game's own dependencies (with every api-ms-* set).
    private static readonly HashSet<string> SystemDlls = new(StringComparer.Ordinal)
    {
        "kernel32.dll", "ntdll.dll", "user32.dll", "advapi32.dll", "ole32.dll", "oleaut32.dll", "ws2_32.dll", "bcrypt.dll",
        "crypt32.dll", "shell32.dll", "shlwapi.dll", "winhttp.dll", "version.dll", "dbghelp.dll", "secur32.dll",
    };

    private static readonly string[] VersionKeys = ["FileDescription", "FileVersion", "ProductName", "CompanyName", "OriginalFilename"];

    private readonly TextWriter _output;

    public PlatformReports(TextWriter output)
    {
        _output = output;
    }

    /// <summary>Which functions of which DLLs the modules in a folder import, system DLLs last.</summary>
    public void Imports(string directory)
    {
        var functions = new Dictionary<string, SortedSet<string>>(StringComparer.Ordinal);
        var users = new Dictionary<string, SortedSet<string>>(StringComparer.Ordinal);
        var modules = new List<string>(Directory.GetFiles(directory, "*.dll"));
        modules.Sort(StringComparer.Ordinal);

        foreach (var module in modules)
        {
            var image = new PeImage(File.ReadAllBytes(module));
            var all = new List<PeImport>(PeImportReader.Imports(image));
            all.AddRange(PeImportReader.DelayImports(image));

            foreach (var import in all)
            {
                var dll = import.Dll.ToLowerInvariant();
                Set(users, dll).Add(Path.GetFileName(module));

                foreach (var function in import.Functions)
                {
                    Set(functions, dll).Add(function);
                }
            }
        }

        var dlls = new List<string>();

        foreach (var pair in functions)
        {
            if (pair.Value.Count > 0)
            {
                dlls.Add(pair.Key);
            }
        }

        dlls.Sort(CompareDlls);

        foreach (var dll in dlls)
        {
            _output.WriteLine($"== {dll}  ({functions[dll].Count} funcs)  used by: {string.Join(", ", users[dll])}");
            _output.WriteLine("    " + string.Join(", ", functions[dll].Take(FunctionsShown)));
        }
    }

    /// <summary>Header, sections, imports, exports, version strings, PDB path and resource types of each module.</summary>
    public void Summaries(IReadOnlyList<string> paths)
    {
        foreach (var path in paths)
        {
            var image = new PeImage(File.ReadAllBytes(path));
            var stamp = DateTime.UnixEpoch.AddSeconds(image.TimeStamp).ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture);
            _output.WriteLine($"=== {Path.GetFileName(path)} machine={DecimalText.Hex(image.Machine)} ts={stamp} base={DecimalText.Hex((long)image.ImageBase)} subsys={image.Subsystem} entry={DecimalText.Hex(image.EntryPoint)} dllchar={DecimalText.Hex(image.DllCharacteristics)}");
            PrintSections(image);
            PrintImportsAndExports(image);

            foreach (var table in PeVersionInfo.StringTables(image))
            {
                PrintVersion(table);
            }

            foreach (var pdb in PeDetails.PdbPaths(image))
            {
                _output.WriteLine("   pdb: " + pdb);
            }

            PrintResources(image);
        }
    }

    public void Xvd(string path)
    {
        using var source = new MappedFileSource(path);
        var header = XvdHeader.Read(source.Read(0, XvdHeader.ReadLength));
        var created = DateTime.FromFileTimeUtc(0).AddTicks((long)(header.Created / FileTimeTicksPerMicrosecond * FileTimeTicksPerMicrosecond));
        var type = XvdTypeName(header.XvdType);
        var content = XvdNames.ContentTypeName(header.ContentType) ?? "None";
        var flags = new List<string>();

        foreach (var (bit, name) in XvdNames.Flags)
        {
            if ((header.VolumeFlags & bit) != 0)
            {
                flags.Add(ReportText.Quote(name));
            }
        }

        if (flags.Count == 0)
        {
            flags.Add(ReportText.Quote("(none)"));
        }

        _output.WriteLine("magic " + BytesText(header.Magic));
        _output.WriteLine($"VolumeFlags={DecimalText.Hex(header.VolumeFlags)} FormatVersion={header.FormatVersion} Created={PythonTime(created)} DriveSize={DecimalText.Hex((long)header.DriveSize)} ({DecimalText.Fixed(header.DriveSize / BytesPerMebibyte, 0)} MiB)");
        _output.WriteLine($"VDUID {header.VolumeId} UDUID {header.UserId}");

        _output.WriteLine("TopHashBlockHash " + Convert.ToHexStringLower(header.TopHashBlockHash));
        _output.WriteLine("OriginalXvcDataHash " + Convert.ToHexStringLower(header.OriginalXvcDataHash));
        _output.WriteLine($"XvdType={header.XvdType} ({type}) ContentType={header.ContentType} ({content}) EmbeddedXvdLen={DecimalText.Hex(header.EmbeddedXvdLength)} UserDataLen={DecimalText.Hex(header.UserDataLength)} XvcDataLen={DecimalText.Hex(header.XvcDataLength)} DynHeaderLen={DecimalText.Hex(header.DynamicHeaderLength)} BlockSize={DecimalText.Hex(header.BlockSize)}");
        _output.WriteLine($"flags: [{string.Join(", ", flags)}]");
        _output.WriteLine("file size " + DecimalText.Hex(source.Length));
        _output.WriteLine("@0x2A0..: " + string.Join(' ', Convert.ToHexStringLower(header.Unknown2A0).Chunk(HexPairWidth).Select(pair => new string(pair))));
    }

    public void Registry(string path)
    {
        PrintKey(RegistryHive.Read(File.ReadAllBytes(path)), 0);
    }

    private void PrintKey(RegistryKey key, int depth)
    {
        var indent = new string(' ', depth * IndentWidth);
        _output.WriteLine($"{indent}[{key.Name}]");

        foreach (var value in key.Values)
        {
            var name = value.Name.Length == 0 ? "(default)" : value.Name;
            _output.WriteLine($"{indent}  {name} ({RegistryText.TypeName(value.Type)}) = {RegistryText.Value(value)}");
        }

        foreach (var child in key.Subkeys)
        {
            PrintKey(child, depth + 1);
        }
    }

    private void PrintSections(PeImage image)
    {
        foreach (var section in image.Sections)
        {
            var start = image.RawStart(section);
            var length = (int)Math.Max(0, Math.Min(section.RawSize, image.Data.Length - start));
            var entropy = ByteEntropy.BitsPerByte(image.Data.AsSpan((int)Math.Min(start, image.Data.Length), length));
            _output.WriteLine($"   {section.Name,-SectionNameWidth} va={DecimalText.Hex(section.VirtualAddress)} vs={DecimalText.Hex(section.VirtualSize)} raw={DecimalText.Hex(section.RawSize)} ent={DecimalText.Fixed(entropy, TwoDecimals)}");
        }
    }

    private void PrintImportsAndExports(PeImage image)
    {
        var imports = PeImportReader.Imports(image);

        if (imports.Count > 0)
        {
            var parts = new List<string>();

            foreach (var import in imports)
            {
                parts.Add($"{import.Dll}({import.Functions.Count})");
            }

            _output.WriteLine("   imports: " + string.Join(", ", parts));
        }

        var exports = PeDetails.Exports(image);

        if (exports.Count > 0)
        {
            _output.WriteLine($"   exports({exports.Count}): {string.Join(", ", exports.Take(ExportsShown))}");
        }
    }

    private void PrintVersion(OrderedMap<string, string> table)
    {
        var parts = new List<string>();

        for (var index = 0; index < table.Count; index++)
        {
            if (VersionKeys.Contains(table.Keys[index]))
            {
                parts.Add($"{ReportText.Quote(table.Keys[index])}: {ReportText.Quote(table.Values[index])}");
            }
        }

        _output.WriteLine($"   ver: {{{string.Join(", ", parts)}}}");
    }

    private void PrintResources(PeImage image)
    {
        var types = PeDetails.ResourceTypes(image);

        if (types.Count == 0)
        {
            return;
        }

        var parts = new List<string>();

        foreach (var type in types)
        {
            parts.Add(RegistryText.ResourceTypeText(type));
        }

        _output.WriteLine($"   resources: [{string.Join(", ", parts)}]");
    }

    private static string XvdTypeName(uint type)
    {
        switch (type)
        {
            case 0:
                return "Fixed";
            case 1:
                return "Dynamic";
            default:
                return "?";
        }
    }

    private static int CompareDlls(string left, string right)
    {
        var leftSystem = left.StartsWith("api-ms", StringComparison.Ordinal) || SystemDlls.Contains(left);
        var rightSystem = right.StartsWith("api-ms", StringComparison.Ordinal) || SystemDlls.Contains(right);

        if (leftSystem != rightSystem)
        {
            return leftSystem.CompareTo(rightSystem);
        }

        return string.CompareOrdinal(left, right);
    }

    private static SortedSet<string> Set(Dictionary<string, SortedSet<string>> map, string key)
    {
        if (!map.TryGetValue(key, out var set))
        {
            set = new SortedSet<string>(StringComparer.Ordinal);
            map[key] = set;
        }

        return set;
    }

    private static string PythonTime(DateTime time)
    {
        var text = time.ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture);
        var microseconds = time.Ticks % TimeSpan.TicksPerSecond / TimeSpan.TicksPerMicrosecond;

        if (microseconds == 0)
        {
            return text;
        }

        return text + "." + microseconds.ToString("D6", CultureInfo.InvariantCulture);
    }

    private static string BytesText(byte[] bytes)
    {
        var text = new System.Text.StringBuilder("b'");

        foreach (var value in bytes)
        {
            RegistryText.AppendByte(text, value);
        }

        return text.Append('\'').ToString();
    }
}
