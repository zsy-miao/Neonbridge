// XCRDAPI.dll d'AnyXboxX : DStorageGetFactory minimal (note de premier démarrage, W7).
//
// Le titre demande une factory DirectStorage au premier usage d'un fichier. La factory suit la
// disposition de la console (IDStorageFactoryX, slots 0 à 8 : IUnknown, CreateQueue, OpenFile,
// CreateStatusArray, SetCpuAffinity, SetDebugFlags, SetStagingBufferSize). Disposition d'après
// TomBox (crates/compat/xcrdapi/src/objects.rs, GPL-3.0-or-later), lue pour la table de slots seulement.
//
// Ce qui est fait : la factory est rendue pour son IID et pour IUnknown ; SetStagingBufferSize (slot 8,
// +0x40) rend S_OK, que le titre teste, sans rien allouer : la lecture de fichier n'existe pas encore,
// donc aucune taille de transit n'est utilisée. Ce qui n'est pas fait rend E_NOTIMPL, sortie mise à NULL,
// et se journalise.
#include <windows.h>
#include <unknwn.h>

#include <cstdint>
#include <cstring>

#include "trace.h"

namespace {

constexpr HRESULT kNotImplemented = 0x8000'4001;
constexpr HRESULT kNoInterface = 0x8000'4002;
constexpr HRESULT kPointer = 0x8000'4003;

// {FB3B54AC-BF82-463A-B777-73195837864D} : IDStorageFactoryX de la console.
constexpr GUID kIidFactoryX = {0xfb3b'54ac, 0xbf82, 0x463a, {0xb7, 0x77, 0x73, 0x19, 0x58, 0x37, 0x86, 0x4d}};

struct Factory;

struct FactoryVtable {
  HRESULT(WINAPI* query_interface)(Factory*, REFIID, void**);
  ULONG(WINAPI* add_ref)(Factory*);
  ULONG(WINAPI* release)(Factory*);
  HRESULT(WINAPI* create_queue)(Factory*, const void* desc, REFIID riid, void** out);
  HRESULT(WINAPI* open_file)(Factory*, const wchar_t* path, REFIID riid, void** out);
  HRESULT(WINAPI* create_status_array)(Factory*, std::uint32_t capacity, const char* name, REFIID riid, void** out);
  HRESULT(WINAPI* set_cpu_affinity)(Factory*, std::uint64_t mask);
  void(WINAPI* set_debug_flags)(Factory*, std::uint32_t flags);
  HRESULT(WINAPI* set_staging_buffer_size)(Factory*, std::uint32_t size);
};

struct Factory {
  const FactoryVtable* vtable;
};

bool SameIid(REFIID a, const GUID& b) {
  return a.Data1 == b.Data1 && a.Data2 == b.Data2 && a.Data3 == b.Data3 &&
         std::memcmp(a.Data4, b.Data4, sizeof b.Data4) == 0;
}

HRESULT WINAPI FactoryQueryInterface(Factory* self, REFIID riid, void** out) {
  if (out == nullptr) return kPointer;
  *out = nullptr;
  if (!SameIid(riid, kIidFactoryX) && !SameIid(riid, IID_IUnknown)) return kNoInterface;
  *out = self;
  return S_OK;
}

// La factory est un objet statique : le compteur ne sert qu'à respecter le contrat COM.
ULONG WINAPI FactoryAddRef(Factory*) { return 1; }
ULONG WINAPI FactoryRelease(Factory*) { return 1; }

HRESULT WINAPI FactoryCreateQueue(Factory*, const void*, REFIID, void** out) {
  if (out != nullptr) *out = nullptr;
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::CreateQueue : non implémenté (E_NOTIMPL)");
  return kNotImplemented;
}

HRESULT WINAPI FactoryOpenFile(Factory*, const wchar_t*, REFIID, void** out) {
  if (out != nullptr) *out = nullptr;
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::OpenFile : lecture de fichier non implémentée (E_NOTIMPL)");
  return kNotImplemented;
}

HRESULT WINAPI FactoryCreateStatusArray(Factory*, std::uint32_t, const char*, REFIID, void** out) {
  if (out != nullptr) *out = nullptr;
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::CreateStatusArray : non implémenté (E_NOTIMPL)");
  return kNotImplemented;
}

HRESULT WINAPI FactorySetCpuAffinity(Factory*, std::uint64_t mask) {
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::SetCpuAffinity mask=%llx : non implémenté (E_NOTIMPL)",
                static_cast<unsigned long long>(mask));
  return kNotImplemented;
}

void WINAPI FactorySetDebugFlags(Factory*, std::uint32_t flags) {
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::SetDebugFlags flags=%x : journalisé, sans effet", flags);
}

HRESULT WINAPI FactorySetStagingBufferSize(Factory*, std::uint32_t size) {
  // Pas de tampon de transit : aucune lecture de fichier n'existe encore. La taille est journalisée.
  anyxboxx::Log("[XCRDAPI] IDStorageFactoryX::SetStagingBufferSize size=%x : ignoré (pas de tampon de transit)", size);
  return S_OK;
}

constexpr FactoryVtable kFactoryVtable = {
    FactoryQueryInterface,     FactoryAddRef,        FactoryRelease,         FactoryCreateQueue,
    FactoryOpenFile,           FactoryCreateStatusArray, FactorySetCpuAffinity, FactorySetDebugFlags,
    FactorySetStagingBufferSize,
};

Factory g_factory{&kFactoryVtable};

}  // namespace

extern "C" {

HRESULT WINAPI DStorageGetFactory(REFIID riid, void** out) {
  if (out == nullptr) return kPointer;
  *out = nullptr;
  return FactoryQueryInterface(&g_factory, riid, out);
}

}  // extern "C"
