import io

p = "native/rc003-tap-probe/probe.cpp"
s = io.open(p, encoding="utf-8").read()
marker = "static void ParserTests()"
idx = s.index(marker)
head = s[:idx]

new_tail = r'''static void SelfTest(ProbeCounters* counters) {
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

// Print candidate rows (small successful buffers or async-pending calls -- the
// shapes a HID report takes). `full` prints every non-empty row for the summary.
static void DumpTable(ProbeCounters* counters, bool full) {
    std::printf("stage=totals ioctl=%ld read=%ld tags=%ld overflow=%ld\n",
        counters->total_ioctl, counters->total_read, counters->table_len, counters->overflow);
    for (int i = 0; i < kMaxTags; ++i) {
        const LONG tag = counters->table[i].tag;
        if (tag == 0) continue;
        const auto& e = counters->table[i];
        const bool candidate = e.small > 0 || e.pending > 0;
        if (!full && !candidate) continue;
        if (e.count == 0) continue;
        wchar_t label[16];
        if (static_cast<DWORD>(tag) == kTagReadFile) swprintf_s(label, L"NtReadFile");
        else swprintf_s(label, L"0x%08lX", static_cast<DWORD>(tag));
        std::wprintf(L"row tag=%ls count=%ld success=%ld pending=%ld small=%ld min=%ld max=%ld",
            label, e.count, e.success, e.pending, e.small, e.min_len, e.max_len);
        if (e.sample_len > 0) {
            std::wprintf(L" sample=[");
            for (LONG k = 0; k < e.sample_len; ++k)
                std::wprintf(L"%02X%ls", e.sample[k], k + 1 < e.sample_len ? L" " : L"");
            std::wprintf(L"]");
        }
        std::wprintf(L"\n");
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
            Assert(RemoteModule(pid, L"sayall-rc003-tap.dll") == 0, "probe_not_previously_loaded");
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
            LoadRemote(process.value, pid, StageDll(source));
            remote_start = RemoteModule(pid, L"sayall-rc003-tap.dll") +
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
'''

io.open(p, "w", encoding="utf-8", newline="\n").write(head + new_tail)
print("probe.cpp rewritten, total %d lines" % (head + new_tail).count("\n"))
