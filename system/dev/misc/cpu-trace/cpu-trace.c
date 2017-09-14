// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// See the README.md in this directory for documentation.

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>

#include <ddk/binding.h>
#include <ddk/device.h>
#include <ddk/driver.h>

#include <zircon/syscalls.h>
#include <zircon/syscalls/resource.h>
#include <zircon/types.h>

#include "cpu-trace-private.h"

static zx_status_t cpu_trace_open(void* ctx, zx_device_t** dev_out, uint32_t flags) {
    cpu_trace_device_t* dev = ctx;
    if (dev->opened)
        return ZX_ERR_ALREADY_BOUND;

    dev->opened = true;
    return ZX_OK;
}

static zx_status_t cpu_trace_close(void* ctx, uint32_t flags) {
    cpu_trace_device_t* dev = ctx;

    dev->opened = false;
    return ZX_OK;
}

static zx_status_t cpu_trace_ioctl(void* ctx, uint32_t op,
                                   const void* cmd, size_t cmdlen,
                                   void* reply, size_t replymax,
                                   size_t* out_actual) {
    cpu_trace_device_t* dev = ctx;

    mtx_lock(&dev->lock);

    ssize_t result;
    switch (IOCTL_FAMILY(op)) {
        case IOCTL_FAMILY_IPT:
            result = ipt_ioctl(dev, op, cmd, cmdlen,
                               reply, replymax, out_actual);
            break;
        default:
            result = ZX_ERR_INVALID_ARGS;
            break;
    }

    mtx_unlock(&dev->lock);

    return result;
}

static void cpu_trace_release(void* ctx) {
    cpu_trace_device_t* dev = ctx;

    ipt_release(dev);

    free(dev);
}

static zx_protocol_device_t cpu_trace_device_proto = {
    .version = DEVICE_OPS_VERSION,
    .open = cpu_trace_open,
    .close = cpu_trace_close,
    .ioctl = cpu_trace_ioctl,
    .release = cpu_trace_release,
};

static zx_status_t cpu_trace_bind(void* ctx, zx_device_t* parent, void** cookie) {
    ipt_init_once();

    cpu_trace_device_t* dev = calloc(1, sizeof(*dev));
    if (!dev)
        return ZX_ERR_NO_MEMORY;

    device_add_args_t args = {
        .version = DEVICE_ADD_ARGS_VERSION,
        .name = "cpu-trace",
        .ctx = dev,
        .ops = &cpu_trace_device_proto,
    };

    zx_status_t status;
    if ((status = device_add(parent, &args, NULL)) < 0) {
        free(dev);
        return status;
    }

    return ZX_OK;
}

static zx_driver_ops_t cpu_trace_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .bind = cpu_trace_bind,
};

ZIRCON_DRIVER_BEGIN(cpu_trace, cpu_trace_driver_ops, "zircon", "0.1", 1)
    BI_MATCH_IF(EQ, BIND_PROTOCOL, ZX_PROTOCOL_MISC_PARENT),
ZIRCON_DRIVER_END(cpu_trace)
