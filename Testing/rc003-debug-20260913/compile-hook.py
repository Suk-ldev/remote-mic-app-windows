import os
import subprocess
import sys

msvc = r'C:\BuildTools\VC\Tools\MSVC\14.44.35207'
sdk = r'C:\Program Files (x86)\Windows Kits\10'
sdkv = '10.0.26100.0'
root = r'D:\Documents\Projects\remote-mic-app-windows'
detours = root + r'\target\rc003-native-tap\reference\Detours-adb07604aa56508448b95bf037c2a6d0d3b6831a'

env = os.environ.copy()
env['Path'] = ';'.join([
    msvc + r'\bin\Hostx64\x64',
    sdk + '\\bin\\' + sdkv + r'\x64',
    env.get('Path', ''),
])
env['INCLUDE'] = ';'.join([
    msvc + r'\include',
    sdk + r'\Include\\' + sdkv + r'\um',
    sdk + r'\Include\\' + sdkv + r'\shared',
    sdk + r'\Include\\' + sdkv + r'\ucrt',
    detours + r'\include',
])
env['LIB'] = ';'.join([
    msvc + r'\lib\x64',
    sdk + r'\Lib\\' + sdkv + r'\um\x64',
    sdk + r'\Lib\\' + sdkv + r'\ucrt\x64',
])

cl_exe = msvc + r'\bin\Hostx64\x64\cl.exe'

jobs = [
    ([cl_exe, '/nologo', '/W4', '/WX', '/O2', '/MT', '/EHsc', '/std:c++17', '/guard:cf',
      '/I', root + r'\native\rc003-hook', root + r'\native\rc003-hook\inject.cpp',
      'advapi32.lib', 'cfgmgr32.lib', 'shell32.lib', 'ole32.lib',
      '/link', '/OUT:sayall-rc003-inject.exe', '/DYNAMICBASE', '/NXCOMPAT'], 'inject'),
    ([cl_exe, '/nologo', '/W4', '/WX', '/O2', '/MT', '/EHsc', '/std:c++17', '/guard:cf',
      '/I', detours + r'\include', '/I', root + r'\native\rc003-hook',
      '/LD', root + r'\native\rc003-hook\hook.cpp', detours + r'\lib.X64\detours.lib',
      '/link', '/OUT:sayall-rc003-hook.dll', '/DYNAMICBASE', '/NXCOMPAT'], 'hook'),
]

for cmd, name in jobs:
    r = subprocess.run(cmd, cwd=root + r'\target\rc003-hook', env=env,
                       capture_output=True)
    out = (r.stdout or b'') + (r.stderr or b'')
    text = out.decode('gbk', errors='replace')
    print(name, 'rc=', r.returncode)
    tail = text.strip().splitlines()
    for line in tail[-6:]:
        print('  ', line)
    if r.returncode != 0:
        sys.exit(1)
print('BUILD OK')
