// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//
//
//

#ifndef NDEBUG
#include <stdio.h>
#endif

//
//
//

#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

//
//
//

#include "common/macros.h"
#include "common/vk/assert.h"
#include "common/vk/find_mem_type_idx.h"
#include "device.h"

//
// Section 11.6 of the Vulkan spec says:
//
//   The VkMemoryRequirements.memoryTypeBits member is identical for
//   all VkBuffer objects created with the same value for the flags
//   and usage members in the VkBufferCreateInfo structure and the
//   handleTypes member of the VkExternalMemoryBufferCreateInfo
//   structure passed to vkCreateBuffer. Further, if usage1 and usage2
//   of type VkBufferUsageFlags are such that the bits set in usage2
//   are a subset of the bits set in usage1, and they have the same
//   flags and VkExternalMemoryBufferCreateInfo ::handleTypes, then
//   the bits set in memoryTypeBits returned for usage1 must be a
//   subset of the bits set in memoryTypeBits returned for usage2, for
//   all values of flags.
//
// This presents some optimization opportunities but unfortunately it also
// results in the validator bleating.
//
// So for now, just capture the VkMemoryPropertyFlags, VkBufferUsageFlags and
// queue family indices in the allocator.
//
void
spinel_allocator_create(struct spinel_allocator * allocator,
                        VkMemoryPropertyFlags     properties,
                        VkBufferUsageFlags        usage,
                        VkSharingMode             mode,
                        uint32_t                  queue_family_count,
                        uint32_t const            queue_family_indices[])
{
  assert(queue_family_count > 0);
  assert(queue_family_count <= SPN_ALLOCATOR_MAX_QUEUE_FAMILY_INDICES);

  allocator->properties         = properties;
  allocator->usage              = usage;
  allocator->mode               = mode;
  allocator->queue_family_count = queue_family_count;

  memcpy(allocator->queue_family_indices,
         queue_family_indices,
         sizeof(*queue_family_indices) * queue_family_count);
}

//
// Query memory property flags
//
bool
spinel_allocator_is_coherent(struct spinel_allocator const * allocator)
{
  return (allocator->properties & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) != 0;
}

bool
spinel_allocator_is_device_local(struct spinel_allocator const * allocator)
{
  return (allocator->properties & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) != 0;
}

//
// Initialize allocated buffers
//
// Enabling can help detect if Spinel is depedenent on zero-initialized memory
// allocations.
//
#ifdef SPN_ALLOCATOR_ALLOC_FILL

static void
spinel_allocator_alloc_fill(struct spinel_allocator *     allocator,
                            VkPhysicalDevice              pd,
                            VkDevice                      d,
                            VkAllocationCallbacks const * ac,
                            VkDeviceSize                  size,
                            VkBuffer                      buf);

#define SPN_ALLOCATOR_ALLOC_FULL_BUFFER_USAGE VK_BUFFER_USAGE_TRANSFER_DST_BIT

#else

#define SPN_ALLOCATOR_ALLOC_FULL_BUFFER_USAGE 0

#endif

//
//
//
void
spinel_allocator_alloc_dbi_dm(struct spinel_allocator *     allocator,
                              VkPhysicalDevice              pd,
                              VkDevice                      d,
                              VkAllocationCallbacks const * ac,
                              VkDeviceSize                  size,
                              VkDeviceSize *                alignment,
                              struct spinel_dbi_dm *        dbi_dm)
{
  VkBufferCreateInfo const bci = {
    .sType                 = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
    .pNext                 = NULL,
    .flags                 = 0,  // only time this will ever change is if we're allocating protected
    .size                  = size,
    .usage                 = allocator->usage | SPN_ALLOCATOR_ALLOC_FULL_BUFFER_USAGE,
    .sharingMode           = allocator->mode,
    .queueFamilyIndexCount = allocator->queue_family_count,
    .pQueueFamilyIndices   = allocator->queue_family_indices
  };

