#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <tlhelp32.h>
#include <detours.h>
#include <vector>
#include "protocol.h"

using IoControl = NTSTATUS(NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID,
    PIO_STATUS_BLOCK, ULONG, PVOID, ULONG, PVOID, ULONG);
using ReadFile_t = NTSTATUS(NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID,
    PIO_STATUS_BLOCK, PVOID, ULONG, PLARGE_INTEGER, PULONG);

static IoControl real_ioctl = nullptr;
static ReadFile_t real_read = nullptr;
static ProbeCounters* counters = nullptr;
static volatile LONG started = 0;

static ULONG SafeInformation(PIO_STATUS_BLOCK iosb) {
    if (!iosb) return 0;
    __try { return static_cast<ULONG>(iosb->Information); }
    __except (EXCEPTION_EXECUTE_HANDLER) { return 0; }
}

static bool Active() {
    return counters && InterlockedCompareExchange(&counters->state, 0, 0) == Capturing;
}

// Read-only observation. The real call always runs first and its result/last-error
// are returned unchanged; we never modify, block, or retain caller buffers.
static NTSTATUS NTAPI HookIoctl(HANDLE file, HANDLE event, PIO_APC_ROUTINE apc,
    PVOID context, PIO_STATUS_BLOCK iosb, ULONG code, PVOID input, ULONG input_size,
    PVOID output, ULONG output_size) {
    const NTSTATUS result = real_ioctl(file, event, apc, context, iosb, code,
        input, input_size, output, output_size);
    const DWORD saved = GetLastError();
    if (Active()) {
        InterlockedIncrement(&counters->total_ioctl);
        // For these BthLEEnum GATT-read IOCTLs the payload sits in the caller's
        // OUTPUT buffer sized by output_size; iosb->Information reads back 0 here,
        // so key the length on output_size (matches ZSTDJan's Frida tap).
        const ULONG len = result == 0 ? output_size : 0;
        ProbeRecord(counters, code, result, len, output);
        if (result == 0 && code == kReadCharacteristicIoctl && output && output_size >= 1)
            ProbePushRing(counters, output, output_size);
    }
    SetLastError(saved);
    return result;
}

static NTSTATUS NTAPI HookRead(HANDLE file, HANDLE event, PIO_APC_ROUTINE apc,
    PVOID context, PIO_STATUS_BLOCK iosb, PVOID buffer, ULONG length,
    PLARGE_INTEGER offset, PULONG key) {
    const NTSTATUS result = real_read(file, event, apc, context, iosb, buffer, length, offset, key);
    const DWORD saved = GetLastError();
    if (Active()) {
        InterlockedIncrement(&counters->total_read);
        const ULONG len = result == 0 ? SafeInformation(iosb) : 0;
        ProbeRecord(counters, kTagReadFile, result, len, buffer);
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
    // Collect before beginning a transaction: no C++ heap allocation while peer
    // threads may be suspended holding allocator locks.
    if (!Thread32First(snapshot, &entry)) result = static_cast<LONG>(GetLastError());
    else do {
        if (entry.th32OwnerProcessID != GetCurrentProcessId() ||
            entry.th32ThreadID == GetCurrentThreadId()) continue;
        HANDLE thread = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT |
            THREAD_SET_CONTEXT | THREAD_QUERY_INFORMATION, FALSE, entry.th32ThreadID);
        if (!thread) {
            const DWORD error = GetLastError();
            if (error == ERROR_INVALID_PARAMETER) continue; // exited before open
            result = static_cast<LONG>(error);
            break;
        }
        threads.push_back(thread);
    } while (Thread32Next(snapshot, &entry));
    CloseHandle(snapshot);
    if (result == NO_ERROR) {
        result = DetourTransactionBegin();
        if (result == NO_ERROR) {
            if (result == NO_ERROR) result = install
                ? DetourAttach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl)
                : DetourDetach(reinterpret_cast<PVOID*>(&real_ioctl), HookIoctl);
            if (result == NO_ERROR) result = install
                ? DetourAttach(reinterpret_cast<PVOID*>(&real_read), HookRead)
                : DetourDetach(reinterpret_cast<PVOID*>(&real_read), HookRead);
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

extern "C" __declspec(dllexport) DWORD WINAPI SayAllProbeStart(void*);

static DWORD RunProbe() {
    if (InterlockedCompareExchange(&started, 1, 0)) return ERROR_ALREADY_EXISTS;
    wchar_t name[96];
    ProbeMappingName(name, GetCurrentProcessId());
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, name);
    if (!mapping) return GetLastError();
    counters = static_cast<ProbeCounters*>(MapViewOfFile(mapping, FILE_MAP_READ | FILE_MAP_WRITE,
        0, 0, sizeof(ProbeCounters)));
    CloseHandle(mapping);
    if (!counters) return GetLastError();
    if (counters->magic != kProbeMagic) return ERROR_INVALID_DATA;
    InterlockedExchange(&counters->state, Starting);
    // Pin the diagnostic DLL. Detaching a hook is not proof that every thread
    // has left its callback. Physical unloading is intentionally not attempted.
    HMODULE pinned = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
            GET_MODULE_HANDLE_EX_FLAG_PIN,
            reinterpret_cast<LPCWSTR>(&SayAllProbeStart), &pinned)) {
        counters->error = static_cast<LONG>(GetLastError());
        InterlockedExchange(&counters->state, Failed);
        return static_cast<DWORD>(counters->error);
    }
    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    real_ioctl = reinterpret_cast<IoControl>(GetProcAddress(ntdll, "NtDeviceIoControlFile"));
    real_read = reinterpret_cast<ReadFile_t>(GetProcAddress(ntdll, "NtReadFile"));
    LONG result = (real_ioctl && real_read) ? ChangeHook(true) : ERROR_PROC_NOT_FOUND;
    if (result != NO_ERROR) {
        counters->error = result;
        InterlockedExchange(&counters->state, Failed);
        return static_cast<DWORD>(result);
    }
    InterlockedExchange(&counters->state, Capturing);
    const ULONGLONG start = GetTickCount64();
    while (!InterlockedCompareExchange(&counters->stop, 0, 0) &&
        GetTickCount64() - start < kProbeDurationMs) Sleep(25);
    InterlockedExchange(&counters->state, Stopping);
    result = ChangeHook(false);
    counters->error = result;
    InterlockedExchange(&counters->state, result == NO_ERROR ? Stopped : Failed);
    // Mapping stays alive with the pinned DLL, including the detach-failure path.
    return static_cast<DWORD>(result);
}

extern "C" __declspec(dllexport) DWORD WINAPI SayAllProbeStart(void*) {
    try { return RunProbe(); }
    catch (...) {
        if (counters) {
            counters->error = ERROR_NOT_ENOUGH_MEMORY;
            InterlockedExchange(&counters->state, Failed);
        }
        return ERROR_NOT_ENOUGH_MEMORY;
    }
}

BOOL WINAPI DllMain(HINSTANCE, DWORD, LPVOID) { return TRUE; }
