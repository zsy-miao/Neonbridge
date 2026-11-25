// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Formats.Ttpf;

/// <summary>A texture status with its detail value, named for reports (<c>ok</c>, <c>skip_fmt_N</c>, <c>bcpack_codecN</c>, <c>approx_codecN</c>...).</summary>
public readonly record struct TextureStatus(TextureStatusKind Kind, int Detail)
{
    public static TextureStatus Ok => new(TextureStatusKind.Ok, 0);

    public bool IsOk => Kind == TextureStatusKind.Ok;

    public string Name
    {
        get
        {
            var detail = Detail.ToString(CultureInfo.InvariantCulture);

            switch (Kind)
            {
                case TextureStatusKind.Ok:
                    return "ok";
                case TextureStatusKind.UnsupportedFormat:
                    return "skip_fmt_" + detail;
                case TextureStatusKind.UnsupportedMode:
                    return "skip_mode" + detail;
                case TextureStatusKind.UnsupportedKind:
                    return "skip_kind" + detail;
                case TextureStatusKind.Size:
                    return "skip_size";
                case TextureStatusKind.CubeCopies:
                    return "skip_cube_copies";
                case TextureStatusKind.UnsupportedCodec:
                    return "skip_unsupported_codec";
                case TextureStatusKind.Approximate:
                    return "approx_codec" + detail;
                default:
                    return "bcpack_codec" + detail;
            }
        }
    }
}
