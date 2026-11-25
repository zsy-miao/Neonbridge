// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics.CodeAnalysis;
using System.Text;
using System.Text.Json;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Translations kept in a <see cref="CacheFolder"/>. The key of a program is the hash of the translator identity (this
/// assembly, the LLVM tools when they disassemble, and the translation options) and the FSSM and container bytes. The entry holds the stage,
/// status, reason and instruction count; its HLSL is stored once per distinct text.
/// </summary>
public sealed class TranslationCache
{
    private const string EntryKind = "programs";
    private const string TextKind = "hlsl";
    private const string EntryExtension = ".json";
    private const string TextExtension = ".hlsl";
    private const string VariantProperty = "w32";
    private const string VariantReasonProperty = "w32Reason";

    private readonly CacheFolder _folder;
    private readonly byte[] _identity;
    private int _hits;
    private int _misses;

    public TranslationCache(CacheFolder folder, string identity)
    {
        _folder = folder;
        _identity = Encoding.UTF8.GetBytes(identity);
    }

    public int Hits => Volatile.Read(ref _hits);

    public int Misses => Volatile.Read(ref _misses);

    /// <summary>
    /// The identity of this translator build with its disassembler and options: the in-process
    /// <see cref="RdnaDecoder"/> when <paramref name="llvm"/> is null (it is part of this assembly), else the LLVM tools.
    /// </summary>
    [UnconditionalSuppressMessage("SingleFile", "IL3000", Justification = "Only the fivestar CLI translates; it runs from its assembly files and is not published single-file or NativeAOT.")]
    public static string Identity(LlvmToolPaths? llvm, TranslationOptions options)
    {
        var assembly = typeof(HlslTranslator).Assembly.Location;

        if (llvm is null)
        {
            return ContentHash.OfText(ContentHash.OfFiles([assembly]), nameof(RdnaDecoder), options.ToString());
        }

        var files = ContentHash.OfFiles([assembly, llvm.Assembler, llvm.Objdump]);

        return ContentHash.OfText(files, llvm.Cpu, options.ToString());
    }

    public string Key(string programPath)
    {
        var container = ProgramLoader.ContainerPath(programPath);
        byte[] containerBytes = [];

        if (File.Exists(container))
        {
            containerBytes = File.ReadAllBytes(container);
        }

        return ContentHash.Of(_identity, File.ReadAllBytes(programPath), containerBytes);
    }

    /// <summary>The cached translation of a program under its name, or null.</summary>
    public TranslatedProgram? Find(string key, string name)
    {
        var entry = _folder.Read(EntryKind, key, EntryExtension);
        var program = entry is null ? null : Parse(entry, name);

        if (program is null)
        {
            Interlocked.Increment(ref _misses);

            return null;
        }

        Interlocked.Increment(ref _hits);

        return program;
    }

    public void Store(string key, TranslatedProgram program)
    {
        var textKey = StoreText(program.Hlsl);
        var variantKey = StoreText(program.GroupWave?.Hlsl);

        _folder.Write(EntryKind, key, EntryExtension, Serialize(program, textKey, variantKey));
    }

    /// <summary>Stores an HLSL text once per distinct content and returns its key; null for no text.</summary>
    private string? StoreText(string? hlsl)
    {
        if (hlsl is null)
        {
            return null;
        }

        var text = Encoding.UTF8.GetBytes(hlsl);
        var textKey = ContentHash.Of(text);

        if (!_folder.Contains(TextKind, textKey, TextExtension))
        {
            _folder.Write(TextKind, textKey, TextExtension, text);
        }

        return textKey;
    }

    private static byte[] Serialize(TranslatedProgram program, string? textKey, string? variantKey)
    {
        using var memory = new MemoryStream();
        using var writer = new Utf8JsonWriter(memory);
        string? stage = null;

        if (program.Stage is not null)
        {
            stage = ShaderStages.ProfileName(program.Stage.Value);
        }

        writer.WriteStartObject();
        writer.WriteString("stage", stage);
        writer.WriteString("status", program.Status);
        writer.WriteString("reason", program.Reason);
        writer.WriteNumber("instructions", program.Instructions);
        writer.WriteString("hlsl", textKey);

        if (program.GroupWave is not null)
        {
            writer.WriteString(VariantProperty, variantKey);
            writer.WriteString(VariantReasonProperty, program.GroupWave.Reason);
        }

        writer.WriteEndObject();
        writer.Flush();

        return memory.ToArray();
    }

    /// <summary>The entry as a program; null when one of its HLSL texts is missing from the cache.</summary>
    private TranslatedProgram? Parse(byte[] entry, string name)
    {
        using var document = JsonDocument.Parse(entry);
        var root = document.RootElement;
        var stageName = root.GetProperty("stage").GetString();

        if (!TryReadText(root.GetProperty("hlsl").GetString(), out var hlsl))
        {
            return null;
        }

        ShaderStage? stage = stageName is null ? null : ShaderStages.FromProfileName(stageName);
        var status = root.GetProperty("status").GetString();
        var reason = root.GetProperty("reason").GetString();
        var program = new TranslatedProgram(name, stage, hlsl, status, reason, root.GetProperty("instructions").GetInt32());

        if (!root.TryGetProperty(VariantReasonProperty, out var variantReason))
        {
            return program;
        }

        if (!TryReadText(root.GetProperty(VariantProperty).GetString(), out var variant))
        {
            return null;
        }

        return program with { GroupWave = new GroupWaveVariant(variant, variantReason.GetString()) };
    }

    /// <summary>The text under a key (null for no key); false when the key names a text the cache no longer has.</summary>
    private bool TryReadText(string? textKey, out string? hlsl)
    {
        hlsl = null;

        if (textKey is null)
        {
            return true;
        }

        var text = _folder.Read(TextKind, textKey, TextExtension);

        if (text is null)
        {
            return false;
        }

        hlsl = Encoding.UTF8.GetString(text);

        return true;
    }
}