  vk(CreateBuffer(d, &bci, ac, &dbi_dm->dbi.buffer));

  VkMemoryRequirements mr;

  vkGetBufferMemoryRequirements(d, dbi_dm->dbi.buffer, &mr);

  //
  // TODO(allanmac): Are we actually doing anything with the memory requirement
  // alignment?  Should we be?
  //
  if (alignment != NULL)
    {
      *alignment = mr.alignment;
    }

  dbi_dm->dbi.offset = 0;
  dbi_dm->dbi.range  = size;  // could be smaller than mr.size

  //
  // TODO(allanmac): Investigate dedicated allocations -- see NVIDIA docs.
  //
#if 0
  VkMemoryDedicatedAllocateInfo const dai =
    {
      .sType  = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
      .pNext  = NULL,
      .image  = VK_NULL_HANDLE,
      .buffer = dbi_dm->dbi.buffer
    };
#endif

  //
  // Indicate that we're going to get the buffer's address
  //
  VkMemoryAllocateFlagsInfo const mafi = {

    .sType      = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_FLAGS_INFO,
    .pNext      = NULL,
    .flags      = VK_MEMORY_ALLOCATE_DEVICE_ADDRESS_BIT_KHR,
    .deviceMask = 0
  };

  VkMemoryAllocateFlagsInfo const * const mafi_next =
    (allocator->usage & VK_BUFFER_USAGE_SHADER_DEVICE_ADDRESS_BIT)  //
      ? &mafi
      : NULL;

  //
  // Physical device memory properties are only used here.
  //
  VkPhysicalDeviceMemoryProperties pdmp;

  vkGetPhysicalDeviceMemoryProperties(pd, &pdmp);

  //
  // Allocate and bind memory
  //
  struct VkMemoryAllocateInfo const mai = {
    .sType           = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
    .pNext           = mafi_next,
    .allocationSize  = mr.size,
    .memoryTypeIndex = vk_find_mem_type_idx(&pdmp, mr.memoryTypeBits, allocator->properties),
  };

  vk(AllocateMemory(d, &mai, ac, &dbi_dm->dm));

  vk(BindBufferMemory(d, dbi_dm->dbi.buffer, dbi_dm->dm, 0));

#ifdef SPN_ALLOCATOR_ALLOC_FILL
  spinel_allocator_alloc_fill(allocator, pd, d, ac, size, dbi_dm->dbi.buffer);
#endif
}

//
//
//
void
spinel_allocator_alloc_dbi_dm_devaddr(struct spinel_allocator *      allocator,
                                      VkPhysicalDevice               pd,
                                      VkDevice                       d,
                                      VkAllocationCallbacks const *  ac,
                                      VkDeviceSize                   size,
                                      VkDeviceSize *                 alignment,
                                      struct spinel_dbi_dm_devaddr * dbi_dm_devaddr)
{
  spinel_allocator_alloc_dbi_dm(allocator, pd, d, ac, size, alignment, &dbi_dm_devaddr->dbi_dm);

  dbi_dm_devaddr->devaddr = spinel_dbi_to_devaddr(d, &dbi_dm_devaddr->dbi_dm.dbi);
}

//
//
//
void
spinel_allocator_free_dbi_dm(struct spinel_allocator *     allocator,
                             VkDevice                      d,
                             VkAllocationCallbacks const * ac,
                             struct spinel_dbi_dm *        dbi_dm)
{
  vkDestroyBuffer(d, dbi_dm->dbi.buffer, ac);

  vkFreeMemory(d, dbi_dm->dm, ac);
}

//
//
//
void
spinel_dbi_devaddr_init_devaddr(VkDevice d, struct spinel_dbi_devaddr * dbi_devaddr)
{
  dbi_devaddr->devaddr = spinel_dbi_to_devaddr(d, &dbi_devaddr->dbi);
}

