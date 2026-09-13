// SayAll RC003 hook injector/streamer.
//
// Reuses the injection path proven by native/rc003-tap-probe (see
// docs/investigations/2026-09-13-rc003-wudfhost-tap-confirmed.md): find the
// WUDFHost hosting the remote's HID-over-GATT stack, verify its identity, stage
// the hook DLL under an ACL that lets LOCAL SERVICE map it, inject via
// LoadLibraryW + a remote thread on SayAllHookStart, then stream Back/Volume
// button EDGES to stdout as newline-delimited "<button> <0|1>" until the parent
// closes our stdin (clean teardown) or the host disappears.
//
// Must run elevated (Administrator). The SayAll app spawns this, keeps it as a
// child, and closes its stdin to request an unhook. Only button id + up/down is
// emitted; no raw HID payload or device identity ever leaves this process.
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
#include "hook_protocol.h"

struct Handle {
    HANDLE value = nullptr;
    explicit Handle(HANDLE h = nullptr) : value(h) {}
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};

static void Emit(const char* stage, const char* result, DWORD win32) {
    // Diagnostics go to stderr so stdout stays a clean edge stream.
    std::fprintf(stderr, "stage=%s result=%s win32=%lu\n", stage, result, win32);
    std::fflush(stderr);
}
static void Check(bool ok, const char* stage) {
    if (!ok) { const DWORD e = GetLastError(); Emit(stage, "failed", e); throw std::runtime_error(stage); }
}
static void Assert(bool ok, const char* stage) {
    if (!ok) SetLastError(ERROR_INVALID_DATA);
    Check(ok, stage);
}

static std::vector<std::wstring> Subkeys(HKEY key) {
    std::vector<std::wstring> names;
    for (DWORD i = 0;; ++i) {
        wchar_t name[512]; DWORD length = 512;
        const LSTATUS r = RegEnumKeyExW(key, i, name, &length, nullptr, nullptr, nullptr, nullptr);
        if (r == ERROR_NO_MORE_ITEMS) break;
        if (r != ERROR_SUCCESS) { SetLastError(static_cast<DWORD>(r)); Check(false, "enumerate_devices"); }
        names.emplace_back(name, length);
    }
    return names;
}

