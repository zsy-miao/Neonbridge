// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>Which XGameSave *Async function a <see cref="SaveOperation"/> carries out.</summary>
internal enum SaveOperationKind
{
    InitializeProvider,
    RemainingQuota,
    DeleteContainer,
    ReadBlobs,
    SubmitUpdate,
}
