// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_VIM3_USB_PHY_XHCI_DEVICE_H_
#define SRC_DEVICES_USB_DRIVERS_VIM3_USB_PHY_XHCI_DEVICE_H_

#include <ddktl/device.h>

namespace vim3_usb_phy {

class XhciDevice;
using XhciDeviceType = ddk::Device<XhciDevice>;

// Device for binding the XHCI driver.
class XhciDevice : public XhciDeviceType {
 public:
  explicit XhciDevice(zx_device_t* parent) : XhciDeviceType(parent) {}

  // Device protocol implementation.
  void DdkRelease() { delete this; }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(XhciDevice);
};

}  // namespace vim3_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_VIM3_USB_PHY_XHCI_DEVICE_H_
