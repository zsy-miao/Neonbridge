// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>XGameSaveFilesGetFolderWithUiAsync: creates the folder; the result is its UTF-8 path with the NUL.</summary>
internal sealed class SaveFolderOperation : ManagedAsyncOperation
{
    private readonly string _folder;
    private readonly byte[] _path;

    public SaveFolderOperation(string folder)
    {
        _folder = folder;
        _path = Encoding.UTF8.GetBytes(folder + "\0");
    }

    public override AsyncWorkResult Work()
    {
        Directory.CreateDirectory(_folder);

        return new AsyncWorkResult(HResult.Ok, (nuint)_path.Length);
    }

    public override unsafe int Write(byte* buffer, nuint size)
    {
        _path.CopyTo(new Span<byte>(buffer, _path.Length));

        return HResult.Ok;
    }
}
