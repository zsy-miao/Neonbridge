// Independently compare the guest's feature records with the selected desktop adapter.
#include "frame_abi.h"
#include "frame_cli.h"
#include <algorithm>

namespace {
using namespace frame_abi;
constexpr UINT guard = 0xc5a7b91d;

void require(bool ok, const char* detail) {
    if (!ok) throw std::runtime_error(detail);
}

ComPtr<ID3D12Device> desktop_control(LUID luid) {
    ComPtr<IDXGIFactory4> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "control factory");
    ComPtr<IDXGIAdapter1> adapter;
    check(factory->EnumAdapterByLuid(luid, IID_PPV_ARGS(&adapter)), "same adapter");
    HMODULE desktop = GetModuleHandleW(L"d3d12.dll");
    if (!desktop) throw std::runtime_error("desktop D3D12 module");
    auto create = reinterpret_cast<PFN_D3D12_CREATE_DEVICE>(GetProcAddress(desktop, "D3D12CreateDevice"));
    if (!create) throw std::runtime_error("desktop factory export");
    ComPtr<ID3D12Device> device;
    check(create(adapter.Get(), D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&device)), "control device");
    return device;
}

template<class T>
void host_query(ID3D12Device* host, D3D12_FEATURE feature, T& record) {
    check(host->CheckFeatureSupport(feature, &record, sizeof(record)), "desktop feature");
}

void allocation_info(ID3D12Device* guest, ID3D12Device* host) {
    // Frosty's slot 25 passes the hidden result pointer ahead of the device.
    using Allocation = D3D12_RESOURCE_ALLOCATION_INFO* (WINAPI*)(
        D3D12_RESOURCE_ALLOCATION_INFO*, ID3D12Device*, UINT, UINT, const D3D12_RESOURCE_DESC*);
    static_assert(sizeof(D3D12_RESOURCE_DESC) == 0x38 && sizeof(D3D12_RESOURCE_ALLOCATION_INFO) == 16);
    auto query_allocation = method<Allocation>(guest, 25);
    auto compare = [&](UINT mask, UINT count, const D3D12_RESOURCE_DESC* descs, bool valid) {
        constexpr UINT64 edge = 0x3123456789abcdefull;
        struct {
            UINT64 before;
            D3D12_RESOURCE_ALLOCATION_INFO info;
            UINT64 after;
        } record{edge, {edge, edge}, ~edge};
        const auto reference = host->GetResourceAllocationInfo(mask, count, descs);
        auto* result = query_allocation(&record.info, guest, mask, count, descs);
        require(result == &record.info && record.before == edge && record.after == ~edge,
                "allocation result pointer or surrounding memory");
        require(record.info.SizeInBytes == reference.SizeInBytes, "allocation byte size");
        if (valid) {
            require(reference.SizeInBytes != UINT64_MAX && reference.SizeInBytes && reference.Alignment &&
                    record.info.Alignment == reference.Alignment, "allocation alignment");
        } else {
            require(record.info.SizeInBytes == UINT64_MAX, "invalid allocation accepted");
        }
    };
    std::array<D3D12_RESOURCE_DESC, 2> resources{};
    auto& buffer = resources[0];
    buffer.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    buffer.Width = 100003;
    buffer.Height = 1;
    buffer.DepthOrArraySize = 1;
    buffer.MipLevels = 1;
    buffer.SampleDesc.Count = 1;
    buffer.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    auto& texture = resources[1];
    texture.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    texture.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    texture.Width = 257;
    texture.Height = 193;
    texture.DepthOrArraySize = 1;
    texture.MipLevels = 1;
    texture.SampleDesc.Count = 1;
    compare(0, 1, &buffer, true);
    compare(1, 1, &texture, true);
    compare(0, UINT(resources.size()), resources.data(), true);
    texture.Dimension = D3D12_RESOURCE_DIMENSION_UNKNOWN;
    compare(0, 1, &texture, false);
}

template<class T, size_t N>
struct Guarded {
    std::array<BYTE, 8> before;
    std::array<T, N> values{};
    std::array<BYTE, 8> after;
    Guarded() { before.fill(0xbd); after.fill(0x73); }
    void check_edges() const {
        require(std::all_of(before.begin(), before.end(), [](BYTE b){ return b == 0xbd; }) &&
                std::all_of(after.begin(), after.end(), [](BYTE b){ return b == 0x73; }), "footprint output overrun");
    }
};

