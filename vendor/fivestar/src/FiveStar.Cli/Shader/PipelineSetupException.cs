// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Cli.Shader;

/// <summary>A pipeline state could not be described for the driver (the program itself was not tried).</summary>
internal sealed class PipelineSetupException : Exception
{
    public PipelineSetupException(string message)
        : base(message)
    {
    }
}
