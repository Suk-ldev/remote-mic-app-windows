#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <winternl.h>
#include <tlhelp32.h>
#include <cfgmgr32.h>
#include <sddl.h>
#include <shlobj.h>
#include <cstdio>
#include <string>
#include <vector>
#include <stdexcept>
#include <cwctype>
#include "protocol.h"

struct Handle {
    HANDLE value = nullptr;
    explicit Handle(HANDLE h = nullptr) : value(h) {}
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};
static void Check(bool ok, const char* stage) {
    if (!ok) {
        const DWORD error = GetLastError();
        std::printf("stage=%s result=failed win32=%lu\n", stage, error);
        throw std::runtime_error(stage);
    }
}
static void Assert(bool ok, const char* stage) {
    if (!ok) SetLastError(ERROR_INVALID_DATA);
    Check(ok, stage);
}

static std::vector<std::wstring> Subkeys(HKEY key) {
    std::vector<std::wstring> names;
    for (DWORD i = 0;; ++i) {
        wchar_t name[512]; DWORD length = 512;
        const LSTATUS result = RegEnumKeyExW(key, i, name, &length, nullptr, nullptr, nullptr, nullptr);
        if (result == ERROR_NO_MORE_ITEMS) break;
        if (result != ERROR_SUCCESS) {
            SetLastError(static_cast<DWORD>(result)); Check(false, "enumerate_devices");
        }
        names.emplace_back(name, length);
    }
    return names;
}

static DWORD FindHost() {
    constexpr auto base = L"SYSTEM\\CurrentControlSet\\Enum\\BTHLEDevice";
    HKEY root = nullptr;
    Assert(RegOpenKeyExW(HKEY_LOCAL_MACHINE, base, 0, KEY_READ, &root) == ERROR_SUCCESS,
        "open_device_registry");
    const auto services = Subkeys(root);
    RegCloseKey(root);
    DWORD selected = 0, target_count = 0;
    std::vector<DWORD> hid_hosts;
    for (const auto& service : services) {
        std::wstring lower = service;
        for (auto& c : lower) c = static_cast<wchar_t>(towlower(c));
        if (lower.find(L"{00001812-") != 0) continue;
        const bool target = lower.find(L"dev_vid&012717_pid&32b8") != std::wstring::npos;
        const std::wstring path = std::wstring(base) + L"\\" + service;
        HKEY key = nullptr;
        if (RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.c_str(), 0, KEY_READ, &key)) continue;
        const auto instances = Subkeys(key);
        RegCloseKey(key);
        for (const auto& instance : instances) {
            std::wstring device_id = L"BTHLEDevice\\" + service + L"\\" + instance;
            DEVINST node = 0;
            if (CM_Locate_DevNodeW(&node, device_id.data(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS)
                continue;
            ULONG status = 0, problem = 0;
            if (CM_Get_DevNode_Status(&status, &problem, node, 0) != CR_SUCCESS ||
                !(status & DN_STARTED) || (status & DN_HAS_PROBLEM) || problem) continue;
            const std::wstring diagnostic = path + L"\\" + instance + L"\\Device Parameters\\WUDFDiagnosticInfo";
            // HostPid is REG_QWORD on current Windows (an earlier assumption of
            // REG_DWORD made RegGetValueW reject it with ERROR_UNSUPPORTED_TYPE=1630).
            // Accept either width; a PID always fits in the low 32 bits.
            ULONGLONG raw = 0; DWORD size = sizeof(raw);
            const LSTATUS hp = RegGetValueW(HKEY_LOCAL_MACHINE, diagnostic.c_str(), L"HostPid",
                    RRF_RT_REG_DWORD | RRF_RT_REG_QWORD, nullptr, &raw, &size);
            const DWORD pid = static_cast<DWORD>(raw);
            if (hp != ERROR_SUCCESS || !pid) continue;
            hid_hosts.push_back(pid);
            if (target) { selected = pid; ++target_count; }
        }
    }
    Assert(target_count == 1, "exactly_one_present_rc003_hid");
    unsigned sharing = 0;
    for (DWORD pid : hid_hosts) if (pid == selected) ++sharing;
    Assert(sharing == 1, "single_ble_hid_in_host");
    std::printf("stage=discovery result=passed pid=%lu present_rc003_hid=1 ble_hid_in_host=1\n", selected);
    return selected;
}

static std::wstring Basename(const std::wstring& path) {
    const size_t pos = path.find_last_of(L"\\/");
    return pos == std::wstring::npos ? path : path.substr(pos + 1);
}

static uintptr_t RemoteModule(DWORD pid, const wchar_t* basename) {
    Handle snapshot(CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid));
    Check(snapshot.value != INVALID_HANDLE_VALUE, "module_snapshot");
    MODULEENTRY32W entry{sizeof(entry)};
    Check(Module32FirstW(snapshot.value, &entry) != FALSE, "module_first");
    do {
        if (_wcsicmp(entry.szModule, basename) == 0)
            return reinterpret_cast<uintptr_t>(entry.modBaseAddr);
    } while (Module32NextW(snapshot.value, &entry));
    return 0;
}