void copy_footprints(ID3D12Device* guest, ID3D12Device* host) {
    constexpr UINT first = 1, count = 3;
    constexpr UINT64 base = 1024;
    static_assert(sizeof(D3D12_PLACED_SUBRESOURCE_FOOTPRINT) == 32);
    D3D12_RESOURCE_DESC texture{};
    texture.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    texture.Width = 131;
    texture.Height = 75;
    texture.DepthOrArraySize = 1;
    texture.SampleDesc.Count = 1;
    texture.MipLevels = 4;
    texture.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    std::array<D3D12_PLACED_SUBRESOURCE_FOOTPRINT, count> layout{};
    std::array<UINT, count> rows{};
    std::array<UINT64, count> bytes{};
    UINT64 total{};
    host->GetCopyableFootprints(&texture, first, count, base, layout.data(), rows.data(), bytes.data(), &total);
    require(total && total != UINT64_MAX && layout[0].Offset == base, "desktop footprint baseline");
    for (UINT mip = 0; mip < count; ++mip) {
        require(bytes[mip] == 4 * (texture.Width >> (first + mip)) && bytes[mip] < layout[mip].Footprint.RowPitch &&
                rows[mip] == texture.Height >> (first + mip), "mip row size or padded pitch");
    }
    enum Outputs { all, layouts, row_info, total_only };
    for (const auto outputs : {all, layouts, row_info, total_only}) {
        Guarded<D3D12_PLACED_SUBRESOURCE_FOOTPRINT, count> placed;
        Guarded<UINT, count> row_counts;
        Guarded<UINT64, count> row_bytes;
        Guarded<UINT64, 1> size;
        auto* placed_out = (outputs == all || outputs == layouts) ? placed.values.data() : nullptr;
        auto* rows_out = (outputs == all || outputs == row_info) ? row_counts.values.data() : nullptr;
        auto* bytes_out = rows_out ? row_bytes.values.data() : nullptr;
        auto* total_out = (outputs == all || outputs == total_only) ? size.values.data() : nullptr;
        guest->GetCopyableFootprints(&texture, first, count, base, placed_out, rows_out, bytes_out, total_out);
        placed.check_edges();
        row_counts.check_edges();
        row_bytes.check_edges();
        size.check_edges();
        if (rows_out) require(row_counts.values == rows && row_bytes.values == bytes, "footprint row arrays");
        if (total_out) require(size.values[0] == total, "total footprint size");
        if (placed_out) for (UINT i = 0; i < count; ++i) {
            const auto& entry = placed.values[i];
            const auto& a = entry.Footprint;
            const auto& b = layout[i].Footprint;
            require(entry.Offset == layout[i].Offset && a.Width == b.Width && a.Height == b.Height &&
                    a.Depth == b.Depth && a.Format == b.Format && a.RowPitch == b.RowPitch, "placed mip layout");
        }
    }
}

