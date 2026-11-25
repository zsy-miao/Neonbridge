// Compare every byte of the returned layout, image views, tilings and sampled
// addresses against the packaged DLL, including the room's exact color target.
static bool check_color_msaa() {
    const ResourceDesc descriptors[] = {
        {3, 0, 2976, 1676, 1, 1, 42, 4, 0, 0, 0x300001}, {3, 0, 17, 13, 3, 1, 28, 4, 0, 0, 0x100001},
        {3, 0, 129, 65, 8, 1, 34, 4, 0, 0, 0x200001},    {3, 0, 1919, 1079, 3, 1, 87, 4, 0, 0, 0x300005},
        {3, 0, 63, 127, 2, 1, 24, 4, 0, 0, 0x100005},    {3, 0, 1057, 701, 4, 1, 41, 4, 0, 0, 0x200005},
    };
    for (UINT version : {3u, 4u}) {
        XGSetHardwareVersion(version);
        for (auto desc : descriptors)
            for (UINT samples : {2u, 4u, 8u}) {
                desc.samples = samples;
                Computer* obj = nullptr;
                if (XGCreateTextureComputer(&desc, &obj) != S_OK || !obj)
                    return false;
                std::array<BYTE, 0x1b40> layout{}, roundtrip{};
                std::array<UINT, 8> view{};
                Computer* packed = nullptr;
                if (obj->methods->layout(obj, layout.data()) != S_OK || !record(layout.data(), layout.size()) ||
                    obj->methods->image(obj, 0, view.data()) != S_OK || !record(view.data(), sizeof(view)) ||
                    XGCreateTextureComputerFromDescriptor(view.data(), &packed) != S_OK || !packed ||
                    packed->methods->layout(packed, roundtrip.data()) != S_OK || layout != roundtrip ||
                    !check_tiling(obj) || !check_coordinates(obj) || packed->methods->release(packed))
                    return false;
                for (UINT s = 0; s <= samples; ++s)
                    for (UINT i = 0; i < 256; ++i) {
                        UINT bit = 0xdeadbeef;
                        const auto off = obj->methods->address(obj, 0, 0, (i * 107u + 11u) % desc.width,
                                                               (i * 71u + 3u) % desc.height, i % desc.slices, s, &bit);
                        if (!record(&off, sizeof(off)) || !record(&bit, sizeof(bit)))
                            return false;
                    }
                if (desc.width == 17) {
                    if (!check_copy(obj, layout.data(), 0, 1, 1, 1, 17, desc.height, 4))
                        return false;
                    const UINT row = 17 * 4 + 16;
                    std::vector<BYTE> tiled(obj->methods->bytes(obj));
                    for (size_t i = 0; i < tiled.size(); ++i)
                        tiled[i] = static_cast<BYTE>((i * 71u + i / 251u) % 251u);
                    for (UINT s = 0; s < samples; ++s) {
                        std::vector<BYTE> linear(row * desc.height, 0xa5), expected = linear;
                        for (UINT y = 0; y < desc.height; ++y)
                            for (UINT x = 0; x < 17; ++x) {
                                auto offset = obj->methods->address(obj, 0, 0, x, y, 1, s, nullptr);
                                if (offset + 4 > tiled.size())
                                    return false;
                                std::memcpy(expected.data() + y * row + x * 4, tiled.data() + offset, 4);
                            }
                        if (obj->methods->from(obj, linear.data(), 0, 1, s, tiled.data(), row, 0) != S_OK ||
                            linear != expected || !record(linear.data(), linear.size()))
                            return false;
                    }
                }
                if (obj->methods->release(obj))
                    return false;
            }
    }
    XGSetHardwareVersion(3);
    return true;
}
