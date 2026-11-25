// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>A drawable or geometry does not match the layout the model readers know.</summary>
public sealed class ModelFormatException : Exception
{
    public ModelFormatException(string message)
        : base(message)
    {
    }
}
