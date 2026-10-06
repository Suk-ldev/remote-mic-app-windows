#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <tlhelp32.h>
#include <detours.h>
#include <vector>
#include "hook_protocol.h"

using IoControl = NTSTATUS(NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID,
    PIO_STATUS_BLOCK, ULONG, PVOID, ULONG, PVOID, ULONG);

static IoControl real_ioctl = nullptr;
static HookShared* shared = nullptr;
static SRWLOCK g_lock = SRWLOCK_INIT;
static unsigned g_prev_mask = 0;
static volatile LONG started = 0;
// Count threads currently executing inside HookIoctl. Teardown drains this to
// zero before unloading the DLL, so no thread runs this module's code as it
// leaves memory (the DLL is not pinned; it unloads itself on clean stop).
static volatile LONG g_active = 0;

// Single logical producer (serialized by g_lock): write the slot, publish head.
static void PushEdge(LONG button, LONG pressed) {
    const LONG pos = shared->edge_head;
    const int slot = pos % kEdgeRing;
    shared->edges[slot].button = button;
    shared->edges[slot].pressed = pressed;
    MemoryBarrier();
    shared->edge_head = pos + 1;
}

// Rewrite every voice-key slot to the armed usage, in place. Returns the number
// of slots rewritten (0 = this report does not carry the voice key).
//
// Position matters and could not be settled by reading code alone: it is not
// established whether the consumer ingests this buffer DURING the real call or
// reads it after the call returns. So the substitution runs at BOTH points and
// never restores. The transform is idempotent per slot (0x003E -> target, and a
// slot already holding the target is left alone), so running it twice is the same
// as running it once, and whichever side consumes the buffer sees a patched
// report. Patching only one position risks delivering the key one report late --
// on a state-semantics report that would mean Alt going down on RELEASE and
// staying down, i.e. a stuck modifier. See ATTRIBUTION.md (2026-10-06).
static unsigned PatchVoiceSlots(unsigned char* d, ULONG len, unsigned to) {
    if (!HookReportHeaderOk(d, len)) return 0;
    unsigned written = 0;
    for (unsigned i = 3; i < 9; i += 2) {
        const unsigned u = d[i] | (static_cast<unsigned>(d[i + 1]) << 8);
        if (u != kVoiceKeyUsage) continue;
        const unsigned char low = static_cast<unsigned char>(to & 0xffu);
        const unsigned char high = static_cast<unsigned char>((to >> 8) & 0xffu);
        bool wrote = false;
        __try {
            d[i] = low;
            d[i + 1] = high;
            wrote = true;
        } __except (EXCEPTION_EXECUTE_HANDLER) {
            wrote = false;
        }
        if (!wrote) {
            // Not writable as mapped. Lift protection for the two bytes, write,
            // then put the original protection back -- the same fallback the
            // upstream agent needed in this host (ATTRIBUTION.md).
            DWORD previous = 0;
            if (VirtualProtect(d + i, 2, PAGE_READWRITE, &previous)) {
                __try {
                    d[i] = low;
                    d[i + 1] = high;
                    wrote = true;
                } __except (EXCEPTION_EXECUTE_HANDLER) {
                    wrote = false;
                }
                DWORD restored = 0;
                VirtualProtect(d + i, 2, previous, &restored);
            }
        }
        if (!wrote) {
            if (shared) InterlockedIncrement(&shared->synth_write_failed);
            continue;
        }
        // Read back: a successful store is not proof the replacement is what the
        // next reader will see (the upstream agent verifies for the same reason).
        const unsigned back = d[i] | (static_cast<unsigned>(d[i + 1]) << 8);
        if (back != to) {
            if (shared) InterlockedIncrement(&shared->synth_verify_failed);
            continue;
        }
        ++written;
        if (shared) InterlockedIncrement(&shared->synth_written);
    }
    return written;
}

// Is the voice key present in this report? Counted separately before and after
// the real call so one real-machine run tells us which position actually carries
// the live report, without another probe build.
static bool VoiceKeyPresent(const unsigned char* d, ULONG len) {
    if (!HookReportHeaderOk(d, len)) return false;
    for (unsigned i = 3; i < 9; i += 2) {
        if ((d[i] | (static_cast<unsigned>(d[i + 1]) << 8)) == kVoiceKeyUsage) return true;
    }
    return false;
}

static void ProcessReport(const unsigned char* buf, ULONG len) {
    unsigned mask = 0;
    if (!HookDecodeTargets(buf, len, mask)) return;
    AcquireSRWLockExclusive(&g_lock);
    InterlockedIncrement(&shared->reports);
    const unsigned changed = mask ^ g_prev_mask;
    if (changed) {
        if (changed & 1u) PushEdge(HB_Back, (mask & 1u) ? 1 : 0);
        if (changed & 2u) PushEdge(HB_VolumeUp, (mask & 2u) ? 1 : 0);
        if (changed & 4u) PushEdge(HB_VolumeDown, (mask & 4u) ? 1 : 0);
        g_prev_mask = mask;
    }
    ReleaseSRWLockExclusive(&g_lock);
}

