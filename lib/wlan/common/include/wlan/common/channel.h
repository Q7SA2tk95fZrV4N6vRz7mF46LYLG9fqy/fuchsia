// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef GARNET_LIB_WLAN_COMMON_INCLUDE_WLAN_COMMON_CHANNEL_H_
#define GARNET_LIB_WLAN_COMMON_INCLUDE_WLAN_COMMON_CHANNEL_H_

#include <fuchsia/wlan/mlme/cpp/fidl.h>

#include <wlan/protocol/mac.h>

#include <cstdint>
#include <string>

bool operator==(const wlan_channel_t& lhs, const wlan_channel_t& rhs);
bool operator!=(const wlan_channel_t& lhs, const wlan_channel_t& rhs);

namespace wlan {
namespace common {

typedef uint16_t Mhz;

// IEEE Std 802.11-2016, Annex E
constexpr Mhz kBaseFreq2Ghz = 2407;
constexpr Mhz kBaseFreq5Ghz = 5000;

// TODO(porce): Replace all channel > 14 test throughout the codes
bool Is5Ghz(uint8_t channel_number);
bool Is2Ghz(uint8_t channel_number);
bool Is5Ghz(const wlan_channel_t& chan);
bool Is2Ghz(const wlan_channel_t& chan);

bool IsValidChan2Ghz(const wlan_channel_t& chan);
bool IsValidChan5Ghz(const wlan_channel_t& chan);
bool IsValidChan(const wlan_channel_t& chan);

Mhz GetCenterFreq(const wlan_channel_t& chan);
uint8_t GetCenterChanIdx(const wlan_channel_t& chan);

std::string ChanStr(const wlan_channel_t& chan);
std::string ChanStrLong(const wlan_channel_t& chan);

uint8_t GetValidCbw(const wlan_channel_t& chan);
std::string GetPhyStr(enum PHY phy);

struct Channel {
    wlan_channel_t chan;
    // TODO(porce): Validation
    // TODO(porce): Notation string.
    // TODO(porce): Center frequencies.
    // Define the rule to translsate center frequency to/from channel numbering.
    // See IEEE Std 802.11-2016 19.3.15
};

wlan_channel_t FromFidl(const ::fuchsia::wlan::mlme::WlanChan& fidl_chan);
::fuchsia::wlan::mlme::WlanChan ToFidl(const wlan_channel_t& chan);

extern const char* kCbwStr[];

PHY FromFidl(::fuchsia::wlan::mlme::PHY phy);
::fuchsia::wlan::mlme::PHY ToFidl(PHY phy);

}  // namespace common
}  // namespace wlan

#endif  // GARNET_LIB_WLAN_COMMON_INCLUDE_WLAN_COMMON_CHANNEL_H_
