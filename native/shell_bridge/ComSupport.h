#pragma once
#include <windows.h>
#include <oleauto.h>
#include <vector>
#include <string>

#define RETURN_HR_IF_NULL(hr, ptr) do { if (!(ptr)) return (hr); } while (0)
#define RETURN_IF_FAILED(call) do { HRESULT result_ = (call); if (FAILED(result_)) return result_; } while (0)

struct LocalVariant : VARIANT {
    LocalVariant() { VariantInit(this); }
    ~LocalVariant() { VariantClear(this); }
    LocalVariant(const LocalVariant&) = delete;
    LocalVariant& operator=(const LocalVariant&) = delete;
};
template<class T> class ComPtr {
    T* value_ = nullptr;
public:
    ~ComPtr() { if (value_) value_->Release(); }
    T** put() { return &value_; }
    T* get() const { return value_; }
    T* operator->() const { return value_; }
};
