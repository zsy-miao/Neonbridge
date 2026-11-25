// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Containers;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>
/// The ISG1 input signatures of extracted programs, read from the DXBC container next to each FSSM program
/// (<see cref="ProgramLoader.ContainerPath"/>). A cache pipeline's shader is a bare code block, so this is where its
/// vertex shader's semantics come from. Each container is read once.
/// </summary>
internal sealed class ProgramSignatures
{
    private const string ProgramExtension = ".fssm";

    private readonly object _gate = new();
    private readonly string _programFolder;
    private readonly Dictionary<string, IReadOnlyList<SignatureElement>?> _inputs = new(StringComparer.Ordinal);

    /// <param name="programFolder">Folder of the extracted <c>.fssm</c> programs (--shader-dir).</param>
    public ProgramSignatures(string programFolder)
    {
        _programFolder = programFolder;
    }

    /// <summary>The input signature of program <paramref name="name"/>, or null when its container is missing.</summary>
    public IReadOnlyList<SignatureElement>? Inputs(string name)
    {
        lock (_gate)
        {
            if (_inputs.TryGetValue(name, out var known))
            {
                return known;
            }

            var path = ProgramLoader.ContainerPath(Path.Combine(_programFolder, DebugLayer.MeshProbes.Source(name) + ProgramExtension));
            IReadOnlyList<SignatureElement>? inputs = null;

            if (File.Exists(path))
            {
                inputs = InputLayoutMapping.Inputs(File.ReadAllBytes(path));
            }

            _inputs[name] = inputs;

            return inputs;
        }
    }
}
