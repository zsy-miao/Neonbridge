// Table des interfaces servies par AnyXboxX (served_interfaces.inc) et IUnknown commun.
#include <windows.h>

#include <cstdio>

#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

constexpr GUID kIUnknown = {0x00000000, 0x0000, 0x0000, {0xC0, 0, 0, 0, 0, 0, 0, 0x46}};

// « 073B7DCB-1FCF-4030-94BE-E3C9EB623428 » → GUID (sans dépendance à COM).
bool ParseGuid(const char* text, GUID* out) {
  unsigned long data1 = 0;
  unsigned int data2 = 0, data3 = 0, bytes[8] = {};
  const int fields = sscanf_s(text, "%8lx-%4x-%4x-%2x%2x-%2x%2x%2x%2x%2x%2x", &data1, &data2, &data3,
                              &bytes[0], &bytes[1], &bytes[2], &bytes[3], &bytes[4], &bytes[5],
                              &bytes[6], &bytes[7]);
  if (fields != 11) return false;
  out->Data1 = data1;
  out->Data2 = static_cast<unsigned short>(data2);
  out->Data3 = static_cast<unsigned short>(data3);
  for (int index = 0; index < 8; ++index) out->Data4[index] = static_cast<unsigned char>(bytes[index]);
  return true;
}

struct ServedEntry {
  GUID provider;
  GUID iid;
  void* (*object)();
  bool valid;
};

ServedEntry MakeEntry(const char* provider, const char* iid, void* (*object)()) {
  ServedEntry entry{};
  entry.object = object;
  entry.valid = ParseGuid(provider, &entry.provider) && ParseGuid(iid, &entry.iid);
  return entry;
}

// Lue une seule fois (initialisation statique sûre entre threads) : QueryApiImpl
// est appelée à chaque fonction X*, des milliers de fois par seconde.
const ServedEntry* Entries(size_t* count) {
  static const ServedEntry kServed[] = {
#define ANYXBOXX_SERVED_INTERFACE(object, provider_text, iid_text) \
  MakeEntry(provider_text, iid_text, &anyxboxx::object),
#include "served_interfaces.inc"
#undef ANYXBOXX_SERVED_INTERFACE
  };
  *count = sizeof kServed / sizeof kServed[0];
  return kServed;
}

}  // namespace

namespace anyxboxx {

void* ServedInterface(const GUID& provider, const GUID& iid) {
  size_t count = 0;
  const ServedEntry* entries = Entries(&count);
  for (size_t index = 0; index < count; ++index) {
    const ServedEntry& entry = entries[index];
    if (entry.valid && IsEqualGUID(provider, entry.provider) && IsEqualGUID(iid, entry.iid)) {
      return entry.object();
    }
  }
  return nullptr;
}

HRESULT WINAPI StaticQueryInterface(void* self, REFIID iid, void** out) noexcept {
  if (out == nullptr) return E_POINTER;
  *out = nullptr;
  if (IsEqualGUID(iid, kIUnknown)) {
    *out = self;
    return S_OK;
  }
  size_t count = 0;
  const ServedEntry* entries = Entries(&count);
  for (size_t index = 0; index < count; ++index) {
    if (entries[index].valid && IsEqualGUID(iid, entries[index].iid) && entries[index].object() == self) {
      *out = self;
      return S_OK;
    }
  }
  return E_NOINTERFACE;
}

ULONG WINAPI StaticAddRef(void*) noexcept { return 1; }
ULONG WINAPI StaticRelease(void*) noexcept { return 1; }

HRESULT WINAPI NotImplementedMethod(void*) noexcept {
  ANYXBOXX_TRACE("[runtime] méthode d'interface non écrite appelée -> E_NOTIMPL");
  return E_NOTIMPL;
}

}  // namespace anyxboxx
