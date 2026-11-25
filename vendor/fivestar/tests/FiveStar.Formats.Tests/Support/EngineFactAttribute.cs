// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Ttpf.BcPack;
using Xunit;

namespace FiveStar.Formats.Tests.Support;

/// <summary>A test of one BCPack lane engine: skipped when this processor cannot run it.</summary>
[AttributeUsage(AttributeTargets.Method)]
public sealed class EngineFactAttribute : FactAttribute
{
    public EngineFactAttribute(BcPackEngine engine)
    {
        if (!BcPackDecoder.IsSupported(engine))
        {
            Skip = $"this processor cannot run the {engine} engine";
        }
    }
}
