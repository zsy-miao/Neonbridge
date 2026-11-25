// Independent console-ABI guest. The optional retail test takes a source,
// 64-bit offset, compressed size and independently inflated reference file.
#define WIN32_LEAN_AND_MEAN
#include <algorithm>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <stdexcept>
#include <unknwn.h>
#include <vector>
#include <windows.h>
#include <wrl/client.h>

using Microsoft::WRL::ComPtr;
struct QueueDesc {
  UINT64 source;
  UINT16 capacity;
  INT8 priority;
  const char *name;
};
struct Request {
  UINT64 options;
  void *dest;
  UINT64 pageOffset;
  UINT32 capacity;
  IUnknown *file;
  UINT64 offset;
  UINT32 size;
  UINT32 intermediate;
  UINT64 tag;
  const char *name;
};
struct QueueInfo {
  QueueDesc desc;
  UINT16 empty, untilSubmit;
};
struct __declspec(uuid("43704f9f-27c6-4a9f-8815-d16527894df4")) XFile
    : IUnknown {
  virtual void STDMETHODCALLTYPE Close() = 0;
  virtual HANDLE STDMETHODCALLTYPE GetHandle() = 0;
};
struct __declspec(uuid("cd66d752-2786-44f7-9070-e2a6fdad3f1b")) Status
    : IUnknown {
  virtual bool STDMETHODCALLTYPE IsComplete(UINT32) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetHResult(UINT32) = 0;
};
struct __declspec(uuid("a3f6649a-8c2c-4b3a-8b6e-10186a04b9bc")) Queue
    : IUnknown {
  virtual void STDMETHODCALLTYPE EnqueueRequest(const Request *) = 0;
  virtual void STDMETHODCALLTYPE EnqueueStatus(Status *, UINT32) = 0;
  virtual void STDMETHODCALLTYPE EnqueueSignal(IUnknown *, UINT64) = 0;
  virtual void STDMETHODCALLTYPE Submit() = 0;
  virtual void STDMETHODCALLTYPE CancelRequestsWithTag(UINT64, UINT64) = 0;
  virtual void STDMETHODCALLTYPE Close() = 0;
  virtual HANDLE STDMETHODCALLTYPE GetErrorEvent() = 0;
  virtual void STDMETHODCALLTYPE RetrieveErrorRecord(void *) = 0;
  virtual void STDMETHODCALLTYPE Query(QueueInfo *) = 0;
};
struct __declspec(uuid("fb3b54ac-bf82-463a-b777-73195837864d")) Factory
    : IUnknown {
  virtual HRESULT STDMETHODCALLTYPE CreateQueue(const QueueDesc *, REFIID,
                                                void **) = 0;
  virtual HRESULT STDMETHODCALLTYPE OpenFile(const wchar_t *, REFIID,
                                             void **) = 0;
  virtual HRESULT STDMETHODCALLTYPE CreateStatusArray(UINT32, const char *,
                                                      REFIID, void **) = 0;
};
extern "C" __declspec(dllimport) HRESULT WINAPI DStorageGetFactory(REFIID,
                                                                   void **);
static_assert(offsetof(Request, capacity) == 24 &&
              offsetof(Request, offset) == 40 && offsetof(Request, tag) == 56);

static void require(bool ok, const char *message) {
  if (!ok)
    throw std::runtime_error(message);
}
static void check(HRESULT hr, const char *message) {
  if (FAILED(hr)) {
    std::fprintf(stderr, "%s: 0x%08x\n", message, unsigned(hr));
    throw std::runtime_error(message);
  }
}
static HRESULT wait_status(Status *s, UINT i) {
  ULONGLONG deadline = GetTickCount64() + 10000;
  while (!s->IsComplete(i) && GetTickCount64() < deadline)
    Sleep(1);
  return s->GetHResult(i);
}

