// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include <ddk/debug.h>
#include <ddk/mmio-buffer.h>
#include <fbl/macros.h>
#include <hw/arch_ops.h>
#include <lib/zx/bti.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>
#include <optional>
#include <string.h>
#include <utility>
#include <zircon/assert.h>
#include <zircon/process.h>

namespace ddk {

// Usage Notes:
//
// ddk::MmioBuffer is a c++ wrapper around the mmio_buffer_t object. It provides
// capabilities to map an MMIO region provided by a VMO, and accessors to read and
// write the MMIO region. Destroying it will result in the MMIO region being
// unmapped. All read/write operations are bounds checked.
//
// ddk::MmioView provides a slice view of an mmio region. It provides the same
// accessors provided by MmioBuffer, but does not manage the buffer's mapping.
// It must outlive the ddk::MmioBuffer it is spawned from.
//
// ddk::MmioPinnedBuffer is a c++ wrapper around the mmio_pinned_buffer_t object.
// It is generated by calling |Pin()| on a MmioBuffer or MmioView and provides
// access to the physical address space for the region. Performing pinning on MmioView
// will only pin the pages associated with the MmioView, and not the entire
// MmioBuffer. Destorying MmioPInnedBuffer will unpin the memory.
//
// Consider using this in conjuntion with hwreg::RegisterBase for increased safety
// and improved ergonomics.
//
////////////////////////////////////////////////////////////////////////////////
// Example: An mmio region provided by the Platform Device protocol.
//
// pdev_mmio_t pdev_mmio;
// GetMmio(index, &pdev_mmio);
//
// std::optional<ddk::MmioBuffer> mmio;
// zx_status_t status;
//
// status = ddk::MmioBuffer::Create(pdev_mmio.offset, pdev_mmio.size,
//                                  zx::vmo(pdev_mmio.vmo),
//                                  ZX_CACHE_POLICY_UNCACHED_DEVICE, mmio);
// if (status != ZX_OK) return status;
//
// auto value = mmio->Read<uint32_t>(kRegOffset);
// value |= kRegFlag;
// mmio->Write(value, kRegOffset);
//
////////////////////////////////////////////////////////////////////////////////
// Example: An mmio region created from a physical region.
//
// std::optional<ddk::MmioBuffer> mmio;
// zx_status_t status;
//
// zx::unowned_resource resource(get_root_resource());
// status = ddk::MmioBuffer::Create(T931_USBPHY21_BASE, T931_USBPHY21_LENGTH,
//                                  *resource, ZX_CACHE_POLICY_UNCACHED_DEVICE,
//                                  &mmio);
// if (status != ZX_OK) return status;
//
// mmio->SetBits(kRegFlag, kRegOffset);
//
////////////////////////////////////////////////////////////////////////////////
// Example: An mmio region which is pinned in order to perform dma. Using views
// to increase safetey.
//
// std::optional<ddk::MmioBuffer> mmio;
// status = ddk::MmioBuffer::Create(pdev_mmio.offset, pdev_mmio.size,
//                                  zx::vmo(pdev_mmio.vmo),
//                                  ZX_CACHE_POLICY_UNCACHED_DEVICE, &mmio);
// if (status != ZX_OK) return status;
//
// ddk::MmioView dma_region = mmio->View(kDmaRegionOffset, kDmaRegionSize);
// ddk::MmioView dma_ctrl = mmio->View(kDmaCtrlRegOffset, kDmaCtrlRegSize);
//
// std::optional<ddk::MmioPinnedBuffer> dma_pinned_region;
// status = dma_region->Pin(&bti_, &dma_pinned_region);
// if (status != ZX_OK) return status;
//
// dma_ctrl->Write<uint64_t>(dma_pinnedRegion->get_paddr(), kPaddrOffset);
// <...>
//

// MmioPinnedBuffer is wrapper around mmio_pinned_buffer_t.
class MmioPinnedBuffer {

public:
    DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(MmioPinnedBuffer);

    explicit MmioPinnedBuffer(mmio_pinned_buffer_t pinned)
        : pinned_(pinned) {
        ZX_ASSERT(pinned_.paddr != 0);
    }

    ~MmioPinnedBuffer() {
        mmio_buffer_unpin(&pinned_);
    }

    MmioPinnedBuffer(MmioPinnedBuffer&& other) {
        transfer(std::move(other));
    }

    MmioPinnedBuffer& operator=(MmioPinnedBuffer&& other) {
        transfer(std::move(other));
        return *this;
    }

