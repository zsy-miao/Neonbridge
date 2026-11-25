// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Dxil.Container;
using FiveStar.Shaders.Dxil.Hlsl;
using FiveStar.Shaders.Dxil.Lowering;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Dxil;

/// <summary>
/// The DXIL backend: compiles translated HLSL (the translator's output and its runtime headers) straight to a DXIL
/// container, without dxc. It handles the HLSL the translator writes, not HLSL in general; anything else throws
/// <see cref="DxilBackendException"/>. docs/dxil-backend.md describes it.
/// </summary>
public static class DxilBackend
{
    // Deep chains of joins make deep SSA lookups; a compile runs on a thread with a large stack, at the caller's priority.
    private const int StackBytes = 256 * 1024 * 1024;

    private static readonly Dictionary<string, DxilShaderKind> Profiles = new(StringComparer.Ordinal)
    {
        ["cs"] = DxilShaderKind.Compute,
        ["ps"] = DxilShaderKind.Pixel,
        ["vs"] = DxilShaderKind.Vertex,
        ["ms"] = DxilShaderKind.Mesh,
        ["gs"] = DxilShaderKind.Geometry,
    };

    /// <summary>
    /// With a switch of at least this many cases in the entry point (the superstep form's dispatch), numeric locals live
    /// in memory instead of SSA slots; 0 (the default) never.
    /// </summary>
    public static int MemoryLocalsCases { get; set; }

    /// <summary>
    /// Whether constants are folded while lowering and repeated computations removed (<c>ConstantFolding</c>,
    /// <c>ValueNumbering</c>); on by default, off to compare.
    /// </summary>
    public static bool Optimize { get; set; } = true;

    /// <summary>Whether the backend compiles programs of this profile (ps, vs, cs, ms, gs).</summary>
    public static bool Supports(string profile)
    {
        return Profiles.ContainsKey(profile);
    }

    /// <summary>The shader kind of a supported profile.</summary>
    public static DxilShaderKind KindOf(string profile)
    {
        if (!Profiles.TryGetValue(profile, out var kind))
        {
            throw new DxilBackendException($"profile {profile} is not supported");
        }

        return kind;
    }

    /// <summary>The DXIL container of <paramref name="hlsl"/>, compiled for <paramref name="profile"/>.</summary>
    public static byte[] Compile(string hlsl, string profile)
    {
        return Compile(hlsl, profile, RuntimeIncludes.Read);
    }

    /// <summary>The same with included files found by <paramref name="includes"/> (by name; null when missing).</summary>
    public static byte[] Compile(string hlsl, string profile, Func<string, string?> includes)
    {
        if (!Supports(profile))
        {
            throw new DxilBackendException($"profile {profile} is not supported");
        }

        byte[]? result = null;
        Exception? failure = null;
        var thread = new Thread(
            () =>
            {
                try
                {
                    result = CompileProgram(hlsl, Profiles[profile], includes);
                }
                catch (Exception exception)
                {
                    failure = exception;
                }
            },
            StackBytes)
        {
            Priority = Thread.CurrentThread.Priority,
        };
        thread.Start();
        thread.Join();

        if (failure is DxilBackendException backend)
        {
            throw backend;
        }

        if (failure is not null)
        {
            throw new DxilBackendException($"internal error: {failure.GetType().Name}: {failure.Message}");
        }

        return result!;
    }

    private static byte[] CompileProgram(string hlsl, DxilShaderKind kind, Func<string, string?> includes)
    {
        var tokens = Preprocessor.Run(hlsl, includes);
        var declarations = Parser.Parse(tokens);
        var (module, main, description) = ShaderLowering.Lower(declarations, kind);

        return ShaderContainer.Write(module, main, description);
    }
}
