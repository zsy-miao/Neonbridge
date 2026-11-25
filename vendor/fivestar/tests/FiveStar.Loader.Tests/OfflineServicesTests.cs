// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Tests.Support;
using Xunit;

namespace FiveStar.Loader.Tests;

/// <summary>The offline store, package chunks and speech through their native entry points and XAsync.</summary>
public sealed unsafe class OfflineServicesTests
{
    private const int StoreNetworkError = unchecked((int)0x89245301);
    private const int Abort = unchecked((int)0x80004004);
    private const int NotSupported = unchecked((int)0x80070032);

    // XPackageInstallationProgress: launchable and completed bools after the three byte counts.
    private const int ProgressSize = 32;
    private const int ProgressCompletedOffset = 25;

    [Fact]
    public void Store_completes_offline_and_never_reports_a_license()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint context = 0;
        ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XStoreCreateContext"))(0, &context);
        var status = (delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus");

        // An add-on license: the completion routine runs through the task queue with the store's offline error.
        nint acquire = TestNative.AsyncBlock(0, (nint)(delegate* unmanaged<nint, void>)&NativeCallbacks.AsyncCompleted);
        int begun = ((delegate* unmanaged<nint, nint, nint, int>)test.Gdk("XStoreAcquireLicenseForDurablesAsync"))(context, 0, acquire);
        int acquired = status(acquire, 1);
        nint license = 0x1234;
        int result = ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XStoreAcquireLicenseForDurablesResult"))(acquire, &license);
        byte valid = ((delegate* unmanaged<nint, byte>)test.Gdk("XStoreIsLicenseValid"))(license);

        // Entitlements: an empty list.
        nint entitled = TestNative.AsyncBlock(0, 0);
        ((delegate* unmanaged<nint, uint, uint, nint, int>)test.Gdk("XStoreQueryEntitledProductsAsync"))(context, 0, 25, entitled);
        status(entitled, 1);

        nint query = 0;
        int queried = ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XStoreQueryEntitledProductsResult"))(entitled, &query);
        int enumerated = ((delegate* unmanaged<nint, nint, nint, int>)test.Gdk("XStoreEnumerateProductsQuery"))(query, 0, (nint)(delegate* unmanaged<nint, void>)&NativeCallbacks.AsyncCompleted);
        byte morePages = ((delegate* unmanaged<nint, byte>)test.Gdk("XStoreProductsQueryHasMorePages"))(query);

        nint purchase = TestNative.AsyncBlock(0, 0);
        ((delegate* unmanaged<nint, nint, nint, nint, nint, int>)test.Gdk("XStoreShowPurchaseUIAsync"))(context, 0, 0, 0, purchase);

        Assert.Equal(0, begun);
        Assert.Equal(StoreNetworkError, acquired);
        Assert.Equal(StoreNetworkError, result);
        Assert.Equal(0, license);
        Assert.Equal(0, valid);
        Assert.True(SpinWait.SpinUntil(() => NativeCallbacks.Completed.Contains(acquire), TimeSpan.FromSeconds(5)));

        Assert.Equal(0, queried);
        Assert.NotEqual(0, query);
        Assert.Equal(0, enumerated);
        Assert.Equal(acquire, Assert.Single(NativeCallbacks.Completed));
        Assert.Equal(0, morePages);
        Assert.Equal(Abort, status(purchase, 1));
    }

    // XStoreGameLicense (XStore.h, 104 bytes): the fields the game reads after XStoreQueryGameLicenseResult.
    private const int LicenseSize = 104;
    private const int LicenseIsActiveOffset = 0x12;
    private const int LicenseTrialOwnedOffset = 0x13;
    private const int LicenseIsTrialOffset = 0x15;
    private const int LicenseTrialIdOffset = 0x1C;
    private const int LicenseExpirationOffset = 0x60;