static DWORD FindHost() {
    constexpr auto base = L"SYSTEM\\CurrentControlSet\\Enum\\BTHLEDevice";
    HKEY root = nullptr;
    Assert(RegOpenKeyExW(HKEY_LOCAL_MACHINE, base, 0, KEY_READ, &root) == ERROR_SUCCESS, "open_device_registry");
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
            if (CM_Locate_DevNodeW(&node, device_id.data(), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS) continue;
            ULONG status = 0, problem = 0;
            if (CM_Get_DevNode_Status(&status, &problem, node, 0) != CR_SUCCESS ||
                !(status & DN_STARTED) || (status & DN_HAS_PROBLEM) || problem) continue;
            const std::wstring diag = path + L"\\" + instance + L"\\Device Parameters\\WUDFDiagnosticInfo";
            ULONGLONG raw = 0; DWORD size = sizeof(raw);
            const LSTATUS hp = RegGetValueW(HKEY_LOCAL_MACHINE, diag.c_str(), L"HostPid",
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
    return selected;
}

static uintptr_t RemoteModule(DWORD pid, const wchar_t* basename) {
    Handle snap(CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid));
    Check(snap.value != INVALID_HANDLE_VALUE, "module_snapshot");
    MODULEENTRY32W entry{sizeof(entry)};
    Check(Module32FirstW(snap.value, &entry) != FALSE, "module_first");
    do {
        if (_wcsicmp(entry.szModule, basename) == 0) return reinterpret_cast<uintptr_t>(entry.modBaseAddr);
    } while (Module32NextW(snap.value, &entry));
    return 0;
}

static void Inspect(HANDLE process) {
    wchar_t actual[32768]; DWORD length = 32768;
    Check(QueryFullProcessImageNameW(process, 0, actual, &length) != FALSE, "process_image");
    wchar_t system[MAX_PATH]; Check(GetSystemDirectoryW(system, MAX_PATH) != 0, "system_directory");
    Assert(_wcsicmp(actual, (std::wstring(system) + L"\\WUDFHost.exe").c_str()) == 0, "system_wudfhost_identity");
    PROCESS_MITIGATION_BINARY_SIGNATURE_POLICY sig{};
    PROCESS_MITIGATION_DYNAMIC_CODE_POLICY dyn{};
    Check(GetProcessMitigationPolicy(process, ProcessSignaturePolicy, &sig, sizeof(sig)) != FALSE, "signature_policy_query");
    Check(GetProcessMitigationPolicy(process, ProcessDynamicCodePolicy, &dyn, sizeof(dyn)) != FALSE, "dynamic_code_policy_query");
    Assert(!(sig.MicrosoftSignedOnly || sig.StoreSignedOnly || sig.MitigationOptIn), "signature_policy_allows_hook");
    Assert(!dyn.ProhibitDynamicCode, "dynamic_code_policy_allows_hook");
    const DWORD host = GetProcessId(process);
    const bool hid_umdf =
        RemoteModule(host, L"microsoft.bluetooth.profiles.hidovergatt.dll") != 0 ||
        RemoteModule(host, L"mshidumdf.dll") != 0;
    Assert(hid_umdf, "hid_driver_loaded");
}

static std::wstring SourceDll() {
    wchar_t path[32768];
    const DWORD size = GetModuleFileNameW(nullptr, path, 32768);
    Check(size > 0 && size < 32768, "executable_path");
    std::wstring result(path);
    return result.substr(0, result.find_last_of(L"\\/") + 1) + L"sayall-rc003-hook.dll";
}

static std::wstring Basename(const std::wstring& path) {
    const size_t pos = path.find_last_of(L"\\/");
    return pos == std::wstring::npos ? path : path.substr(pos + 1);
}

// Stage the hook DLL into a fresh, ACL-locked ProgramData directory. SYSTEM and
// Administrators get full control; LOCAL SERVICE (the WUDFHost) gets read+execute
// only so it can map the DLL but never tamper with it; ordinary users get nothing.
// A unique DLL basename per run avoids colliding with a prior (detached) DLL that
// stays pinned in the shared device-pool host.
static std::wstring StageDll(const std::wstring& source) {
    PSECURITY_DESCRIPTOR sd = nullptr;
    Check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
        L"D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;LS)", SDDL_REVISION_1, &sd, nullptr) != FALSE,
        "runtime_acl");
    SECURITY_ATTRIBUTES attrs{sizeof(attrs), sd, FALSE};
    PWSTR program_data = nullptr;
    Assert(SUCCEEDED(SHGetKnownFolderPath(FOLDERID_ProgramData, 0, nullptr, &program_data)), "program_data");
    GUID id{}; Assert(SUCCEEDED(CoCreateGuid(&id)), "runtime_nonce");
    wchar_t guid[40]; StringFromGUID2(id, guid, 40);
    const std::wstring dir = std::wstring(program_data) + L"\\SayAll-Rc003-Hook-" + guid;
    CoTaskMemFree(program_data);
    const BOOL made = CreateDirectoryW(dir.c_str(), &attrs);
    const DWORD e = GetLastError();
    if (!made) { LocalFree(sd); SetLastError(e); Check(false, "create_secure_runtime"); }
    const std::wstring dll = dir + L"\\sayall-rc003-hook-" + std::wstring(guid + 1, 8) + L".dll";
    Handle in(CreateFileW(source.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    Handle out(CreateFileW(dll.c_str(), GENERIC_WRITE, 0, &attrs, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr));
    LocalFree(sd);
    Check(in.value != INVALID_HANDLE_VALUE && out.value != INVALID_HANDLE_VALUE, "stage_open");
    char buffer[16384]; DWORD read = 0;
    for (;;) {
        Check(ReadFile(in.value, buffer, sizeof(buffer), &read, nullptr) != FALSE, "stage_read");
        if (!read) break;
        DWORD written = 0;
        Check(WriteFile(out.value, buffer, read, &written, nullptr) && written == read, "stage_write");
    }
    Check(FlushFileBuffers(out.value) != FALSE, "stage_flush");
    return dll;
}

static void LoadRemote(HANDLE process, DWORD pid, const std::wstring& path, const std::wstring& dll_basename) {
    const auto load = GetProcAddress(GetModuleHandleW(L"kernel32.dll"), "LoadLibraryW");
    Assert(load != nullptr, "loader_export");
    HMODULE owner = nullptr;
    Check(GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        reinterpret_cast<LPCWSTR>(load), &owner) != FALSE, "loader_owner");
    wchar_t module_path[32768];
    Check(GetModuleFileNameW(owner, module_path, 32768) != 0, "loader_owner_path");
    const wchar_t* base = wcsrchr(module_path, L'\\');
    Assert(base != nullptr, "loader_owner_basename");
    const uintptr_t remote_owner = RemoteModule(pid, base + 1);
    Assert(remote_owner != 0, "remote_loader_owner");
    const uintptr_t entry = remote_owner + reinterpret_cast<uintptr_t>(load) - reinterpret_cast<uintptr_t>(owner);
    const SIZE_T bytes = (path.size() + 1) * sizeof(wchar_t);
    void* memory = VirtualAllocEx(process, nullptr, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    Check(memory != nullptr, "allocate_dll_path");
    SIZE_T written = 0;
    if (!WriteProcessMemory(process, memory, path.c_str(), bytes, &written) || written != bytes) {
        const DWORD e = GetLastError(); VirtualFreeEx(process, memory, 0, MEM_RELEASE); SetLastError(e); Check(false, "write_dll_path");
    }
    Handle thread(CreateRemoteThread(process, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(entry), memory, 0, nullptr));
    if (!thread.value) { const DWORD e = GetLastError(); VirtualFreeEx(process, memory, 0, MEM_RELEASE); SetLastError(e); Check(false, "load_thread"); }
    Assert(WaitForSingleObject(thread.value, 15000) == WAIT_OBJECT_0, "load_thread_completed");
    VirtualFreeEx(process, memory, 0, MEM_RELEASE);
    Assert(RemoteModule(pid, dll_basename.c_str()) != 0, "dll_loaded");
}

int wmain() {
    setvbuf(stdout, nullptr, _IONBF, 0);
    try {
        const DWORD pid = FindHost();
        Handle process(OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_VM_WRITE |
            PROCESS_VM_OPERATION | PROCESS_CREATE_THREAD, FALSE, pid));
        Check(process.value != nullptr, "open_process");
        Inspect(process.value);

        // Create the shared edge mapping named by the WUDFHost pid; the hook DLL
        // opens the same name. LOCAL SERVICE needs map read+write.
        wchar_t name[96]; HookMappingName(name, pid);
        PSECURITY_DESCRIPTOR sd = nullptr;
        Check(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;LS)", SDDL_REVISION_1, &sd, nullptr) != FALSE, "mapping_acl");
        SECURITY_ATTRIBUTES attrs{sizeof(attrs), sd, FALSE};
        Handle mapping(CreateFileMappingW(INVALID_HANDLE_VALUE, &attrs, PAGE_READWRITE, 0, sizeof(HookShared), name));
        const DWORD map_err = GetLastError(); LocalFree(sd);
        Check(mapping.value != nullptr, "create_mapping");
        auto* shared = static_cast<HookShared*>(MapViewOfFile(mapping.value, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, sizeof(HookShared)));
        Check(shared != nullptr, "map_edges");
        if (map_err != ERROR_ALREADY_EXISTS) { ZeroMemory(shared, sizeof(HookShared)); }
        shared->magic = kHookMagic;
        shared->stop = 0;
        shared->state = HS_Idle;

        const std::wstring staged = StageDll(SourceDll());
        const std::wstring staged_base = Basename(staged);
        LoadRemote(process.value, pid, staged, staged_base);
        // Resolve the hook's entry point in the remote and start its worker.
        HMODULE local = LoadLibraryExW(SourceDll().c_str(), nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        Check(local != nullptr, "local_hook_module");
        const auto start = GetProcAddress(local, "SayAllHookStart");
        Assert(start != nullptr, "start_export");
        const uintptr_t remote_start = RemoteModule(pid, staged_base.c_str()) +
            reinterpret_cast<uintptr_t>(start) - reinterpret_cast<uintptr_t>(local);
        Handle worker(CreateRemoteThread(process.value, nullptr, 0, reinterpret_cast<LPTHREAD_START_ROUTINE>(remote_start), nullptr, 0, nullptr));
        Check(worker.value != nullptr, "start_hook_worker");
        const ULONGLONG begun = GetTickCount64();
        while (shared->state != HS_Capturing && shared->state != HS_Failed &&
            WaitForSingleObject(worker.value, 0) == WAIT_TIMEOUT && GetTickCount64() - begun < 10000) Sleep(20);
        if (shared->state != HS_Capturing) {
            // Localize the failing leg. state/magic separate a handshake failure
            // (worker never mapped our section: state stays 0/Idle, magic still
            // reads back ours) from a Detours failure (state=HS_Failed with
            // hook_error set). worker_exit is RunHook's return value -- the real
            // error code of the silent early-return paths (OpenFileMapping /
            // MapViewOfFile / magic / pin) that never touch shared. A live worker
            // reads back STILL_ACTIVE (259); report that as "running" so a genuine
            // 259 error can't masquerade as a 10s timeout.
            DWORD exit_code = 0;
            const bool got = GetExitCodeThread(worker.value, &exit_code) != FALSE;
            const bool running = got && exit_code == STILL_ACTIVE;
            std::fprintf(stderr,
                "stage=hook_installed result=failed win32=%lu state=%ld magic=0x%08lX hook_error=%ld worker=%s worker_exit=%lu threads_seen=%ld threads_updated=%ld threads_denied=%ld\n",
                running ? 0u : exit_code,
                static_cast<long>(shared->state), static_cast<unsigned long>(shared->magic),
                static_cast<long>(shared->error), running ? "running" : "exited", exit_code,
                static_cast<long>(shared->threads_seen), static_cast<long>(shared->threads_updated),
                static_cast<long>(shared->threads_denied));
            std::fflush(stderr);
            throw std::runtime_error("hook_installed");
        }
        std::fprintf(stderr, "stage=hook_installed result=passed threads_seen=%ld threads_updated=%ld threads_denied=%ld\n",
            static_cast<long>(shared->threads_seen), static_cast<long>(shared->threads_updated),
            static_cast<long>(shared->threads_denied));
        std::fflush(stderr);
        std::printf("ready\n");

        // Stream edges until the parent closes stdin (EOF -> request unhook) or
        // the host process exits. A background thread watches stdin for EOF.
        HANDLE stdin_h = GetStdHandle(STD_INPUT_HANDLE);
        Handle host(OpenProcess(SYNCHRONIZE, FALSE, pid));
        LONG tail = shared->edge_head;  // ignore any edges from before we attached
        for (;;) {
            // Drain new edges.
            const LONG head = shared->edge_head;
            while (tail != head) {
                const HookEdge& e = shared->edges[tail % kEdgeRing];
                const char* b = e.button == HB_Back ? "back" : e.button == HB_VolumeUp ? "volume_up" :
                    e.button == HB_VolumeDown ? "volume_down" : "?";
                std::printf("%s %ld\n", b, e.pressed);
                ++tail;
            }
            // Teardown conditions: parent closed stdin, or host gone.
            char probe = 0; DWORD got = 0;
            if (PeekNamedPipe(stdin_h, &probe, 1, &got, nullptr, nullptr) == FALSE) {
                const DWORD e = GetLastError();
                if (e == ERROR_BROKEN_PIPE || e == ERROR_INVALID_HANDLE) break;  // parent gone
            }
            if (host.value && WaitForSingleObject(host.value, 0) == WAIT_OBJECT_0) break;  // WUDFHost exited
            Sleep(8);
        }
        InterlockedExchange(&shared->stop, 1);
        WaitForSingleObject(worker.value, 5000);
        Emit("stream", "stopped", 0);
        return 0;
    } catch (const std::exception&) {
        return 1;
    }
}
