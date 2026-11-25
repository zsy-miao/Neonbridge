// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Calls;

/// <summary>The image, the export's address and the runtime a recipe runs with.</summary>
internal sealed record ExportCallContext(LoadedImage Image, nint Function, LoaderRuntime Runtime);