//
//
//
void
spinel_dbi_dm_devaddr_init_devaddr(VkDevice d, struct spinel_dbi_dm_devaddr * dbi_dm_devaddr)
{
  dbi_dm_devaddr->devaddr = spinel_dbi_to_devaddr(d, &dbi_dm_devaddr->dbi_dm.dbi);
}

//
//
//
VkDeviceAddress
spinel_dbi_to_devaddr(VkDevice d, VkDescriptorBufferInfo const * dbi)
{
  VkBufferDeviceAddressInfo const bdai = {

    .sType  = VK_STRUCTURE_TYPE_BUFFER_DEVICE_ADDRESS_INFO,
    .pNext  = NULL,
    .buffer = dbi->buffer
  };

  VkDeviceAddress const devaddr = vkGetBufferDeviceAddress(d, &bdai) + dbi->offset;

  return devaddr;
}

//
//
//
void
spinel_dbi_devaddr_from_dbi(VkDevice                       d,
                            struct spinel_dbi_devaddr *    dbi_devaddr,
                            VkDescriptorBufferInfo const * dbi,
                            VkDeviceSize                   offset,
                            VkDeviceSize                   range)
{
  dbi_devaddr->dbi = (VkDescriptorBufferInfo){

    .buffer = dbi->buffer,
    .offset = dbi->offset + offset,
    .range  = range
  };

  dbi_devaddr->devaddr = spinel_dbi_to_devaddr(d, &dbi_devaddr->dbi);
}

//
// Enable explicit filling of all allocations by defining
// SPN_ALLOCATOR_ALLOC_FILL with the desired dword bit-pattern.
//
// Example:
//
//   #define SPN_ALLOCATOR_ALLOC_FILL 0x3CCCCCCC
//
#ifdef SPN_ALLOCATOR_ALLOC_FILL

//
//
//
static void
spinel_allocator_alloc_fill(struct spinel_allocator *     allocator,
                            VkPhysicalDevice              pd,
                            VkDevice                      d,
                            VkAllocationCallbacks const * ac,
                            VkDeviceSize                  size,
                            VkBuffer                      buf)
{
  VkQueue q;

  vkGetDeviceQueue(d, allocator->queue_family_indices[0], 0, &q);

  VkCommandPoolCreateInfo const cpci = {
    .sType            = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
    .pNext            = NULL,
    .flags            = 0,
    .queueFamilyIndex = allocator->queue_family_indices[0],
  };

  VkCommandPool cp;

  vk(CreateCommandPool(d, &cpci, ac, &cp));

  VkCommandBufferAllocateInfo const cbai = {

    .sType              = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
    .pNext              = NULL,
    .commandPool        = cp,
    .level              = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
    .commandBufferCount = 1,
  };

  VkCommandBuffer cb;

  vk(AllocateCommandBuffers(d, &cbai, &cb));

  VkCommandBufferBeginInfo const cbbi = {

    .sType            = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
    .pNext            = NULL,
    .flags            = 0,
    .pInheritanceInfo = NULL
  };

  vk(BeginCommandBuffer(cb, &cbbi));

  vkCmdFillBuffer(cb, buf, 0UL, size, SPN_ALLOCATOR_ALLOC_FILL);

  vk(EndCommandBuffer(cb));

  VkSubmitInfo const submit_info = {
    .sType                = VK_STRUCTURE_TYPE_SUBMIT_INFO,
    .pNext                = NULL,
    .commandBufferCount   = 1,
    .pCommandBuffers      = &cb,
    .waitSemaphoreCount   = 0,
    .pWaitSemaphores      = NULL,
    .pWaitDstStageMask    = NULL,
    .signalSemaphoreCount = 0,
    .pSignalSemaphores    = NULL,
  };

  vk(QueueSubmit(q, 1, &submit_info, VK_NULL_HANDLE));

  vk(QueueWaitIdle(q));

  vkFreeCommandBuffers(d, cp, 1, &cb);

  vkDestroyCommandPool(d, cp, ac);
}

#endif

//
//
//