static void run(int argc, wchar_t **argv) {
  require(argc == 1 || argc == 2 || argc == 5 || argc == 6,
          "expected [receipt] or source offset size oracle [receipt]");
  ComPtr<Factory> f, other;
  check(DStorageGetFactory(IID_PPV_ARGS(&f)), "Xbox factory");
  check(DStorageGetFactory(IID_PPV_ARGS(&other)), "repeat factory");
  require(f.Get() == other.Get(), "factory identity");
  void *none = f.Get();
  require(DStorageGetFactory(__uuidof(Queue), &none) == E_NOINTERFACE && !none,
          "foreign factory IID");
  ComPtr<Queue> q;
  QueueDesc d{0, 0x80, 0, "Storage fixture"};
  check(f->CreateQueue(&d, IID_PPV_ARGS(&q)), "queue");
  QueueInfo info{};
  q->Query(&info);
  require(info.empty == 127 && info.untilSubmit == 64 &&
              !std::strcmp(info.desc.name, d.name),
          "queue capacity");
  ComPtr<Status> status;
  check(f->CreateStatusArray(3, "Completion", IID_PPV_ARGS(&status)), "status");
  const unsigned char compressed[] = {
      0x78, 0x9c, 0x63, 0x60, 0x64, 0x62, 0x66, 0x61, 0x65, 0x63, 0xe7, 0xe0,
      0xe4, 0xe2, 0xe6, 0xe1, 0xe5, 0xe3, 0x07, 0x00, 0x02, 0xb8, 0x00, 0x79};
  std::vector<unsigned char> expected(16);
  for (unsigned i = 0; i < expected.size(); ++i)
    expected[i] = static_cast<unsigned char>(i);
  unsigned size = sizeof(compressed);
  const wchar_t *file = L"xcrdapi-fixture.bin";
  UINT64 offset = 16;
  if (argc >= 5) {
    file = argv[1];
    offset = std::wcstoull(argv[2], nullptr, 0);
    size = std::wcstoul(argv[3], nullptr, 0);
    std::ifstream oracle(argv[4], std::ios::binary);
    require(oracle.good(), "open oracle");
    expected.assign(std::istreambuf_iterator<char>(oracle), {});
    require(expected.size() > 16, "nonempty oracle");
  } else {
    std::ofstream out(file, std::ios::binary | std::ios::trunc);
    out.write(reinterpret_cast<const char *>(expected.data()), expected.size());
    out.write(reinterpret_cast<const char *>(compressed), sizeof(compressed));
    require(out.good(), "write fixture");
  }
  ComPtr<XFile> source;
  check(f->OpenFile(file, IID_PPV_ARGS(&source)), "file");
  BY_HANDLE_FILE_INFORMATION fi{};
  HANDLE handle = source->GetHandle();
  require(handle && handle != INVALID_HANDLE_VALUE &&
              GetFileInformationByHandle(handle, &fi),
          "file information");
  CloseHandle(handle);
  require(
      (((UINT64(fi.nFileSizeHigh) << 32) | fi.nFileSizeLow) >= offset + size),
      "file size");
  std::vector<unsigned char> out(expected.size() + 2, 0xa5);
  Request r{1,
            out.data() + 1,
            0,
            static_cast<UINT32>(expected.size()),
            source.Get(),
            offset,
            size,
            0,
            17,
            nullptr};
  q->EnqueueRequest(&r);
  q->EnqueueStatus(status.Get(), 0);
  require(!status->IsComplete(0) && status->GetHResult(0) == E_PENDING,
          "status pending before submit");
  q->Submit();
  check(wait_status(status.Get(), 0), "zlib read");
  require(std::equal(expected.begin(), expected.end(), out.begin() + 1) &&
              out.front() == 0xa5 && out.back() == 0xa5,
          "zlib bytes or guards");
  if (argc >= 5) {
    std::printf("retail=%zu offset=%llu ", expected.size(), offset);
    return;
  }
  std::fill(out.begin(), out.end(), static_cast<unsigned char>(0xa5));
  r.options = 0;
  r.offset = 0;
  r.size = 16;
  q->EnqueueRequest(&r);
  q->EnqueueStatus(status.Get(), 1);
  q->Submit();
  check(wait_status(status.Get(), 1), "raw read");
  require(std::equal(expected.begin(), expected.end(), out.begin() + 1),
          "raw bytes");
  std::fill(out.begin(), out.end(), static_cast<unsigned char>(0xa5));
  q->EnqueueRequest(&r);
  q->CancelRequestsWithTag(~UINT64(0), 17);
  q->EnqueueStatus(status.Get(), 0);
  q->Submit();
  check(wait_status(status.Get(), 0), "cancelled read");
  require(
      std::all_of(out.begin(), out.end(), [](unsigned v) { return v == 0xa5; }),
      "cancelled write");
  HANDLE event = q->GetErrorEvent();
  require(event != nullptr, "error event");
  r.offset = 65536;
  q->EnqueueRequest(&r);
  q->EnqueueStatus(status.Get(), 0);
  r.options = 1;
  r.offset = 0;
  r.size = 16;
  q->EnqueueRequest(&r);
  q->EnqueueStatus(status.Get(), 1);
  r.options = 0;
  q->EnqueueRequest(&r);
  q->EnqueueStatus(status.Get(), 2);
  source.Reset();
  q->Submit();
  check(wait_status(status.Get(), 2), "next batch recovers after failure");
  require(FAILED(status->GetHResult(0)) && FAILED(status->GetHResult(1)),
          "EOF and corrupt zlib");
  require(WaitForSingleObject(event, 1000) == WAIT_OBJECT_0,
          "signalled error event");
  alignas(8) std::array<BYTE, 1024> record{};
  q->RetrieveErrorRecord(record.data());
  require(*reinterpret_cast<const UINT32 *>(record.data()) == 2,
          "two recorded errors");
  require(WaitForSingleObject(event, 0) == WAIT_TIMEOUT, "reset error event");
  q->RetrieveErrorRecord(record.data());
  UINT32 count, kind;
  std::memcpy(&count, record.data(), sizeof(count));
  std::memcpy(&kind, record.data() + 12, sizeof(kind));
  require(count == 0 && kind == UINT32_MAX, "cleared first failure");
  CloseHandle(event);
  check(f->OpenFile(file, IID_PPV_ARGS(&source)), "reopen file");
  std::fill(out.begin(), out.end(), static_cast<unsigned char>(0xa5));
  r.file = source.Get();
  q->EnqueueRequest(&r);
  q->Submit();
  source->Close();
  require(std::equal(expected.begin(), expected.end(), out.begin() + 1) &&
              !source->GetHandle(),
          "explicit close waits for outstanding read");

  ComPtr<Queue> memory;
  QueueDesc mem_desc{1, 0x80, 2, "Memory queue"};
  check(f->CreateQueue(&mem_desc, IID_PPV_ARGS(&memory)), "memory queue");
  std::fill(out.begin(), out.end(), static_cast<unsigned char>(0xa5));
  r.options = (UINT64(1) << 22) | 1;
  r.file =
      reinterpret_cast<IUnknown *>(const_cast<unsigned char *>(compressed));
  r.size = sizeof(compressed);
  memory->EnqueueRequest(&r);
  memory->EnqueueStatus(status.Get(), 2);
  memory->Submit();
  check(wait_status(status.Get(), 2), "memory zlib");
  require(std::equal(expected.begin(), expected.end(), out.begin() + 1) &&
              out.front() == 0xa5 && out.back() == 0xa5,
          "memory bytes and guards");
  r.capacity = 4;
  std::fill(out.begin(), out.end(), static_cast<unsigned char>(0xa5));
  memory->EnqueueRequest(&r);
  memory->EnqueueStatus(status.Get(), 2);
  memory->Close();
  require(FAILED(status->GetHResult(2)) &&
              std::all_of(out.begin(), out.end(),
                          [](unsigned v) { return v == 0xa5; }),
          "bounded inflate");

  // Fill exactly half the ring without calling Submit.
  for (unsigned i = 0; i < 63; ++i)
    q->EnqueueStatus(status.Get(), 0);
  q->EnqueueStatus(status.Get(), 1);
  check(wait_status(status.Get(), 1), "automatic submit");
  q->Close();
  require(DeleteFileW(file) != FALSE, "release input file");
}

int wmain(int argc, wchar_t **argv) {
  try {
    run(argc, argv);
    constexpr auto result =
        "raw=16 zlib=16 guards=2 errors=2 cancellation=1 autosubmit=64 PASS";
    if (argc == 2 || argc == 6) {
      std::ofstream report(argv[argc - 1], std::ios::binary);
      if (argc == 6)
        report << "retail PASS";
      else
        report << result;
      require(report.good(), "write receipt");
    }
    std::puts(argc >= 5 ? "PASS" : result);
    return 0;
  } catch (const std::exception &e) {
    std::fprintf(stderr, "xcrdapi-read FAIL %s\n", e.what());
    return 1;
  }
}
