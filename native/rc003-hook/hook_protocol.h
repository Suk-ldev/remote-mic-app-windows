#pragma once
#include <windows.h>
#include <cstdint>
#include <cwchar>

// Production tap ABI between the injected hook (inside WUDFHost, LOCAL SERVICE)
// and the SayAll app. The hook observes RC003's Back/Volume vendor reports that
// Windows drops (see docs/investigations/2026-09-13-rc003-wudfhost-tap-confirmed.md)
// and publishes press/release EDGES for exactly those three buttons. No raw HID
// payload, device identity, or pointer crosses the boundary -- only button id +
// up/down. The hook is read-only: it never modifies, blocks, or delays the real
// I/O; it only observes the completed output buffer.
constexpr LONG kHookMagic = 0x53414831;  // "SAH1"
constexpr DWORD kReadCharacteristicIoctl = 0x80018483;  // BthLEEnum read-characteristic (confirmed carrier)
constexpr int kEdgeRing = 256;

enum HookButton : LONG { HB_Back = 0, HB_VolumeUp = 1, HB_VolumeDown = 2 };
enum HookState : LONG { HS_Idle, HS_Starting, HS_Capturing, HS_Stopping, HS_Stopped, HS_Failed };

struct HookEdge {
    volatile LONG button;   // HookButton
    volatile LONG pressed;  // 1 = down, 0 = up
};

struct HookShared {
    LONG magic;
    volatile LONG state;      // HookState
    volatile LONG stop;       // app sets 1 to request clean unhook
    volatile LONG error;      // Win32/Detours error on failure
    volatile LONG edge_head;  // total edges produced; app drains [tail, edge_head)
    volatile LONG reports;    // carrier reports observed (diagnostic)
    HookEdge edges[kEdgeRing];
};

inline void HookMappingName(wchar_t (&name)[96], DWORD pid) {
    swprintf_s(name, L"Global\\SayAll.Rc003.Hook.%lu", pid);
}

// Decode a 9-byte carrier report into a bitmask of the three target buttons:
// bit0 = Back (usage 0x00F1), bit1 = Volume+ (0x0080), bit2 = Volume- (0x0081).
// All other usages (working keys Windows already delivers) are ignored.
inline bool HookDecodeTargets(const unsigned char* d, ULONG len, unsigned& mask) {
    if (len != 9 || d[0] != 1 || d[1] != 0 || d[2] != 0) return false;
    mask = 0;
    for (unsigned i = 3; i < 9; i += 2) {
        const unsigned u = d[i] | (static_cast<unsigned>(d[i + 1]) << 8);
        if (u == 0x00f1) mask |= 1u;
        else if (u == 0x0080) mask |= 2u;
        else if (u == 0x0081) mask |= 4u;
    }
    return true;
}
