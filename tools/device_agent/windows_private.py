"""Create a new operator directory/files with an explicit protected Windows DACL.

No shell, global ACL change, inherited broad access or permission repair is used.
Only the current process user and SYSTEM receive access to the new objects.
"""
import ctypes
from ctypes import wintypes
import os


class SECURITY_ATTRIBUTES(ctypes.Structure):
    _fields_ = [('nLength', wintypes.DWORD), ('lpSecurityDescriptor', wintypes.LPVOID),
                ('bInheritHandle', wintypes.BOOL)]


def private_descriptor(directory):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    advapi = ctypes.WinDLL('advapi32', use_last_error=True)
    kernel.GetCurrentProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.LocalFree.argtypes = [wintypes.HLOCAL]
    kernel.LocalFree.restype = wintypes.HLOCAL
    advapi.OpenProcessToken.argtypes = [wintypes.HANDLE, wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE)]
    advapi.OpenProcessToken.restype = wintypes.BOOL
    advapi.GetTokenInformation.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID,
                                          wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    advapi.GetTokenInformation.restype = wintypes.BOOL
    advapi.ConvertSidToStringSidW.argtypes = [wintypes.LPVOID, ctypes.POINTER(wintypes.LPWSTR)]
    advapi.ConvertSidToStringSidW.restype = wintypes.BOOL
    advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD,
                                                                           ctypes.POINTER(wintypes.LPVOID),
                                                                           ctypes.POINTER(wintypes.DWORD)]
    advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW.restype = wintypes.BOOL
    handle = wintypes.HANDLE()
    if not advapi.OpenProcessToken(kernel.GetCurrentProcess(), 0x0008, ctypes.byref(handle)):
        raise OSError('Cannot obtain private operator identity')
    sid_string = wintypes.LPWSTR()
    try:
        size = wintypes.DWORD()
        advapi.GetTokenInformation(handle, 1, None, 0, ctypes.byref(size))
        if not 0 < size.value < 65536:
            raise OSError('Invalid operator identity buffer')
        buffer = ctypes.create_string_buffer(size.value)
        if not advapi.GetTokenInformation(handle, 1, buffer, size.value, ctypes.byref(size)):
            raise OSError('Cannot read private operator identity')
        sid = ctypes.cast(buffer, ctypes.POINTER(wintypes.LPVOID))[0]
        if not advapi.ConvertSidToStringSidW(sid, ctypes.byref(sid_string)):
            raise OSError('Cannot encode private operator identity')
        flags = 'OICI' if directory else ''
        sddl = 'O:' + sid_string.value + 'D:P(A;' + flags + ';FA;;;' + sid_string.value + ')(A;' + flags + ';FA;;;SY)'
        descriptor = wintypes.LPVOID()
        if not advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, ctypes.byref(descriptor), None):
            raise OSError('Cannot create private security descriptor')
        return kernel, descriptor
    finally:
        if sid_string:
            kernel.LocalFree(ctypes.cast(sid_string, wintypes.HLOCAL))
        kernel.CloseHandle(handle)


def create_directory(path):
    kernel, descriptor = private_descriptor(True)
    kernel.CreateDirectoryW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(SECURITY_ATTRIBUTES)]
    kernel.CreateDirectoryW.restype = wintypes.BOOL
    attributes = SECURITY_ATTRIBUTES(ctypes.sizeof(SECURITY_ATTRIBUTES), descriptor, False)
    try:
        if not kernel.CreateDirectoryW(str(path), ctypes.byref(attributes)):
            raise OSError('Cannot create a new private operator directory')
    finally:
        kernel.LocalFree(descriptor)


def create_file(path, data):
    import msvcrt
    kernel, descriptor = private_descriptor(False)
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                  ctypes.POINTER(SECURITY_ATTRIBUTES), wintypes.DWORD,
                                  wintypes.DWORD, wintypes.HANDLE]
    kernel.CreateFileW.restype = wintypes.HANDLE
    attributes = SECURITY_ATTRIBUTES(ctypes.sizeof(SECURITY_ATTRIBUTES), descriptor, False)
    handle = None
    try:
        handle = kernel.CreateFileW(str(path), 0x40000000, 0, ctypes.byref(attributes), 1,
                                   0x00200080, None)
        if handle == ctypes.c_void_p(-1).value:
            handle = None
            raise OSError('Cannot create a new private operator file')
        fd = msvcrt.open_osfhandle(handle, os.O_WRONLY | os.O_BINARY)
        handle = None  # The retained Python fd now owns the Windows handle.
        try:
            view = memoryview(data)
            while view:
                written = os.write(fd, view)
                if written <= 0:
                    raise OSError('Incomplete private file write')
                view = view[written:]
            os.fsync(fd)
        finally:
            os.close(fd)
    finally:
        if handle is not None:
            kernel.CloseHandle(handle)
        kernel.LocalFree(descriptor)
