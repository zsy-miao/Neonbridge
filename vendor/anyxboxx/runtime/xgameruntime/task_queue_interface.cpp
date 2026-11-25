// Interface XTaskQueue / XAsync / XThread du runtime GDK, servie par AnyXboxX.
//
// Le jeu (xgameruntime.lib) obtient l'objet par QueryApiImpl, puis appelle la
// méthode n° k avec this suivi des arguments de la fonction publique (vérifié
// au désassemblage). Les numéros viennent de gdk_api_table.inc ; le travail est
// fait par le module Task de libHttpClient (MIT, third_party/libhttpclient-task).
#include <windows.h>

#include "XAsync.h"
#include "XAsyncProvider.h"
#include "XTaskQueue.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

// --- XThread : marquage « sensible au temps » propre à chaque thread -------------
thread_local bool t_time_sensitive = false;

HRESULT XThreadSetTimeSensitive(bool value) noexcept {
  t_time_sensitive = value;
  return S_OK;
}

HRESULT XThreadVerifyNotTimeSensitive() noexcept { return t_time_sensitive ? E_FAIL : S_OK; }

void XThreadAssertNotTimeSensitive() noexcept {
  if (t_time_sensitive) ANYXBOXX_TRACE("[runtime] appel bloquant depuis un thread sensible au temps");
}

bool XThreadIsTimeSensitive() noexcept { return t_time_sensitive; }

void* const kMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                              // 0-2
    ANYXBOXX_METHOD(XAsyncGetStatus),                       // 3
    ANYXBOXX_METHOD(XAsyncGetResultSize),                   // 4
    ANYXBOXX_METHOD(XAsyncCancel),                          // 5
    ANYXBOXX_METHOD(XAsyncRun),                             // 6
    ANYXBOXX_METHOD(XAsyncBegin),                           // 7
    ANYXBOXX_NOT_IMPLEMENTED,                               // 8 : aucune fonction publique
    ANYXBOXX_METHOD(XAsyncSchedule),                        // 9
    ANYXBOXX_METHOD(XAsyncComplete),                        // 10
    ANYXBOXX_METHOD(XAsyncGetResult),                       // 11
    ANYXBOXX_METHOD(XTaskQueueCreate),                      // 12
    ANYXBOXX_METHOD(XTaskQueueCreateComposite),             // 13
    ANYXBOXX_METHOD(XTaskQueueGetPort),                     // 14
    ANYXBOXX_METHOD(XTaskQueueDuplicateHandle),             // 15
    ANYXBOXX_METHOD(XTaskQueueDispatch),                    // 16
    ANYXBOXX_METHOD(XTaskQueueCloseHandle),                 // 17
    ANYXBOXX_METHOD(XTaskQueueSubmitCallback),              // 18
    ANYXBOXX_METHOD(XTaskQueueSubmitDelayedCallback),       // 19
    ANYXBOXX_METHOD(XTaskQueueRegisterWaiter),              // 20
    ANYXBOXX_METHOD(XTaskQueueUnregisterWaiter),            // 21
    ANYXBOXX_METHOD(XTaskQueueTerminate),                   // 22
    ANYXBOXX_METHOD(XTaskQueueRegisterMonitor),             // 23
    ANYXBOXX_METHOD(XTaskQueueUnregisterMonitor),           // 24
    ANYXBOXX_METHOD(XTaskQueueGetCurrentProcessTaskQueue),  // 25
    ANYXBOXX_METHOD(XTaskQueueSetCurrentProcessTaskQueue),  // 26
    ANYXBOXX_METHOD(XThreadSetTimeSensitive),               // 27
    ANYXBOXX_METHOD(XThreadVerifyNotTimeSensitive),         // 28
    ANYXBOXX_METHOD(XThreadAssertNotTimeSensitive),         // 29
    ANYXBOXX_METHOD(XThreadIsTimeSensitive),                // 30
};

const anyxboxx::StaticInterface kTaskQueue{kMethods};

}  // namespace

namespace anyxboxx {

void* TaskQueueInterface() { return const_cast<StaticInterface*>(&kTaskQueue); }

}  // namespace anyxboxx
