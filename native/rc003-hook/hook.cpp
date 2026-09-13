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

// Single logical producer (serialized by g_lock): write the slot, publish head.
static void PushEdge(LONG button, LONG pressed) {
    const LONG pos = shared->edge_head;
    const int slot = pos % kEdgeRing;
    shared->edges[slot].button = button;
    shared->edges[slot].pressed = pressed;
    MemoryBarrier();
    shared->edge_head = pos + 1;
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

// Read-only observation: the real call runs first and its result/last-error are
// returned unchanged. We only read the completed output buffer for the carrier.
static NTSTATUS NTAPI HookIoctl(HANDLE file, HANDLE event, PIO_APC_ROUTINE apc,
    PVOID context, PIO_STATUS_BLOCK iosb, ULONG code, PVOID input, ULONG input_size,
    PVOID output, ULONG output_size) {
    const NTSTATUS result = real_ioctl(file, event, apc, context, iosb, code,
        input, input_size, output, output_size);
    const DWORD saved = GetLastError();
    if (result == 0 && code == kReadCharacteristicIoctl && output && output_size == 9 &&
        shared && InterlockedCompareExchange(&shared->state, 0, 0) == HS_Capturing) {
        __try { ProcessReport(reinterpret_cast<const unsigned char*>(output), output_size); }
        __except (EXCEPTION_EXECUTE_HANDLER) {}
    }
    SetLastError(saved);
    return result;
}

static LONG ChangeHook(bool install) {
    HANDLE snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    if (snapshot == INVALID_HANDLE_VALUE) return static_cast<LONG>(GetLastError());
    std::vector<HANDLE> threads;
    THREADENTRY32 entry{sizeof(entry)};
    LONG result = NO_ERROR;
    if (!Thread32First(snapshot, &entry)) result = static_cast<LONG>(GetLastError());
    else do {
        if (entry.th32OwnerProcessID != GetCurrentProcessId() ||
            entry.th32ThreadID == GetCurrentThreadId()) continue;
        HANDLE thread = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT |
            THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION, FALSE, entry.th32ThreadID);
        if (!thread) {
            const DWORD error = GetLastError();
            if (error == ERROR_INVALID_PARAMETER) continue;
            result = static_cast<LONG>(error);
            break;
        }
        threads.push_back(thread);
    } while (Thread32Next(snapshot, &entry));
    CloseHandle(snapshot);
    if (result == NO_ERROR) {
        result = DetourTransactionBegin();
        if (result == NO_ERROR) {
            result = install
                ? DetourAttach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl)
                : DetourDetach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl);
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
    if (!mapping) return GetLastError();
    shared = static_cast<HookShared*>(MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE,
        0, 0, sizeof(HookShared)));
    CloseHandle(mapping);
    if (!shared) return GetLastError();
    if (shared->magic != kHookMagic) return ERROR_INVALID_DATA;
    InterlockedExchange(&shared->state, HS_Starting);
    // Pin: detaching a hook is not proof every thread has left the callback.
    HMODULE pinned = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_PIN,
            reinterpret_cast<LPCWSTR>(&SayAllHookStart), &pinned)) {
        shared->error = static_cast<LONG>(GetLastError());
        InterlockedExchange(&shared->state, HS_Failed);
        return static_cast<DWORD>(shared->error);
    }
    real_ioctl = reinterpret_cast<IoControl>(GetProcAddress(GetModuleHandleW(L"ntdll.dll"),
        "NtDeviceIoControlFile"));
    LONG result = real_ioctl ? ChangeHook(true) : ERROR_PROC_NOT_FOUND;
    if (result != NO_ERROR) {
        shared->error = result;
        InterlockedExchange(&shared->state, HS_Failed);
        return static_cast<DWORD>(result);
    }
    InterlockedExchange(&shared->state, HS_Capturing);
    // App-controlled lifetime: run until it requests teardown (method 3 unhooks
    // on remote sleep/disconnect). No fixed timer; the host is pinned regardless.
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
    InterlockedExchange(&shared->state, result == NO_ERROR ? HS_Stopped : HS_Failed);
    started = 0;  // allow a later re-arm into the same pinned DLL (method 3 wake)
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
