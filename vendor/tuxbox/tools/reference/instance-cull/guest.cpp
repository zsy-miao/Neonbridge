#include "../visibility_fixture.h"

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "instance-cull", false,
                                  [](const wchar_t* adapter, bool) { return visibility_fixture::run(adapter, false); });
}
