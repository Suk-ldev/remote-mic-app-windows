#pragma once
#include <windows.h>
#include <cstdint>
#include <cwchar>

// Production tap ABI between the injected hook (inside WUDFHost, LOCAL SERVICE)
// and the SayAll app. The hook observes RC003's Back/Volume vendor reports that
// Windows drops (see docs/investigations/2026-09-13-rc003-wudfhost-tap-confirmed.md)
// and publishes press/release EDGES for exactly those three buttons. No raw HID
// payload, device identity, or pointer crosses the boundary -- only button id +
// up/down.
//
// It also performs ONE in-place substitution, and only when the app explicitly
// arms it (synth_to != 0): the voice key's usage is rewritten to a single
// measured modifier usage so the OS receives that key as a DEVICE REPORT rather
// than as SendInput. IMEs and voice tools that drop LLKHF_INJECTED keys (Doubao,
// Chatterfly) only ever respond to the former. Nothing else in the report is
// modified, blocked, or delayed -- see Bugs/2026-10-06-injected-key-voice-tools.md.
constexpr LONG kHookMagic = 0x53414832;  // "SAH2" -- bumped with the synth fields
constexpr DWORD kReadCharacteristicIoctl = 0x80018483;  // BthLEEnum read-characteristic (confirmed carrier)
constexpr int kEdgeRing = 256;

// The remote's voice key also reports as keyboard F5 (usage 0x003E) in this same
// carrier. The ATVV audio session runs over the BLE protocol layer and does not
// pass through here, and Windows maps this usage to nothing useful on the remote,
// so replacing it in the report costs nothing (same source of truth as
// key_suppressor, which swallows the F5 that would otherwise reach the OS).
constexpr unsigned kVoiceKeyUsage = 0x003Eu;

// Substitution targets whitelist. A replacement usage is re-translated to a VK by
// the Windows HID mapper, so an unmeasured usage may produce an unexpected key or
// no event at all: measured on real hardware upstream (GetSayAll v0.5.0, probe
// wudf_ioctl_synth.py) -- 0x00E6 -> VK_RMENU (Right Alt), 0x00E2 -> VK_LMENU.
// Extending this table requires measuring the new usage first; the Rust side
// pre-filters with the same list (crates/sayall-windows/src/rc003_hook.rs).
inline bool HookSynthUsageAllowed(unsigned usage) {
    return usage == 0x00E2u || usage == 0x00E6u;
}

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
    volatile LONG threads_seen;     // host peer threads enumerated at hook time (diag)
    volatile LONG threads_updated;  // peer threads DetourUpdateThread could fix up (diag)
    volatile LONG threads_denied;   // peer threads OpenThread refused -- write-restricted host (diag)
    // Voice-key substitution. The app writes synth_to (0 = off, else a usage that
    // passed HookSynthUsageAllowed); the hook publishes what it actually did so a
    // single log line separates "app armed it" from "it ran" from "it landed".
    volatile LONG synth_to;           // armed replacement usage, 0 = disabled
    volatile LONG synth_seen_before;  // voice usage present BEFORE the real call
    volatile LONG synth_seen_after;   // voice usage present AFTER the real call
    volatile LONG synth_written;      // slots rewritten (either position)
    volatile LONG synth_verify_failed;  // readback did not show the replacement
    volatile LONG synth_write_failed;   // buffer not writable even after unprotect
    HookEdge edges[kEdgeRing];
};

inline void HookMappingName(wchar_t (&name)[96], DWORD pid) {
    swprintf_s(name, L"Global\\SayAll.Rc003.Hook.%lu", pid);
}

// The carrier report: report id 1, two zero bytes, then three little-endian
// 16-bit usage slots at offsets 3/5/7. Observation and substitution must agree on
// exactly what counts as a report, so both go through this predicate.
inline bool HookReportHeaderOk(const unsigned char* d, ULONG len) {
    return len == 9 && d[0] == 1 && d[1] == 0 && d[2] == 0;
}

// Decode a 9-byte carrier report into a bitmask of the three target buttons:
// bit0 = Back (usage 0x00F1), bit1 = Volume+ (0x0080), bit2 = Volume- (0x0081).
// All other usages (working keys Windows already delivers) are ignored.
inline bool HookDecodeTargets(const unsigned char* d, ULONG len, unsigned& mask) {
    if (!HookReportHeaderOk(d, len)) return false;
    mask = 0;
    for (unsigned i = 3; i < 9; i += 2) {
        const unsigned u = d[i] | (static_cast<unsigned>(d[i + 1]) << 8);
        if (u == 0x00f1) mask |= 1u;
        else if (u == 0x0080) mask |= 2u;
        else if (u == 0x0081) mask |= 4u;
    }
    return true;
}