static void Inspect(HANDLE process) {
    wchar_t actual[32768]; DWORD length = 32768;
    Check(QueryFullProcessImageNameW(process, 0, actual, &length) != FALSE, "process_image");
    wchar_t system[MAX_PATH]; Check(GetSystemDirectoryW(system, MAX_PATH) != 0, "system_directory");
    Assert(_wcsicmp(actual, (std::wstring(system) + L"\\WUDFHost.exe").c_str()) == 0,
        "system_wudfhost_identity");
    PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY signature{};
    PROCESS_MITIGATION_DYNAMIC_CODE_POLICY dynamic{};
    Check(GetProcessMitigationPolicy(process, ProcessSignaturePolicy, &signature, sizeof(signature)) != FALSE,
        "signature_policy_query");
    Check(GetProcessMitigationPolicy(process, ProcessDynamicCodePolicy, &dynamic, sizeof(dynamic)) != FALSE,
        "dynamic_code_policy_query");
    std::printf("stage=mitigations signature_flags=%lu dynamic_flags=%lu\n", signature.Flags, dynamic.Flags);
    Assert(!(signature.MicrosoftSignedOnly || signature.StoreSignedOnly || signature.MitigationOptIn),
        "signature_policy_allows_probe");
    Assert(!dynamic.ProhibitDynamicCode, "dynamic_code_policy_allows_probe");
    // The BLE HID device is served by a UMDF profile driver inside this WUDFHost.
    // Current Windows loads microsoft.bluetooth.profiles.hidovergatt.dll for
    // HID-over-GATT; mshidumdf.dll is the generic fallback on other stacks/versions.
    // Presence of either confirms we are about to enter the HID host, not an
    // unrelated WUDFHost -- belt-and-suspenders atop the registry HostPid match.
    const DWORD host = GetProcessId(process);
    const bool hid_umdf =
        RemoteModule(host, L"microsoft.bluetooth.profiles.hidovergatt.dll") != 0 ||
        RemoteModule(host, L"mshidumdf.dll") != 0;
    Assert(hid_umdf, "hid_driver_loaded");
}

static std::wstring DllPath() {
    wchar_t path[32768];
    const DWORD size = GetModuleFileNameW(nullptr, path, 32768);
    Check(size > 0 && size < 32768, "executable_path");
    std::wstring result(path);
    return result.substr(0, result.find_last_of(L"\\/") + 1) + L"sayall-rc003-tap.dll";
}