    void reset() {
        memset(&pinned_, 0, sizeof(pinned_));
    }

    zx_paddr_t get_paddr() const {
        return pinned_.paddr;
    }

private:
    void transfer(MmioPinnedBuffer&& other) {
        pinned_ = other.pinned_;
        other.reset();
    }
    mmio_pinned_buffer_t pinned_;
};

// Forward declaration
template <typename ViewType>
class MmioBase;

class MmioView;
typedef MmioBase<MmioView> MmioBuffer;

// MmioBase is wrapper around mmio_block_t.
// Use MmioBuffer (defined below) instead of MmioBase.
template <typename ViewType>
class MmioBase {

public:
    DISALLOW_COPY_AND_ASSIGN_ALLOW_MOVE(MmioBase);

    explicit MmioBase(mmio_buffer_t mmio)
        : mmio_(mmio), ptr_(reinterpret_cast<uintptr_t>(mmio.vaddr)) {
        ZX_ASSERT(mmio_.vaddr != nullptr);
    }

    virtual ~MmioBase() {
        mmio_buffer_release(&mmio_);
    }

    MmioBase(MmioBase&& other) {
        transfer(std::move(other));
    }

    MmioBase& operator=(MmioBase&& other) {
        transfer(std::move(other));
        return *this;
    }

    static zx_status_t Create(zx_off_t offset, size_t size, zx::vmo vmo, uint32_t cache_policy,
                              std::optional<MmioBase>* mmio_buffer) {
        mmio_buffer_t mmio;
        zx_status_t status = mmio_buffer_init(&mmio, offset, size, vmo.release(), cache_policy);
        if (status == ZX_OK) {
            *mmio_buffer = MmioBase(mmio);
        }
        return status;
    }

    static zx_status_t Create(zx_paddr_t base, size_t size, const zx::resource& resource,
                              uint32_t cache_policy, std::optional<MmioBase>* mmio_buffer) {
        mmio_buffer_t mmio;
        zx_status_t status = mmio_buffer_init_physical(&mmio, base, size, resource.get(),
                                                       cache_policy);
        if (status == ZX_OK) {
            *mmio_buffer = MmioBase(mmio);
        }
        return status;
    }

    void reset() {
        memset(&mmio_, 0, sizeof(mmio_));
    }

    void Info() const {
        zxlogf(INFO, "vaddr = %p\n", mmio_.vaddr);
        zxlogf(INFO, "size = %lu\n", mmio_.size);
    }

    void* get() const {
        return mmio_.vaddr;
    }
    size_t get_size() const {
        return mmio_.size;
    }
    zx::unowned_vmo get_vmo() const {
        return zx::unowned_vmo(mmio_.vmo);
    }

    zx_status_t Pin(const zx::bti& bti, std::optional<MmioPinnedBuffer>* pinned_buffer) {
        mmio_pinned_buffer_t pinned;
        zx_status_t status = mmio_buffer_pin(&mmio_, bti.get(), &pinned);
        if (status == ZX_OK) {
            *pinned_buffer = MmioPinnedBuffer(pinned);
        }
        return status;
    }

    // Provides a slice view into the mmio.
    // The returned slice object must not outlive this object.
    ViewType View(zx_off_t off) const {
        return ViewType(mmio_, off);
    }
    ViewType View(zx_off_t off, size_t size) const {
        return ViewType(mmio_, off, size);
    }

    uint32_t Read32(zx_off_t offs) const {
        return Read<uint32_t>(offs);
    }

    uint32_t ReadMasked32(uint32_t mask, zx_off_t offs) const {
        return ReadMasked<uint32_t>(mask, offs);
    }

    void Write32(uint32_t val, zx_off_t offs) const {
        Write<uint32_t>(val, offs);
    }

    void ModifyBits32(uint32_t bits, uint32_t mask, zx_off_t offs) const {
        ModifyBits<uint32_t>(bits, mask, offs);
    }

    void ModifyBits32(uint32_t val, uint32_t start, uint32_t width, zx_off_t offs) const {
        ModifyBits<uint32_t>(val, start, width, offs);
    }

    void SetBits32(uint32_t bits, zx_off_t offs) const {
        SetBits<uint32_t>(bits, offs);
    }

    void ClearBits32(uint32_t bits, zx_off_t offs) const {
        ClearBits<uint32_t>(bits, offs);
    }

