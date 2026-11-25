#pragma once
#include <string>
#include <windows.h>
#include <d3d12.h>

namespace tuxbox_test {
class DisplayCheck {
public:
    DisplayCheck(UINT width, UINT height, const wchar_t* window_mode = nullptr);
    ~DisplayCheck();
    const std::string& sizing() const { return sizing_; }
    DisplayCheck(const DisplayCheck&) = delete;
    DisplayCheck& operator=(const DisplayCheck&) = delete;
    void verify(ID3D12CommandQueue* queue, ID3D12Resource* source, UINT64 token,
                COLORREF corner, COLORREF center);
private:
    HWND window_ = nullptr;
    bool owns_title_ = false;
    bool check_size_ = false;
    std::string sizing_;
};
}