// Observation never touches the real I/O: the real call runs first and its
// result/last-error are returned unchanged. The only write is the voice-key
// substitution, and only while the app has armed it (synth_to != 0).
static NTSTATUS NTAPI HookIoctl(HANDLE file, HANDLE event, PIO_APC_ROUTINE apc,
    PVOID context, PIO_STATUS_BLOCK iosb, ULONG code, PVOID input, ULONG input_size,
    PVOID output, ULONG output_size) {
    // Mark this module busy for the whole call so teardown can wait us out before
    // unloading. Balanced by the decrement below on every return path.
    InterlockedIncrement(&g_active);
    const bool carrier = code == kReadCharacteristicIoctl && output && output_size == 9 &&
        shared && InterlockedCompareExchange(&shared->state, 0, 0) == HS_Capturing;
    // Snapshot the armed target once: the app may disarm mid-call, and the two
    // substitution points must agree on what this report is being rewritten to.
    const unsigned synth = carrier
        ? static_cast<unsigned>(InterlockedCompareExchange(&shared->synth_to, 0, 0))
        : 0u;
    unsigned char* const buffer = reinterpret_cast<unsigned char*>(output);
    if (synth && HookSynthUsageAllowed(synth)) {
        __try {
            if (VoiceKeyPresent(buffer, output_size)) {
                InterlockedIncrement(&shared->synth_seen_before);
                PatchVoiceSlots(buffer, output_size, synth);
            }
        } __except (EXCEPTION_EXECUTE_HANDLER) {}
    }
    const NTSTATUS result = real_ioctl(file, event, apc, context, iosb, code,
        input, input_size, output, output_size);
    const DWORD saved = GetLastError();
    if (result == 0 && carrier) {
        __try {
            ProcessReport(buffer, output_size);
            if (synth && HookSynthUsageAllowed(synth) && VoiceKeyPresent(buffer, output_size)) {
                InterlockedIncrement(&shared->synth_seen_after);
                PatchVoiceSlots(buffer, output_size, synth);
            }
        } __except (EXCEPTION_EXECUTE_HANDLER) {}
    }
    SetLastError(saved);
    InterlockedDecrement(&g_active);
    return result;
}

static LONG ChangeHook(bool install) {
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    if (snapshot == INVALID_HANDLE_VALUE) return static_cast<LONG>(GetLastError());
    std::vector<HANDLE> threads;
    THREADENTRY32 entry{sizeof(entry)};
    LONG result = NO_ERROR;
    LONG seen = 0, denied = 0;
    if (!Thread32First(snapshot, &entry)) result = static_cast<LONG>(GetLastError());
    else do {
        if (entry.th32OwnerProcessID != GetCurrentProcessId() ||
            entry.th32ThreadID == GetCurrentThreadId()) continue;
        ++seen;
        HANDLE thread = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT |
            THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION, FALSE, entry.th32ThreadID);
        if (!thread) {
            // WUDFHost is a write-restricted LOCAL SERVICE process: it cannot open
            // all of its OWN peer threads for SUSPEND/SET_CONTEXT (ERROR_ACCESS_DENIED),
            // and a thread can also exit mid-enumeration (ERROR_INVALID_PARAMETER).
            // Neither is fatal -- previously ACCESS_DENIED aborted the whole hook and
            // it never installed. Skip the thread; we still fix up the calling thread
            // below (pseudo-handle, no OpenThread), and Detours rewrites ntdll's 5-byte
            // stub as one transaction, so an un-updated peer only matters in the
            // vanishingly rare case its RIP sits inside those 5 bytes at commit.
            ++denied;
            continue;
        }
        threads.push_back(thread);
    } while (Thread32Next(snapshot, &entry));
    CloseHandle(snapshot);
    if (shared) {
        shared->threads_seen = seen;
        shared->threads_denied = denied;
        shared->threads_updated = static_cast<LONG>(threads.size());
    }
    if (result == NO_ERROR) {
        result = DetourTransactionBegin();
        if (result == NO_ERROR) {
            result = install
                ? DetourAttach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl)
                : DetourDetach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl);
            // Always fix up the calling thread; GetCurrentThread() is a pseudo-handle
            // that needs no OpenThread and is immune to the write-restricted token.
            if (result == NO_ERROR) result = DetourUpdateThread(GetCurrentThread());
            for (HANDLE thread : threads) {
                if (result != NO_ERROR) break;
                DWORD exit_code = 0;
                if (GetExitCodeThread(thread, &exit_code) && exit_code != STILL_ACTIVE) continue;
                result = DetourUpdateThread(thread);
                if (result != NO_ERROR) break;
            }
            if (result == NO_ERROR) result = DetourTransactionCommit();
            else DetourTransactionAbort();
        }
    }
    for (HANDLE thread : threads) CloseHandle(thread);
    return result;
}

