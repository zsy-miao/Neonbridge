//! Data exports from the console D3D12 and DXGI interfaces.
use windows::Win32::Graphics::{Direct3D, Direct3D12, Dxgi};
use windows::core::{GUID, Interface};

macro_rules! interface_ids {
    ($module:ident, $($interface:ident),+ $(,)?) => {
        $(const _: () = {
            #[unsafe(export_name = concat!("IID_", stringify!($interface)))]
            static IID: GUID = $module::$interface::IID;
        };)+
    };
}

interface_ids!(
    Direct3D12,
    ID3D12CommandAllocator,
    ID3D12CommandList,
    ID3D12CommandQueue,
    ID3D12Debug,
    ID3D12DescriptorHeap,
    ID3D12Device,
    ID3D12DeviceChild,
    ID3D12Fence,
    ID3D12GraphicsCommandList,
    ID3D12Heap,
    ID3D12Object,
    ID3D12Pageable,
    ID3D12PipelineState,
    ID3D12Resource,
    ID3D12RootSignature,
    ID3D12RootSignatureDeserializer,
);

interface_ids!(
    Dxgi,
    IDXGIAdapter,
    IDXGIAdapter1,
    IDXGIAdapter2,
    IDXGIDevice,
    IDXGIDevice1,
    IDXGIDevice2,
    IDXGIDeviceSubObject,
    IDXGIFactory,
    IDXGIFactory1,
    IDXGIFactory2,
    IDXGIObject,
    IDXGISwapChain,
    IDXGISwapChain1,
);

#[unsafe(export_name = "WKPDID_D3DDebugObjectNameW")]
static DEBUG_OBJECT_NAME: GUID = Direct3D::WKPDID_D3DDebugObjectNameW;

#[unsafe(export_name = "IID_ID3D12XboxDmaCommandList")]
static XBOX_DMA_COMMAND_LIST: GUID = GUID::from_u128(0x30bc77b9_8379_4b25_95de_b8588c42275d);

#[unsafe(export_name = "IID_IGraphicsUnknown")]
static GRAPHICS_UNKNOWN: GUID = GUID::from_u128(0xaceeea63_e0a9_4a1c_bbec_71b2f485f758);