// Fresh, atomically ACL-protected directory. Never load a LocalSystem DLL from
// a directory writable by ordinary users. No tasks, services or startup entries.
// SYSTEM and Administrators get full control (they alone may write/replace the
// DLL); the BLE HID WUDFHost runs as LOCAL SERVICE, which is granted read+execute
// only (FRFX) so it can map the DLL but never tamper with it. Ordinary users get
// nothing.
static std::wstring StageDll(const std::wstring& source) {
    PSECURITY_DESCRIPTOR descriptor = nullptr;
    Check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
        L"D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;LS)",
        SDDL_REVISION_1, &descriptor, nullptr) != FALSE, "runtime_acl");
    SECURITY_ATTRIBUTES attributes{sizeof(attributes), descriptor, FALSE};
    PWSTR program_data = nullptr;
    Assert(SUCCEEDED(SHGetKnownFolderPath(FOLDERID_ProgramData, 0, nullptr, &program_data)), "program_data");
    GUID id{}; Assert(SUCCEEDED(CoCreateGuid(&id)), "runtime_nonce");
    wchar_t guid[40]; StringFromGUID2(id, guid, 40);
    const std::wstring dir = std::wstring(program_data) + L"\\SayAll-Rc003-Probe-" + guid;
    CoTaskMemFree(program_data);
    const BOOL made = CreateDirectoryW(dir.c_str(), &attributes);
    const DWORD error = GetLastError();
    if (!made) { LocalFree(descriptor); SetLastError(error); Check(false, "create_secure_runtime"); }
    // Unique DLL basename per injection: WUDFHost is a shared UMDF device-pool
    // host that does not recycle on a single device restart, so a prior run's
    // (already-detached) tap DLL stays pinned. A fresh name lets the probe be
    // re-run without recycling the host or colliding on the module list.
    const std::wstring dll = dir + L"\\sayall-rc003-tap-" + std::wstring(guid + 1, 8) + L".dll";
    Handle input(CreateFileW(source.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
        OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    Handle output(CreateFileW(dll.c_str(), GENERIC_WRITE, 0, &attributes,
        CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
    LocalFree(descriptor);
    Check(input.value != INVALID_HANDLE_VALUE && output.value != INVALID_HANDLE_VALUE, "stage_open");
    char buffer[16384]; DWORD read = 0;
    for (;;) {
        Check(ReadFile(input.value, buffer, sizeof(buffer), &read, nullptr) != FALSE, "stage_read");
        if (!read) break;
        DWORD written = 0;
        Check(WriteFile(output.value, buffer, read, &written, nullptr) && written == read, "stage_write");
    }
    Check(FlushFileBuffers(output.value) != FALSE, "stage_flush");
    std::printf("stage=secure_runtime result=passed retained_until_host_exit=1\n");
    // Local-only return path for cleanup after the host exits, not production telemetry.
    std::fwprintf(stderr, L"Local diagnostic DLL: %ls\n", dll.c_str());
    return dll;
}

static void LoadRemote(HANDLE process, DWORD pid, const std::wstring& path, const std::wstring& dll_basename) {
    const auto load = GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW");
    Assert(load != nullptr, "loader_export");
    HMODULE owner = nullptr;
    Check(GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS |
        GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, reinterpret_cast<LPCWSTR>(load), &owner) != FALSE,
        "loader_owner");
    wchar_t module_path[32768];
    Check(GetModuleFileNameW(owner, module_path, 32768) != 0, "loader_owner_path");
    const wchar_t* basename = wcsrchr(module_path, L'\\');
    Assert(basename != nullptr, "loader_owner_basename");
    const uintptr_t remote_owner = RemoteModule(pid, basename + 1);
    Assert(remote_owner != 0, "remote_loader_owner");
    const uintptr_t entry = remote_owner + reinterpret_cast<uintptr_t>(load) - reinterpret_cast<uintptr_t>(owner);
    const SIZE_T bytes = (path.size() + 1) * sizeof(wchar_t);
    void* memory = VirtualAllocEx(process, nullptr, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    Check(memory != nullptr, "allocate_dll_path");
    SIZE_T written = 0;
    if (!WriteProcessMemory(process, memory, path.c_str(), bytes, &written) || written != bytes) {
        const DWORD error = GetLastError(); VirtualFreeEx(process, memory, 0, MEM_RELEASE);
        SetLastError(error); Check(false, "write_dll_path");
    }
    Handle thread(CreateRemoteThread(process, nullptr, 0,
        reinterpret_cast<LPTHREAD_START_ROUTINE>(entry), memory, 0, nullptr));
    if (!thread.value) {
        const DWORD error = GetLastError(); VirtualFreeEx(process, memory, 0, MEM_RELEASE);
        SetLastError(error); Check(false, "load_thread");
    }
    const DWORD waited = WaitForSingleObject(thread.value, 15000);
    // A timed-out loader may still read its argument: keep the allocation, no kill/retry.
    Assert(waited == WAIT_OBJECT_0, "load_thread_completed");
    VirtualFreeEx(process, memory, 0, MEM_RELEASE);
    Assert(RemoteModule(pid, dll_basename.c_str()) != 0, "dll_loaded");
}

static void SelfTest(ProbeCounters* counters) {
    // In-process: issue a known-failing IOCTL and read so the freshly installed
    // hooks record at least one call each, then check the decoder.
    using Io = NTSTATUS(NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID, PIO_STATUS_BLOCK,
        ULONG, PVOID, ULONG, PVOID, ULONG);
    using Rd = NTSTATUS(NTAPI*)(HANDLE, HANDLE, PIO_APC_ROUTINE, PVOID, PIO_STATUS_BLOCK,
        PVOID, ULONG, PLARGE_INTEGER, PULONG);
    HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    const auto io = reinterpret_cast<Io>(GetProcAddress(ntdll, "NtDeviceIoControlFile"));
    const auto rd = reinterpret_cast<Rd>(GetProcAddress(ntdll, "NtReadFile"));
    unsigned char bytes[9]{}; IO_STATUS_BLOCK iosb{};
    io(INVALID_HANDLE_VALUE, nullptr, nullptr, nullptr, &iosb, kReadCharacteristicIoctl,
        nullptr, 0, bytes, sizeof(bytes));
    rd(INVALID_HANDLE_VALUE, nullptr, nullptr, nullptr, &iosb, bytes, sizeof(bytes), nullptr, nullptr);
    Assert(counters->total_ioctl >= 1 && counters->total_read >= 1, "hooks_forwarded_both_calls");
    unsigned mask = 0;
    unsigned char report[9] = {1, 0, 0, 0xf1, 0, 0x80, 0, 0x81, 0};
    Assert(DecodeMissingMask(report, 9, mask) && mask == 7, "decode_three_keys");
    Assert(!DecodeMissingMask(report, 8, mask), "reject_truncation");
    std::printf("stage=self_test result=passed total_ioctl=%ld total_read=%ld\n",
        counters->total_ioctl, counters->total_read);
}

// Print candidate rows (reports successful buffers or async-pending calls -- the
// shapes a HID report takes). `full` prints every non-empty row for the summary.
static void DumpTable(ProbeCounters* counters, bool full) {
    std::printf("stage=totals ioctl=%ld read=%ld tags=%ld overflow=%ld\n",
        counters->total_ioctl, counters->total_read, counters->table_len, counters->overflow);
    for (int i = 0; i < kMaxTags; ++i) {
        const LONG tag = counters->table[i].tag;
        if (tag == 0) continue;
        const auto& e = counters->table[i];
        const bool candidate = e.reports > 0 || e.pending > 0;
        if (!full && !candidate) continue;
        if (e.count == 0) continue;
        wchar_t label[16];
        if (static_cast<DWORD>(tag) == kTagReadFile) swprintf_s(label, L"NtReadFile");
        else swprintf_s(label, L"0x%08lX", static_cast<DWORD>(tag));
        std::wprintf(L"row tag=%ls count=%ld success=%ld pending=%ld reports=%ld min=%ld max=%ld",
            label, e.count, e.success, e.pending, e.reports, e.min_len, e.max_len);
        if (e.sample_len > 0) {
            std::wprintf(L" sample=[");
            for (LONG k = 0; k < e.sample_len; ++k)
                std::wprintf(L"%02X%ls", e.sample[k], k + 1 < e.sample_len ? L" " : L"");
            std::wprintf(L"]");
        }
        std::wprintf(L"\n");
    }
    if (full) {
        // Distinct recent carrier reports: this is the on-wire byte layout for
        // Back / Volume+ / Volume- (and every other key) as delivered to WUDFHost.
        std::printf("ring pushed=%ld\n", counters->ring_head);
        std::wstring seen;
        for (int i = 0; i < kRingSize; ++i) {
            const LONG len = counters->ring[i].len;
            if (len <= 0) continue;
            std::wstring hex;
            wchar_t byte[4];
            for (LONG k = 0; k < len; ++k) {
                swprintf_s(byte, L"%02X", counters->ring[i].data[k]);
                hex += byte;
                if (k + 1 < len) hex += L" ";
            }
            if (seen.find(L"[" + hex + L"]") != std::wstring::npos) continue;  // dedup identical reports
            seen += L"[" + hex + L"]";
            std::wprintf(L"report len=%ld bytes=[%ls]\n", len, hex.c_str());
        }
    }
    fflush(stdout);
}

int wmain(int argc, wchar_t** argv) {
    setvbuf(stdout, nullptr, _IONBF, 0);
    if (argc != 2 || (wcscmp(argv[1], L"--preflight") && wcscmp(argv[1], L"--self-test") &&
        wcscmp(argv[1], L"--discover-rc003"))) {
        std::printf("Usage: sayall-rc003-probe --preflight | --self-test | --discover-rc003\n");
        return 2;
    }
    try {
        const bool self = wcscmp(argv[1], L"--self-test") == 0;
        const bool preflight = wcscmp(argv[1], L"--preflight") == 0;
        const DWORD pid = self ? GetCurrentProcessId() : FindHost();
        const DWORD access = preflight ? PROCESS_QUERY_INFORMATION | PROCESS_VM_READ :
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_CREATE_THREAD;
        Handle process(OpenProcess(access, FALSE, pid));
        Check(process.value != nullptr, "open_process");
        if (!self) Inspect(process.value);
        if (preflight) {
            std::printf("stage=preflight result=passed injection_attempted=0\n"); return 0;
        }
        if (!self) {
            Assert(FindHost() == pid, "host_identity_recheck");
        }
        wchar_t name[96]; ProbeMappingName(name, pid);
        PSECURITY_DESCRIPTOR descriptor = nullptr;
        // SYSTEM/Admins full; LOCAL SERVICE (the WUDFHost the tap runs in) gets
        // map read+write so the injected DLL can publish diagnostic counters.
        Check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;LS)",
            SDDL_REVISION_1, &descriptor, nullptr) != FALSE, "mapping_acl");
        SECURITY_ATTRIBUTES attributes{sizeof(attributes), descriptor, FALSE};
        Handle mapping(CreateFileMappingW(INVALID_HANDLE_VALUE, &attributes, PAGE_READWRITE, 0,
            sizeof(ProbeCounters), name));
        const DWORD created_error = GetLastError(); LocalFree(descriptor);
        Check(mapping.value != nullptr, "create_mapping");
        Assert(created_error != ERROR_ALREADY_EXISTS, "fresh_mapping");
        auto* counters = static_cast<ProbeCounters*>(MapViewOfFile(mapping.value,
            FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, sizeof(ProbeCounters)));
        Check(counters != nullptr, "map_counters");
        counters->magic = kProbeMagic;
        const std::wstring source = DllPath();
        HMODULE local_dll = LoadLibraryExW(source.c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
            LOAD_LIBRARY_SEARCH_SYSTEM32);
        Check(local_dll != nullptr, "local_probe_module");
        const auto start = GetProcAddress(local_dll, "SayAllProbeStart");
        Assert(start != nullptr, "start_export");
        uintptr_t remote_start = reinterpret_cast<uintptr_t>(start);
        if (!self) {
            const std::wstring staged = StageDll(source);
            const std::wstring staged_base = Basename(staged);
            LoadRemote(process.value, pid, staged, staged_base);
            remote_start = RemoteModule(pid, staged_base.c_str()) +
                reinterpret_cast<uintptr_t>(start) - reinterpret_cast<uintptr_t>(local_dll);
        }
        Handle worker(CreateRemoteThread(process.value, nullptr, 0,
            reinterpret_cast<LPTHREAD_START_ROUTINE>(remote_start), nullptr, 0, nullptr));
        Check(worker.value != nullptr, "start_probe_worker");
        const ULONGLONG begun = GetTickCount64();
        while (counters->state != Capturing && counters->state != Failed &&
            WaitForSingleObject(worker.value, 0) == WAIT_TIMEOUT && GetTickCount64() - begun < 10000) Sleep(20);
        Assert(counters->state == Capturing, "hook_installed");
        std::printf("stage=capture state=ready mode=%ls duration_seconds=%u\n",
            self ? L"self" : L"discover", self ? 0 : 90);
        if (self) {
            SelfTest(counters);
            InterlockedExchange(&counters->stop, 1);
        } else {
            // Live discovery: dump candidate rows every 2s so a table row can be
            // correlated with physical button presses, until the worker window ends.
            while (WaitForSingleObject(worker.value, 2000) == WAIT_TIMEOUT &&
                GetTickCount64() - begun < kProbeDurationMs + 15000) {
                std::printf("--- tick t=%llus ---\n", (GetTickCount64() - begun) / 1000);
                DumpTable(counters, false);
            }
            InterlockedExchange(&counters->stop, 1);
        }
        Assert(WaitForSingleObject(worker.value, 5000) == WAIT_OBJECT_0, "worker_stopped");
        std::printf("=== final table ===\n");
        DumpTable(counters, true);
        Assert(counters->state == Stopped && counters->error == 0, "hook_detached");
        std::printf("stage=%s result=passed\n", self ? "self_test" : "discovery");
        return 0;
    } catch (const std::exception&) { return 1; }
}