extern "C" __declspec(dllexport) DWORD WINAPI SayAllHookStart(void*);

static DWORD RunHook() {
    if (InterlockedCompareExchange(&started, 1, 0)) return ERROR_ALREADY_EXISTS;
    wchar_t name[96];
    HookMappingName(name, GetCurrentProcessId());
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, name);
    if (!mapping) { const DWORD e = GetLastError(); InterlockedExchange(&started, 0); return e; }
    shared = static_cast<HookShared*>(MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE,
        0, 0, sizeof(HookShared)));
    CloseHandle(mapping);
    if (!shared) { const DWORD e = GetLastError(); InterlockedExchange(&started, 0); return e; }
    if (shared->magic != kHookMagic) { InterlockedExchange(&started, 0); return ERROR_INVALID_DATA; }
    InterlockedExchange(&shared->state, HS_Starting);
    // Grab our own module handle WITHOUT bumping the refcount: LoadRemote's
    // LoadLibraryW holds the single reference, and on clean teardown we release
    // exactly that one via FreeLibraryAndExitThread so the DLL unloads instead of
    // accumulating a pinned copy per connect in the long-lived device-pool host.
    HMODULE self = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            reinterpret_cast<LPCWSTR>(&SayAllHookStart), &self)) {
        shared->error = static_cast<LONG>(GetLastError());
        InterlockedExchange(&started, 0);
        InterlockedExchange(&shared->state, HS_Failed);
        return static_cast<DWORD>(shared->error);
    }
    real_ioctl = reinterpret_cast<IoControl>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"),
        "NtDeviceIoControlFile"));
    LONG result = real_ioctl ? ChangeHook(true) : ERROR_PROC_NOT_FOUND;
    if (result != NO_ERROR) {
        shared->error = result;
        InterlockedExchange(&started, 0);
        InterlockedExchange(&shared->state, HS_Failed);
        return static_cast<DWORD>(result);
    }
    InterlockedExchange(&shared->state, HS_Capturing);
    // App-controlled lifetime: run until it requests teardown (method 3 unhooks
    // on remote sleep/disconnect). No fixed timer.
    while (!InterlockedCompareExchange(&shared->stop, 0, 0)) Sleep(25);
    InterlockedExchange(&shared->state, HS_Stopping);
    result = ChangeHook(false);
    // Flush any button left logically down so the app never sees a stuck press.
    AcquireSRWLockExclusive(&g_lock);
    if (g_prev_mask & 1u) PushEdge(HB_Back, 0);
    if (g_prev_mask & 2u) PushEdge(HB_VolumeUp, 0);
    if (g_prev_mask & 4u) PushEdge(HB_VolumeDown, 0);
    g_prev_mask = 0;
    ReleaseSRWLockExclusive(&g_lock);
    shared->error = result;
    // Detach is done; drain any thread still inside HookIoctl before we free the
    // module, so none is executing our code as it unloads. Bounded wait -- if a
    // straggler never clears (should not happen; the callback is short), fall back
    // to staying resident: a rare leaked copy is better than freeing live code.
    bool drained = false;
    for (int i = 0; i < 400; ++i) {  // ~2s
        if (InterlockedCompareExchange(&g_active, 0, 0) == 0) { drained = true; break; }
        Sleep(5);
    }
    InterlockedExchange(&started, 0);
    InterlockedExchange(&shared->state, result == NO_ERROR ? HS_Stopped : HS_Failed);
    if (drained && self) {
        Sleep(50);  // margin for the uncounted prologue/epilogue of any last caller
        UnmapViewOfFile(shared);
        shared = nullptr;
        // Releases LoadRemote's reference and exits this thread atomically; the DLL
        // unloads once the refcount hits zero. Never returns.
        FreeLibraryAndExitThread(self, static_cast<DWORD>(result));
    }
    return static_cast<DWORD>(result);
}

extern "C" __declspec(dllexport) DWORD WINAPI SayAllHookStart(void*) {
    try { return RunHook(); }
    catch (...) {
        if (shared) {
            shared->error = ERROR_NOT_ENOUGH_MEMORY;
            InterlockedExchange(&shared->state, HS_Failed);
        }
        return ERROR_NOT_ENOUGH_MEMORY;
    }
}

BOOL WINAPI DllMain(HINSTANCE, DWORD, LPVOID) { return TRUE; }
