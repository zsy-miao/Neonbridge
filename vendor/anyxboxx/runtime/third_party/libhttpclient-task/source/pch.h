// En-tête précompilé d'AnyXboxX pour le module Task de libHttpClient (MIT).
// Remplace Source/Common/pch.h et pch_common.h : seules les définitions dont le
// module Task a besoin, sans la trace ni l'allocateur de libHttpClient.
#pragma once

#include <windows.h>
#include <objbase.h>

#include <algorithm>
#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <functional>
#include <list>
#include <memory>
#include <mutex>
#include <new>
#include <thread>
#include <vector>

#define ASSERT(condition) assert(condition)

// Définitions de httpClient/pal.h utilisées par le module Task.
#include <condition_variable>
#ifndef E_NOT_SUPPORTED
#define E_NOT_SUPPORTED __HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED)  // 0x80070032
#endif
using DefaultUnnamedMutex = std::mutex;
using DefaultUnnamedConditionVariable = std::condition_variable;

#define HC_TRACE_ERROR(area, ...) ((void)0)
#define HC_TRACE_ERROR_HR(area, hr, ...) ((void)0)
#define HC_TRACE_WARNING(area, ...) ((void)0)
#define HC_TRACE_INFORMATION(area, ...) ((void)0)
#define HC_TRACE_VERBOSE(area, ...) ((void)0)
#define ASYNC_LIB_TRACE(result, message) ((void)0)

// Codes d'erreur du runtime de jeu, ramenés aux valeurs standard (comme libHttpClient).
#define E_GAMERUNTIME_INVALID_HANDLE E_HANDLE

// Suivi des handles : désactivé, comme dans libHttpClient.
#define SYSTEM_HANDLE_DEFINE_HELPERS(a, b)
#define SystemHandleAssert(h)
#define SystemHandleMarkCreated(h)
#define SystemHandleMarkDestroyed(h)
#define USE_UNIQUE_HANDLES() (true)
#define GetCurrentRuntimeIteration() 0

#include "ResultMacros.h"

// FAIL_FAST doit arrêter le processus même en Release (assert disparaît avec NDEBUG),
// comme le fait le runtime GDK : un état incohérent ne doit pas continuer en silence.
#undef FAIL_FAST_MSG
#define FAIL_FAST_MSG(fmt, ...) __fastfail(FAST_FAIL_FATAL_APP_EXIT)

// API publiques (libHttpClient les recevait par httpClient.h).
#include "XAsync.h"
#include "XAsyncProvider.h"
#include "XTaskQueue.h"

#define CATCH_RETURN()                                    \
  catch (std::bad_alloc const&) { return E_OUTOFMEMORY; } \
  catch (...) { return E_FAIL; }
