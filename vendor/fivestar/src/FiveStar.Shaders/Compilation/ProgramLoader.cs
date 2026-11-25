// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Disassembly;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Compilation;

/// <summary>
/// Loads an extracted FSSM program for translation: decodes its header, disassembles its code (and, for a geometry
/// program, its copy shader) and reads signatures and PSV0 from the DXBC container with the same name in the sibling
/// <c>dxbc</c> folder, when there is one.
/// </summary>
public sealed class ProgramLoader
{
    private const int ReasonLength = 60;
    private const int Wave64 = 64;

    private readonly IDisassembler _disassembler;

    public ProgramLoader(IDisassembler disassembler)
    {
        _disassembler = disassembler;
    }

    /// <summary>The container that belongs to an FSSM file: <c>&lt;fssm folder&gt;/../dxbc/&lt;name&gt;.dxbc</c>.</summary>
    public static string ContainerPath(string programPath)
    {
        var folder = Path.GetDirectoryName(Path.GetDirectoryName(Path.GetFullPath(programPath))) ?? string.Empty;

        return Path.Combine(folder, ShaderArchiveExtractor.ContainerFolder, Path.GetFileNameWithoutExtension(programPath) + ".dxbc");
    }

    /// <summary>Loads one program; <paramref name="stages"/> (when given) limits which stages are loaded.</summary>
    public LoadedProgram Load(string programPath, IReadOnlySet<ShaderStage>? stages)
    {
        var name = Path.GetFileNameWithoutExtension(programPath);
        var bytes = File.ReadAllBytes(programPath);
        ShaderConfig config;

        try
        {
            config = FssmHeader.Decode(bytes);
        }
        catch (Exception exception) when (exception is InvalidDataException or ArgumentOutOfRangeException)
        {
            return new LoadedProgram(name, null, null, ProgramStatus.HeaderFailure, Cut(exception.Message));
        }

        var stage = ShaderStages.FromHardware(config.Stage);

        if (stage is null || (stages is not null && !stages.Contains(stage.Value)))
        {
            return new LoadedProgram(name, stage, null, ProgramStatus.SkipStage, null);
        }

        try
        {
            return new LoadedProgram(name, stage, Build(programPath, bytes, config, stage.Value), null, null);
        }
        catch (Exception exception) when (exception is InvalidOperationException or InvalidDataException or IOException)
        {
            return new LoadedProgram(name, stage, null, ProgramStatus.DisassemblyFailure, Cut(exception.Message));
        }
    }

    private ShaderProgram Build(string programPath, byte[] bytes, ShaderConfig config, ShaderStage stage)
    {
        var code = FssmHeader.ProgramCode(bytes);
        var instructions = _disassembler.Disassemble(code, config.WaveSize ?? Wave64);
        var signatures = ShaderSignatures.None;
        PipelineStateInfo? pipelineState = null;
        var containerPath = ContainerPath(programPath);

        if (File.Exists(containerPath))
        {
            var container = DxbcContainer.Parse(File.ReadAllBytes(containerPath));
            signatures = new ShaderSignatures(Signature(container, "ISG1"), Signature(container, "OSG1"));
            var psv = container.Find("PSV0");

            if (psv is not null)
            {
                pipelineState = PipelineStateParser.Parse(psv.Data.Span);
            }
        }

        var program = new ShaderProgram(stage, instructions, code, signatures, config, pipelineState);

        if (stage == ShaderStage.Geometry)
        {
            program = program with { CopyShader = _disassembler.Disassemble(FssmHeader.CopyShaderCode(bytes), Wave64) };
        }

        return program;
    }

    private static IReadOnlyList<SignatureElement> Signature(DxbcContainer container, string fourCc)
    {
        var chunk = container.Find(fourCc);

        if (chunk is null)
        {
            return [];
        }

        return SignatureParser.Parse(chunk.Data.Span);
    }

    private static string Cut(string message)
    {
        if (message.Length <= ReasonLength)
        {
            return message;
        }

        return message[..ReasonLength];
    }
}
