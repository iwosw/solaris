"""Acquire real X11 input focus for this run's mapped native window."""
from __future__ import annotations

import ctypes as c
import ctypes.util
import selectors
import time


def acquire(display: str, pid: int, timeout: float) -> dict:
    lib = ctypes.util.find_library("X11")
    if not lib:
        raise RuntimeError("native window focus requires libX11")
    x = c.CDLL(lib)
    pointer, window = c.c_void_p, c.c_ulong
    signatures = {
        "XOpenDisplay": (pointer, [c.c_char_p]),
        "XDefaultRootWindow": (window, [pointer]),
        "XConnectionNumber": (c.c_int, [pointer]),
        "XInternAtom": (window, [pointer, c.c_char_p, c.c_int]),
        "XSelectInput": (c.c_int, [pointer, window, c.c_long]),
        "XQueryTree": (c.c_int, [pointer, window, c.POINTER(window), c.POINTER(window), c.POINTER(c.POINTER(window)), c.POINTER(c.c_uint)]),
        "XGetWindowProperty": (c.c_int, [pointer, window, window, c.c_long, c.c_long, c.c_int, window, c.POINTER(window), c.POINTER(c.c_int), c.POINTER(window), c.POINTER(window), c.POINTER(pointer)]),
        "XGetWindowAttributes": (c.c_int, [pointer, window, pointer]),
        "XFree": (c.c_int, [pointer]),
        "XPending": (c.c_int, [pointer]),
        "XNextEvent": (c.c_int, [pointer, pointer]),
        "XRaiseWindow": (c.c_int, [pointer, window]),
        "XSetInputFocus": (c.c_int, [pointer, window, c.c_int, window]),
        "XGetInputFocus": (c.c_int, [pointer, c.POINTER(window), c.POINTER(c.c_int)]),
        "XSync": (c.c_int, [pointer, c.c_int]),
        "XCloseDisplay": (c.c_int, [pointer]),
        "XSetErrorHandler": (pointer, [pointer]),
    }
    for name, (restype, argtypes) in signatures.items():
        function = getattr(x, name)
        function.restype, function.argtypes = restype, argtypes

    class Attributes(c.Structure):
        _fields_ = [(name, kind) for name, kind in (
            ("x", c.c_int), ("y", c.c_int), ("width", c.c_int), ("height", c.c_int),
            ("border_width", c.c_int), ("depth", c.c_int), ("visual", pointer),
            ("root", window), ("class_", c.c_int), ("bit_gravity", c.c_int),
            ("win_gravity", c.c_int), ("backing_store", c.c_int),
            ("backing_planes", window), ("backing_pixel", window),
            ("save_under", c.c_int), ("colormap", window), ("map_installed", c.c_int),
            ("map_state", c.c_int), ("all_event_masks", c.c_long),
            ("your_event_mask", c.c_long), ("do_not_propagate_mask", c.c_long),
            ("override_redirect", c.c_int), ("screen", pointer))]

    connection = x.XOpenDisplay(display.encode())
    if not connection:
        raise RuntimeError(f"cannot open owned X display {display}")
    class Error(c.Structure):
        _fields_ = [("type", c.c_int), ("display", pointer), ("resourceid", window),
                    ("serial", window), ("error_code", c.c_ubyte),
                    ("request_code", c.c_ubyte), ("minor_code", c.c_ubyte)]

    errors = []
    @c.CFUNCTYPE(c.c_int, pointer, pointer)
    def on_error(_display, event):
        error = c.cast(event, c.POINTER(Error)).contents
        errors.append((error.error_code, error.request_code, error.resourceid))
        return 0

    previous_handler = x.XSetErrorHandler(c.cast(on_error, pointer))

    def sync():
        x.XSync(connection, 0)
        if errors:
            raise RuntimeError(f"X11 focus protocol error: {errors[0]}")

    try:
        root = x.XDefaultRootWindow(connection)
        atom = x.XInternAtom(connection, b"_NET_WM_PID", 0)
        # Subscribe before the initial tree scan; MapNotify wakes a missing window.
        x.XSelectInput(connection, root, 1 << 19)  # SubstructureNotifyMask
        sync()

        def find(parent):
            root_return, parent_return, count = window(), window(), c.c_uint()
            children = c.POINTER(window)()
            if not x.XQueryTree(connection, parent, c.byref(root_return), c.byref(parent_return), c.byref(children), c.byref(count)):
                return None
            try:
                for index in range(count.value):
                    child = children[index]
                    actual, format_, items, remaining, data = window(), c.c_int(), window(), window(), pointer()
                    x.XGetWindowProperty(connection, child, atom, 0, 1, 0, 6, c.byref(actual), c.byref(format_), c.byref(items), c.byref(remaining), c.byref(data))  # XA_CARDINAL
                    matches = False
                    if data:
                        try:
                            matches = format_.value == 32 and items.value == 1 and c.cast(data, c.POINTER(window))[0] == pid
                        finally:
                            x.XFree(data)
                    attributes = Attributes()
                    if matches and x.XGetWindowAttributes(connection, child, c.byref(attributes)) and attributes.map_state == 2:
                        return child
                    nested = find(child)
                    if nested:
                        return nested
                return None
            finally:
                if children:
                    x.XFree(children)

        deadline = time.monotonic() + timeout
        with selectors.DefaultSelector() as selector:
            selector.register(x.XConnectionNumber(connection), selectors.EVENT_READ)
            while True:
                target = find(root)
                sync()
                if target:
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise RuntimeError("native process did not map its X11 window before focus deadline")
                # Synchronous Xlib queries can buffer MapNotify while draining
                # the socket. Never block while that internal queue is nonempty.
                if not x.XPending(connection) and not selector.select(remaining):
                    raise RuntimeError("native process did not map its X11 window before focus deadline")
                event = c.create_string_buffer(24 * c.sizeof(c.c_long))
                while x.XPending(connection):
                    x.XNextEvent(connection, event)
        x.XRaiseWindow(connection, target)
        x.XSetInputFocus(connection, target, 2, 0)  # RevertToParent, CurrentTime
        sync()
        focused, revert = window(), c.c_int()
        x.XGetInputFocus(connection, c.byref(focused), c.byref(revert))
        if focused.value != target:
            raise RuntimeError("X server did not grant the native window input focus")
        return {"mechanism": "XRaiseWindow+XSetInputFocus", "display": display, "pid": pid,
                "window_id": target, "x_server_focus_verified": True}
    finally:
        x.XCloseDisplay(connection)
        x.XSetErrorHandler(previous_handler)
