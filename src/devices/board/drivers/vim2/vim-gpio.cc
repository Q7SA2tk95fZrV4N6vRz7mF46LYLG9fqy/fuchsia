// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/hardware/platform/bus/c/banjo.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <limits.h>

#include <ddk/metadata/gpio.h>
#include <fbl/algorithm.h>
#include <soc/aml-s905x/s905x-gpio.h>
#include <soc/aml-s912/s912-gpio.h>
#include <soc/aml-s912/s912-hw.h>

#include "src/devices/board/drivers/vim2/gpio_light_bind.h"
#include "vim-gpios.h"
#include "vim.h"

namespace vim {

// S905X and S912 have same MMIO addresses
static const pbus_mmio_t gpio_mmios[] = {
    {
        .base = S912_GPIO_BASE,
        .length = S912_GPIO_LENGTH,
    },
    {
        .base = S912_GPIO_AO_BASE,
        .length = S912_GPIO_AO_LENGTH,
    },
    {
        .base = S912_GPIO_INTERRUPT_BASE,
        .length = S912_GPIO_INTERRUPT_LENGTH,
    },
};

// S905X and S912 have same GPIO IRQ numbers
static const pbus_irq_t gpio_irqs[] = {
    {
        .irq = S912_GPIO_IRQ_0,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_1,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_2,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_3,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_4,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_5,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_6,
        .mode = 0,
    },
    {
        .irq = S912_GPIO_IRQ_7,
        .mode = 0,
    },
    {
        .irq = S912_AO_GPIO_IRQ_0,
        .mode = 0,
    },
    {
        .irq = S912_AO_GPIO_IRQ_1,
        .mode = 0,
    },
};

// GPIOs to expose from generic GPIO driver.
static const gpio_pin_t gpio_pins[] = {
    // For wifi.
    {S912_WIFI_SDIO_WAKE_HOST},
    {GPIO_WIFI_DEBUG},
    // For thermal.
    {GPIO_THERMAL_FAN_O},
    {GPIO_THERMAL_FAN_1},
    // For ethernet.
    {GPIO_ETH_MAC_RST},
    {GPIO_ETH_MAC_INTR},
    // For display.
    {GPIO_DISPLAY_HPD},
    // For gpio-light
    {
        GPIO_SYS_LED,
    },
    // For eMMC.
    {S912_EMMC_RST},
    // For Wifi.
    {GPIO_WIFI_PWREN},
};

static const pbus_metadata_t gpio_metadata[] = {{
    .type = DEVICE_METADATA_GPIO_PINS,
    .data_buffer = reinterpret_cast<const uint8_t*>(&gpio_pins),
    .data_size = sizeof(gpio_pins),
}};

zx_status_t Vim::GpioInit() {
  pbus_dev_t gpio_dev = {};
  gpio_dev.name = "gpio";
  gpio_dev.vid = PDEV_VID_AMLOGIC;
  gpio_dev.pid = PDEV_PID_AMLOGIC_S912;
  gpio_dev.did = PDEV_DID_AMLOGIC_GPIO;
  gpio_dev.mmio_list = gpio_mmios;
  gpio_dev.mmio_count = std::size(gpio_mmios);
  gpio_dev.irq_list = gpio_irqs;
  gpio_dev.irq_count = std::size(gpio_irqs);
  gpio_dev.metadata_list = gpio_metadata;
  gpio_dev.metadata_count = std::size(gpio_metadata);

  zx_status_t status = pbus_.ProtocolDeviceAdd(ZX_PROTOCOL_GPIO_IMPL, &gpio_dev);
  if (status != ZX_OK) {
    zxlogf(ERROR, "GpioInit: pbus_protocol_device_add failed: %d", status);
    return status;
  }

  gpio_impl_ = ddk::GpioImplProtocolClient(parent());
  if (!gpio_impl_.is_valid()) {
    zxlogf(ERROR, "%s: device_get_protocol failed", __func__);
    return ZX_ERR_INTERNAL;
  }

  using light_name = char[ZX_MAX_NAME_LEN];
  static const light_name light_names[] = {"SYS_LED"};

  static const pbus_metadata_t light_metadata[] = {
      {
          .type = DEVICE_METADATA_NAME,
          .data_buffer = reinterpret_cast<const uint8_t*>(&light_names),
          .data_size = sizeof(light_names),
      },
  };

  pbus_dev_t light_dev = {};
  light_dev.name = "gpio-light";
  light_dev.vid = PDEV_VID_GENERIC;
  light_dev.pid = PDEV_PID_GENERIC;
  light_dev.did = PDEV_DID_GPIO_LIGHT;
  light_dev.metadata_list = light_metadata;
  light_dev.metadata_count = std::size(light_metadata);

  status = pbus_.AddComposite(&light_dev, reinterpret_cast<uint64_t>(gpio_light_fragments),
                              std::size(gpio_light_fragments), "pdev");
  if (status != ZX_OK) {
    zxlogf(ERROR, "GpioInit could not add gpio_light_dev: %d", status);
    return status;
  }

  return ZX_OK;
}
}  // namespace vim
