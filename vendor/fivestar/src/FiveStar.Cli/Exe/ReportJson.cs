// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Common;

namespace FiveStar.Cli.Exe;

/// <summary>Renders an <see cref="ExeAnalysis"/> as indented JSON, through the repository's own JSON writer.</summary>
internal static class ReportJson
{
    private const int Indent = 2;

    public static string Build(ExeAnalysis analysis)
    {
        var root = Obj(
            ("file", FileObject(analysis)),
            ("sections", Sections(analysis.Sections)),
            ("tlsCallbacks", TlsCallbacks(analysis.TlsCallbacks)),
            ("pdata", Obj(("functions", analysis.PdataFunctions), ("bytes", analysis.PdataBytes))),
            ("exports", Strings(analysis.Exports)),
            ("loaderClassified", analysis.LoaderClassified),
            ("imports", Imports(analysis.Imports)),
            ("gdk", Gdk(analysis)),
            ("graphics", Imports(analysis.GraphicsImports)),
            ("archiveReferences", References(analysis.ArchiveReferences)),
            ("shaderReferences", References(analysis.ShaderReferences)),
            ("dayOne", DayOne(analysis.DayOneGaps)));

        return JsonText.Write(root, Indent) + "\n";
    }

    private static OrderedMap<string, object?> FileObject(ExeAnalysis analysis)
    {
        return Obj(
            ("name", analysis.FileName),
            ("bytes", analysis.FileLength),
            ("is64Bit", analysis.Is64Bit),
            ("machine", analysis.Machine),
            ("entryPoint", analysis.EntryPoint),
            ("imageBase", analysis.ImageBase),
            ("timeStamp", analysis.TimeStamp),
            ("subsystem", analysis.Subsystem),
            ("dllCharacteristics", analysis.DllCharacteristics));
    }

    private static List<object?> Sections(IReadOnlyList<SectionInfo> sections)
    {
        var list = new List<object?>();

        foreach (var section in sections)
        {
            list.Add(Obj(
                ("name", section.Name),
                ("virtualAddress", section.VirtualAddress),
                ("virtualSize", section.VirtualSize),
                ("rawSize", section.RawSize),
                ("entropy", DecimalText.Round(section.Entropy, Indent)),
                ("highEntropy", section.HighEntropy),
                ("console", section.Console)));
        }

        return list;
    }

    private static List<object?> Imports(IReadOnlyList<ImportGap> imports)
    {
        var list = new List<object?>();

        foreach (var import in imports)
        {
            list.Add(Obj(
                ("dll", import.Dll),
                ("function", import.Function),
                ("delayed", import.Delayed),
                ("kind", import.Kind),
                ("detail", import.Detail),
                ("sites", import.Sites)));
        }

        return list;
    }

    private static OrderedMap<string, object?> Gdk(ExeAnalysis analysis)
    {
        return Obj(
            ("lib", analysis.GdkLibName),
            ("thunkCount", analysis.GdkThunkCount),
            ("namedCount", analysis.GdkNamedCount),
            ("calls", GdkCalls(analysis.GdkCalls)));
    }

    private static List<object?> GdkCalls(IReadOnlyList<GdkCall> calls)
    {
        var list = new List<object?>();

        foreach (var call in calls)
        {
            list.Add(Obj(
                ("name", call.Name),
                ("clsid", call.Clsid),
                ("iid", call.Iid),
                ("slot", call.Slot),
                ("thunk", call.Thunk),
                ("sites", call.Sites),
                ("shimmed", call.Shimmed)));
        }

        return list;
    }

    private static List<object?> References(IReadOnlyList<SymbolReference> references)
    {
        var list = new List<object?>();

        foreach (var reference in references)
        {
            list.Add(Obj(
                ("what", reference.What),
                ("rva", reference.Rva),
                ("section", reference.Section),
                ("note", reference.Note)));
        }

        return list;
    }

    private static List<object?> DayOne(IReadOnlyList<GapItem> gaps)
    {
        var list = new List<object?>();

        foreach (var gap in gaps)
        {
            list.Add(Obj(("area", gap.Area), ("what", gap.What), ("why", gap.Why)));
        }

        return list;
    }

    private static List<object?> TlsCallbacks(IReadOnlyList<uint> callbacks)
    {
        var list = new List<object?>();

        foreach (var callback in callbacks)
        {
            list.Add(callback);
        }

        return list;
    }

    private static List<object?> Strings(IReadOnlyList<string> values)
    {
        var list = new List<object?>();

        foreach (var value in values)
        {
            list.Add(value);
        }

        return list;
    }

    private static OrderedMap<string, object?> Obj(params (string Key, object? Value)[] pairs)
    {
        var map = new OrderedMap<string, object?>();

        foreach (var pair in pairs)
        {
            map.Set(pair.Key, pair.Value);
        }

        return map;
    }
}
