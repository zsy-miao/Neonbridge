// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>A blob's name and bytes; in an update, null bytes mean "delete the blob".</summary>
internal sealed record SaveBlobData(string Name, byte[]? Data);
