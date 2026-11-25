// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Tests.Support;
using Microsoft.Win32.SafeHandles;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>Mod folders laid over the package root, seen through the CreateFileW entry point mapped code imports.</summary>
public sealed unsafe class FileOverlayTests
{
    private const uint GenericRead = 0x80000000;
    private const uint FileShareRead = 1;
    private const uint OpenExisting = 3;
    private const int ErrorFileNotFound = 2;

    [Fact]
    public void A_package_file_opens_from_the_first_mod_folder_that_has_it_and_else_from_the_package_also_through_drive_G()
    {
        string root = Path.Combine(Path.GetTempPath(), "fivestar-overlay-tests", Guid.NewGuid().ToString("N"));
        string package = Path.Combine(root, "package");
        Write(package, "a.txt", "package a");
        Write(package, "b.txt", "package b");
        Write(package, "c.txt", "package c");

        Write(Path.Combine(root, "high"), "a.txt", "high a");
        Write(Path.Combine(root, "low"), "a.txt", "low a");
        Write(Path.Combine(root, "low"), "b.txt", "low b");

        try
        {
            using var test = TestRuntime.Create(null, package);
            test.Runtime.Overlay.AddFolder(Path.Combine(root, "high"));
            test.Runtime.Overlay.AddFolder(Path.Combine(root, "low"));
            var createFile = (delegate* unmanaged<char*, uint, uint, nint, uint, uint, nint, nint>)test.Shim("kernel32.dll", "CreateFileW");

            Assert.Equal("high a", Open(createFile, Path.Combine(package, "data", "a.txt")));
            Assert.Equal("low b", Open(createFile, Path.Combine(package, "data", "b.txt")));
            Assert.Equal("package c", Open(createFile, Path.Combine(package, "data", "c.txt")));
            Assert.Equal($"error {ErrorFileNotFound}", Open(createFile, Path.Combine(package, "data", "d.txt")));

            // The console's package drive is the package root, mods included.
            Assert.Equal("high a", Open(createFile, @"G:\data\a.txt"));
            Assert.Equal("package c", Open(createFile, "g:/data/c.txt"));
        }
        finally
        {
            Directory.Delete(root, true);
        }
    }

    private static void Write(string folder, string name, string text)
    {
        Directory.CreateDirectory(Path.Combine(folder, "data"));
        File.WriteAllText(Path.Combine(folder, "data", name), text);
    }

    /// <summary>The file's text, or "error n" with the last error the caller sees.</summary>
    private static string Open(delegate* unmanaged<char*, uint, uint, nint, uint, uint, nint, nint> createFile, string path)
    {
        nint handle;

        fixed (char* name = path)
        {
            Marshal.SetLastSystemError(0);
            handle = createFile(name, GenericRead, FileShareRead, 0, OpenExisting, 0, 0);
        }

        if (handle == -1)
        {
            return $"error {Marshal.GetLastSystemError()}";
        }

        using var stream = new FileStream(new SafeFileHandle(handle, true), FileAccess.Read);
        using var reader = new StreamReader(stream);

        return reader.ReadToEnd();
    }
}
