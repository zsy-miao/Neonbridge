#pragma once
#include "frame_abi.h"
#include <mfapi.h>

// All media objects in a probe must be released before shutting down MF.
struct MediaSession {
    MediaSession() {
        frame_abi::check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "initialize COM");
        const HRESULT hr = MFStartup(MF_VERSION);
        if (FAILED(hr)) {
            CoUninitialize();
            frame_abi::check(hr, "MFStartup");
        }
    }
    ~MediaSession() {
        MFShutdown();
        CoUninitialize();
    }
    MediaSession(const MediaSession&) = delete;
    MediaSession& operator=(const MediaSession&) = delete;
};
