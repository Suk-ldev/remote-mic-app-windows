#pragma once
#include <windows.h>
#include <cstdint>
#include <cwchar>

// Diagnostic-only ABI. Discovery build: we do not yet know which ntdll I/O call
// carries the RC003 vendor (Back/Volume) reports on this Windows/Bluetooth stack
// -- ZSTDJan's NtDeviceIoControlFile(0x80018483) tap matched zero calls here.
// The tap therefore tallies EVERY NtDeviceIoControlFile (keyed by IOCTL code)
// and every NtReadFile, with success/pending counts, output length range, and a
// short sample of reports successful buffers, so the host can correlate a table
// row with physical button presses and read the on-wire byte layout directly.
// This runs only on the user's own machine against their own remote, with the
// user actively pressing keys; the sampled bytes are button reports, no PII.
constexpr LONG kProbeMagic = 0x53415933;  // v3: output-buffer sampling + report ring
constexpr DWORD kReadCharacteristicIoctl = 0x80018483;  // confirmed RC003 carrier IOCTL on this stack
constexpr DWORD kProbeDurationMs = 1800000;  // 30 min: remote user presses at leisure; host reads the ring live
constexpr int kMaxTags = 64;
constexpr int kSampleBytes = 24;
constexpr int kRingSize = 24;   // last N carrier output buffers, to read per-button layout
constexpr int kRingBytes = 12;  // a report is 9 bytes; 12 leaves headroom
constexpr DWORD kTagReadFile = 0xFFFF0001;  // sentinel row for NtReadFile (real IOCTL codes never collide)

enum ProbeState : LONG { Idle, Starting, Capturing, Stopping, Stopped, Failed };

struct TagEntry {
    volatile LONG tag;        // IOCTL code, or kTagReadFile; 0 = empty slot
    volatile LONG count;      // total calls seen
    volatile LONG success;    // returned STATUS_SUCCESS
    volatile LONG pending;    // returned STATUS_PENDING (async; data not ready at return)
    volatile LONG reports;      // successes with 1..32 bytes written (likely a report)
    volatile LONG min_len;    // min bytes written on success (0 = unset)
    volatile LONG max_len;    // max bytes written on success
    volatile LONG sample_len; // bytes captured in sample[]
    volatile unsigned char sample[kSampleBytes];  // last reports successful buffer
};

struct RingSample {
    volatile LONG len;
    volatile unsigned char data[kRingBytes];
};

struct ProbeCounters {
    LONG magic;
    volatile LONG state;
    volatile LONG stop;
    volatile LONG error;
    volatile LONG total_ioctl;   // all NtDeviceIoControlFile calls (any code)
    volatile LONG total_read;    // all NtReadFile calls
    volatile LONG overflow;      // distinct tags beyond kMaxTags
    volatile LONG table_len;     // tags claimed
    volatile LONG ring_head;     // total carrier reports pushed (slot = (head-1) % kRingSize)
    TagEntry table[kMaxTags];
    RingSample ring[kRingSize];  // recent carrier (0x80018483) output buffers
};

inline void ProbeMappingName(wchar_t (&name)[96], DWORD pid) {
    swprintf_s(name, L"Global\\SayAll.Rc003.Diagnostic.%lu", pid);
}

// Record one observed I/O completion into the shared table (lock-free slot claim).
// buf/len describe the bytes actually written (from IO_STATUS_BLOCK.Information);
// they are only read for a synchronous STATUS_SUCCESS.
inline void ProbeRecord(ProbeCounters* c, DWORD tag, LONG status, ULONG len, const void* buf) {
    for (int i = 0; i < kMaxTags; ++i) {
        LONG cur = InterlockedCompareExchange(&c->table[i].tag, static_cast<LONG>(tag), 0);
        if (cur == 0) { InterlockedIncrement(&c->table_len); cur = static_cast<LONG>(tag); }
        if (cur != static_cast<LONG>(tag)) continue;
        InterlockedIncrement(&c->table[i].count);
        if (status == 0) {
            InterlockedIncrement(&c->table[i].success);
            if (c->table[i].min_len == 0 || static_cast<LONG>(len) < c->table[i].min_len)
                c->table[i].min_len = static_cast<LONG>(len);
            if (static_cast<LONG>(len) > c->table[i].max_len)
                c->table[i].max_len = static_cast<LONG>(len);
            if (len >= 1 && len <= 32 && buf) {
                InterlockedIncrement(&c->table[i].reports);
                const ULONG n = len < kSampleBytes ? len : kSampleBytes;
                __try {
                    unsigned char tmp[kSampleBytes];
                    memcpy(tmp, buf, n);
                    for (ULONG k = 0; k < n; ++k) c->table[i].sample[k] = tmp[k];
                    c->table[i].sample_len = static_cast<LONG>(n);
                } __except (EXCEPTION_EXECUTE_HANDLER) {}
            }
        } else if (status == 0x103) {
            InterlockedIncrement(&c->table[i].pending);
        }
        return;
    }
    InterlockedIncrement(&c->overflow);
}

// Push one carrier output buffer into the ring so the host can read the actual
// per-button byte layout after a press session (best-effort, SEH-guarded).
inline void ProbePushRing(ProbeCounters* c, const void* buf, ULONG len) {
    if (!buf || len == 0) return;
    const LONG idx = InterlockedIncrement(&c->ring_head) - 1;
    const int slot = static_cast<int>(((idx % kRingSize) + kRingSize) % kRingSize);
    const ULONG n = len < static_cast<ULONG>(kRingBytes) ? len : static_cast<ULONG>(kRingBytes);
    __try {
        unsigned char tmp[kRingBytes];
        memcpy(tmp, buf, n);
        for (ULONG k = 0; k < n; ++k) c->ring[slot].data[k] = tmp[k];
        c->ring[slot].len = static_cast<LONG>(n);
    } __except (EXCEPTION_EXECUTE_HANDLER) {}
}

// Retained for the eventual production path once the carrier is identified.
inline bool DecodeMissingMask(const unsigned char* data, ULONG size, unsigned& mask) {
    if (size != 9 || data[0] != 1 || data[1] != 0 || data[2] != 0) return false;
    mask = 0;
    for (unsigned i = 3; i < 9; i += 2) {
        const unsigned usage = data[i] | (static_cast<unsigned>(data[i + 1]) << 8);
        if (usage == 0x00f1) mask |= 1;
        if (usage == 0x0080) mask |= 2;
        if (usage == 0x0081) mask |= 4;
    }
    return true;
}