// Include two extra DWORDs in each claim, exactly as a later SDK might. The
// driver accepts larger records and only writes the fields it understands.
template<size_t N>
std::array<UINT, N + 2> query(ID3D12Device* guest, D3D12_FEATURE feature,
                              std::initializer_list<UINT> input = {}) {
    std::array<UINT, N + 2> data;
    data.fill(guard);
    size_t offset = 0;
    for (UINT value : input) data.at(offset++) = value;
    const auto original = data;
    HRESULT short_hr = guest->CheckFeatureSupport(feature, data.data(), N * sizeof(UINT) - 1);
    require(short_hr == E_INVALIDARG && data == original, "short record overwritten or accepted");
    require(guest->CheckFeatureSupport(feature, nullptr, N * sizeof(UINT)) == E_INVALIDARG,
            "empty record accepted");
    check(guest->CheckFeatureSupport(feature, data.data(), sizeof(data)), "guest feature");
    require(data[N] == guard && data[N + 1] == guard, "extension overwritten");
    return data;
}

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    auto guest = create_device(requested, selected);
    auto host = desktop_control(selected.AdapterLuid);
    allocation_info(guest.Get(), host.Get());
    copy_footprints(guest.Get(), host.Get());

    D3D12_FEATURE_DATA_D3D12_OPTIONS native{};
    host_query(host.Get(), D3D12_FEATURE_D3D12_OPTIONS, native);
    auto options = query<16>(guest.Get(), D3D12_FEATURE_D3D12_OPTIONS);
    require(options[0] == UINT(native.DoublePrecisionFloatShaderOps != 0) &&
            options[4] == UINT(native.ResourceBindingTier) && options[15] == UINT(native.ResourceHeapTier) &&
            options[3] == UINT(native.TiledResourcesTier) && options[7] == 0 && options[14] == 0,
            "64-byte Xbox OPTIONS layout");
    D3D12_FEATURE_DATA_GPU_VIRTUAL_ADDRESS_SUPPORT addresses{};
    host_query(host.Get(), D3D12_FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT, addresses);
    auto bits = query<2>(guest.Get(), D3D12_FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT);
    require(bits[0] == (std::min)(addresses.MaxGPUVirtualAddressBitsPerResource, 40u) &&
            bits[1] == (std::min)(addresses.MaxGPUVirtualAddressBitsPerProcess, 40u) &&
            options[9] == (std::min)(native.MaxGPUVirtualAddressBitsPerResource, 40u), "GPU address widths");

    auto options5 = query<3>(guest.Get(), D3D12_FEATURE_D3D12_OPTIONS5);
    require(options5[0] == 0 && options5[1] == 0 && options5[2] == 0, "unavailable raytracing or render pass");
    for (UINT version : {1u, 2u}) {
        auto root = query<1>(guest.Get(), D3D12_FEATURE_ROOT_SIGNATURE, {version});
        require(root[0] == D3D_ROOT_SIGNATURE_VERSION_1_0, "packaged root signature limit");
    }

    constexpr auto format = DXGI_FORMAT_R8G8B8A8_UNORM;
    D3D12_FEATURE_DATA_FORMAT_SUPPORT flags{format, {}, {}};
    host_query(host.Get(), D3D12_FEATURE_FORMAT_SUPPORT, flags);
    auto support = query<3>(guest.Get(), D3D12_FEATURE_FORMAT_SUPPORT, {format});
    constexpr UINT unavailable = UINT(D3D12_FORMAT_SUPPORT2_SAMPLER_FEEDBACK);
    require(support[0] == format && support[1] == UINT(flags.Support1) &&
            support[2] == (UINT(flags.Support2) & ~unavailable),
            "same-adapter format flags");
    auto extended = query<3>(guest.Get(), D3D12_FEATURE_FORMAT_SUPPORT, {116});
    require(extended[0] == 116 && extended[1] == 0 && extended[2] == 0, "unmapped Xbox format");
    D3D12_FEATURE_DATA_FORMAT_INFO info{format, 0};
    host_query(host.Get(), D3D12_FEATURE_FORMAT_INFO, info);
    auto planes = query<2>(guest.Get(), D3D12_FEATURE_FORMAT_INFO, {format});
    require(planes[0] == format && planes[1] == ((guard & 0xffffff00) | info.PlaneCount), "plane count or padding");
    for (UINT count : {1u, 4u}) {
        D3D12_FEATURE_DATA_MULTISAMPLE_QUALITY_LEVELS msaa{format, count, {}, 0};
        host_query(host.Get(), D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS, msaa);
        auto quality = query<4>(guest.Get(), D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS, {format, count, 0u});
        require(quality[0] == format && quality[1] == count && quality[2] == 0 &&
                quality[3] == UINT(msaa.NumQualityLevels != 0), "multisample count");
    }
    std::array<UINT, 3> invalid = {UINT_MAX, guard, guard};
    const auto before = invalid;
    require(guest->CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT, invalid.data(), sizeof(invalid)) ==
            E_INVALIDARG && invalid == before, "invalid format modified caller's data");
    require(guest->CheckFeatureSupport(static_cast<D3D12_FEATURE>(0xff), invalid.data(), sizeof(invalid)) ==
            E_NOTIMPL && invalid == before, "unknown feature modified caller's data");
    char receipt[128];
    std::snprintf(receipt, sizeof(receipt),
                  "d3d12-features PASS pci=%04x:%04x queries=7 binding=%u heap=%u root=1 dxr=0 allocations=4 footprints=4",
                  selected.VendorId, selected.DeviceId, options[4], options[15]);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-features", false, run);
}