    [Fact]
    public void Game_license_is_an_active_full_license_at_the_offsets_the_game_reads()
    {
        using var test = TestRuntime.Create();
        nint context = 0;
        ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XStoreCreateContext"))(0, &context);
        nint block = TestNative.AsyncBlock(0, 0);

        ((delegate* unmanaged<nint, nint, int>)test.Gdk("XStoreQueryGameLicenseAsync"))(context, block);
        int done = ((delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus"))(block, 1);
        byte* license = stackalloc byte[LicenseSize];
        new Span<byte>(license, LicenseSize).Fill(0xCC);
        int result = ((delegate* unmanaged<nint, byte*, int>)test.Gdk("XStoreQueryGameLicenseResult"))(block, license);

        Assert.Equal(0, done);
        Assert.Equal(0, result);
        Assert.Equal(1, license[LicenseIsActiveOffset]);
        Assert.Equal(new byte[3], new Span<byte>(license + LicenseTrialOwnedOffset, LicenseIsTrialOffset - LicenseTrialOwnedOffset + 1).ToArray());
        Assert.Equal(0, license[LicenseTrialIdOffset]);
        Assert.Equal(long.MaxValue, *(long*)(license + LicenseExpirationOffset));
    }

    [Fact]
    public void Installation_monitor_calls_its_progress_callback_once_with_the_install_complete()
    {
        using var test = TestRuntime.Create();
        NativeCallbacks.Reset();
        nint monitor = 0;
        ((delegate* unmanaged<nint, uint, nint, uint, nint, nint*, int>)test.Gdk("XPackageCreateInstallationMonitor"))(0, 0, 0, 1000, 0, &monitor);
        ulong token = 0;
        nint callback = (nint)(delegate* unmanaged<nint, nint, void>)&NativeCallbacks.ProgressChanged;

        int registered = ((delegate* unmanaged<nint, nint, nint, ulong*, int>)test.Gdk("XPackageRegisterInstallationProgressChanged"))(monitor, 0x77, callback, &token);
        byte* progress = stackalloc byte[ProgressSize];
        ((delegate* unmanaged<nint, byte*, void>)test.Gdk("XPackageGetInstallationProgress"))(monitor, progress);

        Assert.Equal(0, registered);
        Assert.NotEqual(0UL, token);
        Assert.True(SpinWait.SpinUntil(() => !NativeCallbacks.Items.IsEmpty, TimeSpan.FromSeconds(5)));
        Assert.Equal($"77:{monitor:x}", Assert.Single(NativeCallbacks.Items));
        Assert.Equal(1, progress[ProgressCompletedOffset]);
    }

    [Fact]
    public void Install_chunks_completes_with_a_finished_monitor_and_speech_is_unavailable()
    {
        using var test = TestRuntime.Create();
        nint block = TestNative.AsyncBlock(0, 0);

        int begun = ((delegate* unmanaged<nint, uint, nint, uint, byte, nint, int>)test.Gdk("XPackageInstallChunksAsync"))(0, 0, 0, 1000, 1, block);
        int done = ((delegate* unmanaged<nint, byte, int>)test.Gdk("XAsyncGetStatus"))(block, 1);
        nint monitor = 0;
        int result = ((delegate* unmanaged<nint, nint*, int>)test.Gdk("XPackageInstallChunksResult"))(block, &monitor);

        byte* progress = stackalloc byte[ProgressSize];
        new Span<byte>(progress, ProgressSize).Clear();
        ((delegate* unmanaged<nint, byte*, void>)test.Gdk("XPackageGetInstallationProgress"))(monitor, progress);

        nint synthesizer = 0x1234;
        int speech = ((delegate* unmanaged<nint*, int>)test.Gdk("XSpeechSynthesizerCreate"))(&synthesizer);

        Assert.Equal(0, begun);
        Assert.Equal(0, done);
        Assert.Equal(0, result);

        Assert.NotEqual(0, monitor);
        Assert.Equal(1, progress[ProgressCompletedOffset]);
        Assert.Equal(NotSupported, speech);
        Assert.Equal(0, synthesizer);
    }
}