    void CopyFrom32(const MmioBuffer& source, zx_off_t source_offs,
                    zx_off_t dest_offs, size_t count) const {
        CopyFrom<uint32_t>(source, source_offs, dest_offs, count);
    }

    template <typename T>
    T Read(zx_off_t offs) const {
        ZX_DEBUG_ASSERT(offs + sizeof(T) <= mmio_.size);
        ZX_DEBUG_ASSERT(ptr_);
        return *reinterpret_cast<volatile T*>(ptr_ + offs);
    }

    template <typename T>
    T ReadMasked(T mask, zx_off_t offs) const {
        return (Read<T>(offs) & mask);
    }

    template <typename T>
    void CopyFrom(const MmioBuffer& source, zx_off_t source_offs,
                  zx_off_t dest_offs, size_t count) const {
        for (size_t i = 0; i < count; i++) {
            T val = source.Read<T>(source_offs);
            Write<T>(val, dest_offs);
            source_offs = source_offs + sizeof(T);
            dest_offs = dest_offs + sizeof(T);
        }
    }

    template <typename T>
    void Write(T val, zx_off_t offs) const {
        ZX_DEBUG_ASSERT(offs + sizeof(T) <= mmio_.size);
        ZX_DEBUG_ASSERT(ptr_);
        *reinterpret_cast<volatile T*>(ptr_ + offs) = val;
        hw_mb();
    }

    template <typename T>
    void ModifyBits(T bits, T mask, zx_off_t offs) const {
        T val = Read<T>(offs);
        Write<T>(static_cast<T>((val & ~mask) | (bits & mask)), offs);
    }

    template <typename T>
    void SetBits(T bits, zx_off_t offs) const {
        ModifyBits<T>(bits, bits, offs);
    }

    template <typename T>
    void ClearBits(T bits, zx_off_t offs) const {
        ModifyBits<T>(0, bits, offs);
    }

    template <typename T>
    T GetBits(size_t shift, size_t count, zx_off_t offs) const {
        T mask = static_cast<T>(((static_cast<T>(1) << count) - 1) << shift);
        T val = Read<T>(offs);
        return static_cast<T>((val & mask) >> shift);
    }

    template <typename T>
    T GetBit(size_t shift, zx_off_t offs) const {
        return GetBits<T>(shift, 1, offs);
    }

    template <typename T>
    void ModifyBits(T bits, size_t shift, size_t count, zx_off_t offs) const {
        T mask = static_cast<T>(((static_cast<T>(1) << count) - 1) << shift);
        T val = Read<T>(offs);
        Write<T>(static_cast<T>((val & ~mask) | ((bits << shift) & mask)), offs);
    }

    template <typename T>
    void ModifyBit(bool val, size_t shift, zx_off_t offs) const {
        ModifyBits<T>(val, shift, 1, offs);
    }

    template <typename T>
    void SetBit(size_t shift, zx_off_t offs) const {
        ModifyBit<T>(true, shift, offs);
    }

    template <typename T>
    void ClearBit(size_t shift, zx_off_t offs) const {
        ModifyBit<T>(false, shift, offs);
    }

protected:
    mmio_buffer_t mmio_;

private:
    void transfer(MmioBase&& other) {
        mmio_ = other.mmio_;
        ptr_ = other.ptr_;
        other.reset();
    }

    uintptr_t ptr_;
};

// A sliced view that of an mmio which does not unmap on close. Must outlive
// mmio buffer it is created from.
class MmioView : public MmioBuffer {
public:
    MmioView(const mmio_buffer_t& mmio, zx_off_t offset)
        : MmioBuffer(mmio_buffer_t{
              .vaddr = static_cast<uint8_t*>(mmio.vaddr) + offset,
              .offset = mmio.offset + offset,
              .size = mmio.size - offset,
              .vmo = mmio.vmo,
          }) {
        ZX_ASSERT(offset < mmio.size);
    }

    MmioView(const mmio_buffer_t& mmio, zx_off_t offset, size_t size)
        : MmioBuffer(mmio_buffer_t{
              .vaddr = static_cast<uint8_t*>(mmio.vaddr) + offset,
              .offset = mmio.offset + offset,
              .size = size,
              .vmo = mmio.vmo,
          }) {
        ZX_ASSERT(size + offset <= mmio.size);
    }

    MmioView(const MmioView& mmio)
        : MmioBuffer(mmio.mmio_) {}

    virtual ~MmioView() override {
        // Prevent unmap operation from occurring.
        mmio_.vmo = ZX_HANDLE_INVALID;
    }
};

} //namespace ddk
