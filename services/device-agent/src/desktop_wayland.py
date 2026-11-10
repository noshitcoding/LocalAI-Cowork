"""Owned Wayland portal bridge; no X11, wlroots, uinput, shell or restore fallback.

Operator runtime: system Python3, PyGObject Gio/GLib/Gst/GstVideo and pipewiresrc.
Wire responses: uint32BE JSON header length, bounded JSON, then exactly bytes raw.
Sources: flatpak.github.io/xdg-desktop-portal/docs/ (ScreenCast, RemoteDesktop,
Clipboard, Request, Session); docs.gtk.org/gio/; PipeWire/src/gst/gstpipewiresrc.c.
This helper cannot qualify a physical compositor or grant rights to another user.
"""
import ctypes
from collections import deque
import json
import os
from pathlib import Path
import queue
import re
import select
import signal
import stat
import struct
import sys
import threading
import time
import uuid
from urllib.parse import unquote

MAX_HEADER = 64 * 1024
MAX_TEXT = 1024 * 1024
MAX_COMMAND = 6 * MAX_TEXT + 4096
MAX_WIDTH, MAX_HEIGHT = 7680, 4320
MAX_FRAME = MAX_WIDTH * MAX_HEIGHT * 4
MAX_LAYOUT = MAX_FRAME + 4096 * (MAX_HEIGHT + 1)
PORTAL = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
SCREEN = "org.freedesktop.portal.ScreenCast"
REMOTE = "org.freedesktop.portal.RemoteDesktop"
CLIP = "org.freedesktop.portal.Clipboard"
PROPERTIES = "org.freedesktop.DBus.Properties"
DBUS = "org.freedesktop.DBus"
DBUS_PATH = "/org/freedesktop/DBus"
LOGIN = "org.freedesktop.login1"
LOGIN_PATH = "/org/freedesktop/login1"
LOGIN_SESSION = LOGIN + ".Session"
TEXT_MIMES = ("text/plain;charset=utf-8", "text/plain")


class Refused(Exception):
    def __init__(self, code, fatal=True):
        self.code, self.fatal = code, fatal
        super().__init__(code)


def integer(value, low, high):
    if type(value) is not int or not low <= value <= high:
        raise Refused("invalid_command")
    return value


def parse_command(raw):
    if not raw or len(raw) > MAX_COMMAND or not raw.endswith(b"\n"):
        raise Refused("command_length")
    def unique(items):
        result = {}
        for key, value in items:
            if key in result:
                raise Refused("invalid_command")
            result[key] = value
        return result
    try:
        command = json.loads(raw.decode("utf-8"), object_pairs_hook=unique)
    except (ValueError, UnicodeError, RecursionError):
        raise Refused("invalid_command") from None
    fields = {
        "open": {"op", "control"}, "frame": {"op"}, "close": {"op"},
        "key": {"op", "keysym", "down"},
        "pointer": {"op", "x", "y", "buttons", "previous", "width", "height"},
        "clipboard_read": {"op"}, "clipboard_write": {"op", "text"},
    }
    if not isinstance(command, dict) or type(command.get("op")) is not str:
        raise Refused("invalid_command")
    if set(command) != fields.get(command["op"]):
        raise Refused("invalid_command")
    op = command["op"]
    if op == "open" and type(command["control"]) is not bool:
        raise Refused("invalid_command")
    if op == "key":
        integer(command["keysym"], 1, 0xffffffff)
        if type(command["down"]) is not bool:
            raise Refused("invalid_command")
    if op == "pointer":
        for key in ("x", "y"):
            integer(command[key], 0, 65535)
        for key in ("width", "height"):
            integer(command[key], 1, 65535)
        integer(command["buttons"], 0, 127)
        integer(command["previous"], 0, 7)
    if op == "clipboard_write":
        if type(command["text"]) is not str:
            raise Refused("invalid_command")
        try:
            length = len(command["text"].encode("utf-8"))
        except UnicodeError:
            raise Refused("invalid_command") from None
        if length > MAX_TEXT:
            raise Refused("clipboard_length")
    return command


def response_header(metadata, payload_length=0):
    if type(payload_length) is not int or not 0 <= payload_length <= MAX_FRAME:
        raise Refused("response_length")
    value = dict(metadata, bytes=payload_length)
    raw = json.dumps(value, ensure_ascii=True, separators=(",", ":"), allow_nan=False).encode("ascii")
    if len(raw) > MAX_HEADER:
        raise Refused("response_length")
    return struct.pack(">I", len(raw)) + raw


class LocalEnvironment:
    def __init__(self):
        if sys.platform != "linux" or os.getuid() != os.geteuid():
            raise Refused("local_wayland_required")
        self.uid = os.geteuid()
        runtime = os.environ.get("XDG_RUNTIME_DIR", "")
        display = os.environ.get("WAYLAND_DISPLAY", "")
        if not runtime or not os.path.isabs(runtime) or not display:
            raise Refused("local_wayland_required")
        self.runtime = Path(runtime)
        self.ordinary_directory(self.runtime, private=True)
        path = Path(display) if os.path.isabs(display) else self.runtime / display
        if ".." in path.parts or path.parent != self.runtime:
            raise Refused("local_wayland_required")
        self.wayland = path
        self.wayland_identity = self.socket(path, self.uid)
        address = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
        match = re.fullmatch(r"unix:path=([^,;]+)(?:,guid=([0-9a-fA-F]{32}))?", address)
        if match is None:
            raise Refused("local_bus_required")
        self.bus_path = Path(unquote(match.group(1)))
        if not self.bus_path.is_absolute() or self.bus_path.parent != self.runtime:
            raise Refused("local_bus_required")
        self.bus_identity = self.socket(self.bus_path, self.uid)
        self.bus_address = address
        self.system_path = Path("/run/dbus/system_bus_socket")
        self.system_identity = self.socket(self.system_path, 0)

    def ordinary_directory(self, path, private=False):
        for part in (path, *path.parents):
            record = part.lstat()
            if not stat.S_ISDIR(record.st_mode) or stat.S_ISLNK(record.st_mode):
                raise Refused("local_namespace_changed")
            sticky_root = record.st_uid == 0 and bool(record.st_mode & stat.S_ISVTX)
            if record.st_uid not in (0, self.uid) or record.st_mode & 0o022 and not sticky_root:
                raise Refused("local_namespace_changed")
        record = path.stat()
        if private and (record.st_uid != self.uid or record.st_mode & 0o077):
            raise Refused("local_namespace_changed")

    def socket(self, path, uid):
        self.ordinary_directory(path.parent)
        record = path.lstat()
        if not stat.S_ISSOCK(record.st_mode) or record.st_uid != uid:
            raise Refused("local_bus_required")
        return record.st_dev, record.st_ino, record.st_uid

    def check(self):
        self.ordinary_directory(self.runtime, private=True)
        if self.socket(self.wayland, self.uid) != self.wayland_identity:
            raise Refused("local_namespace_changed")
        if self.socket(self.bus_path, self.uid) != self.bus_identity:
            raise Refused("local_namespace_changed")
        if self.socket(self.system_path, 0) != self.system_identity:
            raise Refused("local_namespace_changed")


def bindings():
    try:
        import gi
        gi.require_version("Gio", "2.0")
        gi.require_version("Gst", "1.0")
        gi.require_version("GstVideo", "1.0")
        from gi.repository import Gio, GLib, Gst, GstVideo
        Gst.init(None)
        return Gio, GLib, Gst, GstVideo
    except (ImportError, ValueError):
        raise Refused("wayland_dependencies_missing") from None


class Portal:
    def __init__(self, stopped, parent):
        self.stopped, self.parent = stopped, parent
        # Establish all cleanup state before imports or the first connection.
        # main owns this object even if construction stops partway through.
        self.Gio = self.GLib = self.Gst = self.GstVideo = None
        self.context = self.environment = None
        self.bus = self.system = None
        self.owner = self.login_owner = self.login_path = None
        self.session = None
        self.started = self.closing = self.clipboard = False
        self.control = False
        self.devices = self.node = 0
        self.width = self.height = 0
        self.logical_size = None
        self.pipeline = self.sink = self.pipeline_bus = None
        self.pipewire_fd = None
        self.closed_code = None
        self.subscriptions = []
        self.requests = set()
        self.keys, self.buttons = set(), 0
        self.clipboard_payload = None
        self.mime_types = None
        self.clipboard_owner = False
        self.transfer_serials = set()
        self.pending_transfers = deque()
        self.in_transfer = False
        self.clipboard_fds = set()
        self.in_guard = False
        self.last_guard = 0.0
        self.responding = False
        self.Gio, self.GLib, self.Gst, self.GstVideo = bindings()
        self.context = self.GLib.MainContext.default()
        self.environment = LocalEnvironment()
        self.bus = self.connect(self.environment.bus_address)
        self.system = self.connect("unix:path=/run/dbus/system_bus_socket")
        self.bus.set_exit_on_close(False)
        self.system.set_exit_on_close(False)
        self.bus.connect("closed", lambda *_: self.revoke("portal_connection_closed"))
        self.system.connect("closed", lambda *_: self.revoke("logind_connection_closed"))
        # A read-only property request may activate the installed portal service,
        # but never creates a session, consent request, stream or clipboard grant.
        self.screen_properties = self.properties(self.bus, PORTAL, PORTAL_PATH, SCREEN)
        self.owner = self.name_owner(self.bus, PORTAL)
        try:
            self.remote_properties = self.properties(self.bus, self.owner, PORTAL_PATH, REMOTE)
        except Refused as error:
            if error.code != "portal_call_failed":
                raise
            self.remote_properties = {}
        try:
            self.clip_properties = self.properties(self.bus, self.owner, PORTAL_PATH, CLIP)
        except Refused as error:
            if error.code != "portal_call_failed":
                raise
            self.clip_properties = {}
        self.login_owner = self.name_owner(self.system, LOGIN)
        try:
            self.login_path = self.rpc(self.system, self.login_owner, LOGIN_PATH, LOGIN + ".Manager",
                                       "GetSessionByPID", "(u)", (os.getpid(),)).unpack()[0]
        except Refused as error:
            if error.code != "portal_call_failed":
                raise
            session_id = os.environ.get("XDG_SESSION_ID", "")
            if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", session_id):
                raise Refused("local_logind_session_required") from None
            self.login_path = self.rpc(self.system, self.login_owner, LOGIN_PATH, LOGIN + ".Manager",
                                       "GetSession", "(s)", (session_id,)).unpack()[0]
        self.subscribe(self.system, self.login_owner, PROPERTIES, "PropertiesChanged", self.login_path,
                       LOGIN_SESSION, self.logind_changed)
        self.subscribe(self.bus, DBUS, DBUS, "NameOwnerChanged", DBUS_PATH, PORTAL,
                       lambda *_: self.revoke("portal_owner_changed"))
        self.subscribe(self.system, DBUS, DBUS, "NameOwnerChanged", DBUS_PATH, LOGIN,
                       lambda *_: self.revoke("logind_owner_changed"))
        self.assert_access(refresh=True)

    def revoke(self, code):
        if self.closed_code is None:
            self.closed_code = code
        self.stopped.set()

    def basic(self):
        if not self.closing and (self.stopped.is_set() or os.getppid() != self.parent):
            raise Refused(self.closed_code or "parent_or_input_closed")
        if self.bus is not None and self.bus.is_closed():
            raise Refused("portal_connection_closed")

    def pump(self):
        # GDBus subscriptions/clipboard requests stay live while stdin is idle.
        for _ in range(256):
            if not self.context.pending():
                break
            self.context.iteration(False)
        if self.pipeline_bus is not None and not self.closing:
            message = self.pipeline_bus.timed_pop_filtered(
                0, self.Gst.MessageType.ERROR | self.Gst.MessageType.EOS)
            if message is not None:
                self.revoke("pipewire_stream_closed")
        self.basic()
        # Transfers may arrive during another asynchronous call. Queue them in
        # the signal callback and service one at a time, without recursive IO.
        if self.pending_transfers and not self.closing and not self.in_guard and not self.in_transfer:
            self.in_transfer = True
            try:
                self.complete_transfer(*self.pending_transfers.popleft())
            finally:
                self.in_transfer = False
            self.basic()

    def wait(self, done, timeout):
        deadline = time.monotonic() + timeout
        while not done():
            self.pump()
            if time.monotonic() >= deadline:
                raise Refused("portal_timeout")
            time.sleep(0.005)
        self.basic()

    def connect(self, address):
        result, cancel = [], self.Gio.Cancellable()
        def complete(_source, response, _data):
            try:
                value = self.Gio.DBusConnection.new_for_address_finish(response)
                if cancel.is_cancelled() or self.closing or self.stopped.is_set() or os.getppid() != self.parent:
                    value.close_sync(None)
                    result.append((None, "parent_or_input_closed"))
                else:
                    result.append((value, None))
            except self.GLib.Error:
                result.append((None, "local_bus_unavailable"))
        self.Gio.DBusConnection.new_for_address(
            address, self.Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
            self.Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, cancel, complete, None)
        try:
            self.wait(lambda: bool(result), 5)
        except Exception:
            if result and result[0][0] is not None:
                try:
                    result[0][0].close_sync(None)
                except self.GLib.Error:
                    pass
            raise
        finally:
            cancel.cancel()
        value, error = result[0]
        if error:
            raise Refused(error)
        return value

    def rpc(self, connection, destination, path, interface, method, signature="()", arguments=(),
            with_fd=False, timeout=5):
        result, cancel = [], self.Gio.Cancellable()
        def complete(source, response, _data):
            try:
                value = (source.call_with_unix_fd_list_finish(response) if with_fd
                         else source.call_finish(response))
                result.append((value, None))
            except self.GLib.Error:
                result.append((None, "portal_call_failed"))
        args = (destination, path, interface, method, self.GLib.Variant(signature, arguments),
                None, self.Gio.DBusCallFlags.NONE, max(1, int(timeout * 1000)))
        if with_fd:
            connection.call_with_unix_fd_list(*args, None, cancel, complete, None)
        else:
            connection.call(*args, cancel, complete, None)
        try:
            self.wait(lambda: bool(result), timeout + 0.1)
        finally:
            cancel.cancel()
        value, error = result[0]
        if error:
            raise Refused(error)
        return value

    def fd_call(self, interface, method, signature, arguments):
        reply, descriptors = self.rpc(self.bus, self.owner, PORTAL_PATH, interface, method,
                                      signature, arguments, with_fd=True)
        values = reply.unpack()
        if len(values) != 1 or descriptors is None:
            raise Refused("portal_fd_invalid")
        index = integer(values[0], 0, descriptors.get_length() - 1)
        fd = None
        try:
            fd = descriptors.get(index)
            os.set_inheritable(fd, False)
            return fd
        except (OSError, self.GLib.Error):
            if fd is not None:
                os.close(fd)
            raise Refused("portal_fd_invalid") from None

    def name_owner(self, connection, name):
        return self.rpc(connection, DBUS, DBUS_PATH, DBUS, "GetNameOwner",
                        "(s)", (name,)).unpack()[0]

    def properties(self, connection, owner, path, interface):
        result = self.rpc(connection, owner, path, PROPERTIES, "GetAll", "(s)", (interface,)).unpack()[0]
        if not isinstance(result, dict) or len(result) > 256:
            raise Refused("portal_properties_invalid")
        return result

    def subscribe(self, connection, sender, interface, member, path, arg0, callback):
        subscription = connection.signal_subscribe(sender, interface, member, path, arg0,
                                                   self.Gio.DBusSignalFlags.NONE, callback, None)
        self.subscriptions.append((connection, subscription))
        return subscription

    def logind_changed(self, _connection, _sender, _path, _iface, _signal, parameters, _data):
        try:
            iface, changed, invalidated = parameters.unpack()
            if iface == LOGIN_SESSION and (changed.get("LockedHint") is True
                    or changed.get("Active") is False or "LockedHint" in invalidated or "Active" in invalidated):
                self.revoke("local_session_locked_or_inactive")
        except (TypeError, ValueError):
            self.revoke("logind_properties_invalid")

    def assert_access(self, refresh=False):
        self.basic()
        if self.in_guard:
            return
        if refresh or time.monotonic() - self.last_guard >= 0.5:
            self.in_guard = True
            try:
                self.environment.check()
                if (self.name_owner(self.bus, PORTAL) != self.owner
                        or self.name_owner(self.system, LOGIN) != self.login_owner):
                    raise Refused("local_service_owner_changed")
                facts = self.properties(self.system, self.login_owner, self.login_path, LOGIN_SESSION)
                user = facts.get("User")
                if (not isinstance(user, (tuple, list)) or len(user) != 2 or user[0] != self.environment.uid
                        or facts.get("Active") is not True or facts.get("LockedHint") is not False
                        or facts.get("Remote") is not False or facts.get("Type") != "wayland"):
                    raise Refused("local_session_locked_or_inactive")
                self.last_guard = time.monotonic()
            finally:
                self.in_guard = False

    def options(self, values):
        result = {}
        for key, value in values.items():
            kind = "b" if type(value) is bool else "u" if type(value) is int else "as" if isinstance(value, list) else "s"
            result[key] = self.GLib.Variant(kind, value)
        return result

    def request(self, interface, method, prefix_signature, prefix, options, timeout=120):
        token = "cowork_" + uuid.uuid4().hex
        options = self.options(dict(options, handle_token=token))
        sender = self.bus.get_unique_name().removeprefix(":").replace(".", "_")
        path = PORTAL_PATH + "/request/" + sender + "/" + token
        received = []
        def response(_connection, _sender, _path, _iface, _signal, parameters, _data):
            if not received:
                received.append(parameters.unpack())
        # Subscribe before dispatch; a fast Response may precede the method reply.
        sub = self.subscribe(self.bus, self.owner, "org.freedesktop.portal.Request", "Response",
                             path, None, response)
        self.requests.add(path)
        completed = False
        try:
            handle = self.rpc(self.bus, self.owner, PORTAL_PATH, interface, method,
                              "(" + prefix_signature + "a{sv})", (*prefix, options)).unpack()[0]
            if handle != path:
                raise Refused("portal_request_identity_changed")
            self.wait(lambda: bool(received), timeout)
            completed = True
            code, result = received[0]
            if code != 0:
                raise Refused("portal_permission_denied" if code == 1 else "portal_request_failed")
            if not isinstance(result, dict) or len(result) > 256:
                raise Refused("portal_response_invalid")
            return result
        finally:
            # A pending original request stays owned until close cancels it;
            # cancellation while a native consent dialog is open must close it.
            if completed:
                self.requests.discard(path)
            self.bus.signal_unsubscribe(sub)
            self.subscriptions.remove((self.bus, sub))

    def probe(self):
        plugins = all(self.Gst.ElementFactory.find(name) is not None
                      for name in ("pipewiresrc", "videoconvert", "capsfilter", "appsink"))
        view = (plugins and self.screen_properties.get("version", 0) >= 2
                and self.screen_properties.get("AvailableSourceTypes", 0) & 1
                and self.screen_properties.get("AvailableCursorModes", 0) & 2)
        control = bool(view and self.remote_properties.get("version", 0) >= 1
                       and self.remote_properties.get("AvailableDeviceTypes", 0) & 3 == 3)
        clipboard = bool(control and self.remote_properties.get("version", 0) >= 2
                         and self.clip_properties.get("version", 0) >= 1)
        return {"ok": True, "available": bool(view), "control": control, "clipboard": clipboard,
                "screencast_version": self.screen_properties.get("version", 0),
                "remote_desktop_version": self.remote_properties.get("version", 0),
                "clipboard_version": self.clip_properties.get("version", 0)}

    def open(self, control):
        if self.session is not None or self.started:
            raise Refused("session_already_open")
        self.assert_access(refresh=True)
        available = self.probe()
        if not available["available"] or control and not available["control"]:
            raise Refused("portal_feature_unsupported")
        session_token = "cowork_session_" + uuid.uuid4().hex
        sender = self.bus.get_unique_name().removeprefix(":").replace(".", "_")
        self.session = PORTAL_PATH + "/session/" + sender + "/" + session_token
        self.subscribe(self.bus, self.owner, "org.freedesktop.portal.Session", "Closed", self.session,
                       None, lambda *_: self.revoke("portal_session_closed"))
        interface = REMOTE if control else SCREEN
        created = self.request(interface, "CreateSession", "", (), {"session_handle_token": session_token})
        if created.get("session_handle") != self.session:
            raise Refused("portal_session_identity_changed")
        self.control = control
        if control:
            devices = {"types": 3}
            if self.remote_properties.get("version", 0) >= 2:
                devices["persist_mode"] = 0
            self.request(REMOTE, "SelectDevices", "o", (self.session,), devices)
        sources = {"types": 1, "multiple": False, "cursor_mode": 2}
        if not control and self.screen_properties.get("version", 0) >= 4:
            sources["persist_mode"] = 0
        self.request(SCREEN, "SelectSources", "o", (self.session,), sources)
        clip_requested = False
        if control and available["clipboard"]:
            self.subscribe(self.bus, self.owner, CLIP, "SelectionOwnerChanged", PORTAL_PATH,
                           None, self.selection_changed)
            self.subscribe(self.bus, self.owner, CLIP, "SelectionTransfer", PORTAL_PATH,
                           None, self.selection_transfer)
            try:
                self.rpc(self.bus, self.owner, PORTAL_PATH, CLIP, "RequestClipboard",
                         "(oa{sv})", (self.session, {}))
                clip_requested = True
            except Refused as error:
                if error.code != "portal_call_failed":
                    raise
                clip_requested = False
        started = self.request(interface, "Start", "os", (self.session, ""), {})
        self.devices = started.get("devices", 0) if control else 0
        if control and (type(self.devices) is not int or self.devices & 3 != 3):
            raise Refused("control_not_granted")
        self.clipboard = bool(clip_requested and started.get("clipboard_enabled") is True)
        streams = started.get("streams")
        if not isinstance(streams, (list, tuple)) or len(streams) != 1:
            raise Refused("one_monitor_required")
        node, properties = streams[0]
        self.node = integer(node, 1, 0xffffffff)
        if not isinstance(properties, dict) or len(properties) > 128 or properties.get("source_type", 1) != 1:
            raise Refused("monitor_stream_required")
        logical = properties.get("logical_size", properties.get("size"))
        if logical is not None:
            if not isinstance(logical, (tuple, list)) or len(logical) != 2:
                raise Refused("stream_geometry_invalid")
            self.logical_size = tuple(integer(value, 1, 65535) for value in logical)
        self.started = True
        self.assert_access(refresh=True)
        self.pipewire_fd = self.fd_call(SCREEN, "OpenPipeWireRemote", "(oa{sv})", (self.session, {}))
        self.start_pipeline(properties)
        width, height, _pixels = self.frame()
        self.width, self.height = width, height
        return {"ok": True, "width": width, "height": height, "clipboard": self.clipboard,
                "devices": self.devices, "control": self.control}

    def start_pipeline(self, properties):
        Gst = self.Gst
        source = Gst.ElementFactory.make("pipewiresrc", "cowork_portal_source")
        convert = Gst.ElementFactory.make("videoconvert", "cowork_bgra_convert")
        filtered = Gst.ElementFactory.make("capsfilter", "cowork_bgra_caps")
        sink = Gst.ElementFactory.make("appsink", "cowork_frame_sink")
        if any(element is None for element in (source, convert, filtered, sink)):
            raise Refused("wayland_dependencies_missing")
        source.set_property("fd", self.pipewire_fd)
        serial = properties.get("pipewire-serial")
        if serial is not None:
            integer(serial, 1, 0xffffffffffffffff)
            if source.find_property("target-object") is None:
                raise Refused("pipewire_serial_target_unsupported")
            source.set_property("target-object", str(serial))
        else:
            if self.screen_properties.get("version", 0) >= 6:
                raise Refused("pipewire_serial_missing")
            source.set_property("path", str(self.node))
        # No default source, restore token, reconnect or ambient PipeWire remote.
        if source.find_property("on-disconnect") is not None:
            source.set_property("on-disconnect", 2)  # GstPipeWireSrcOnDisconnect.ERROR.
        if source.find_property("max-buffers") is not None:
            source.set_property("max-buffers", 3)
        if source.find_property("resend-last") is not None:
            source.set_property("resend-last", False)
        if source.find_property("keepalive-time") is not None:
            # A static monitor does not necessarily deliver changed buffers.
            # Keepalives belong only to this live stream; EOS/error still revokes.
            source.set_property("keepalive-time", 250)
        filtered.set_property("caps", Gst.Caps.from_string("video/x-raw,format=BGRA"))
        sink.set_property("max-buffers", 1)
        sink.set_property("drop", True)
        sink.set_property("sync", False)
        sink.set_property("enable-last-sample", False)
        self.pipeline = Gst.Pipeline.new("cowork_owned_portal_pipeline")
        for element in (source, convert, filtered, sink):
            self.pipeline.add(element)
        if not source.link(convert) or not convert.link(filtered) or not filtered.link(sink):
            raise Refused("pipewire_pipeline_failed")
        self.sink, self.pipeline_bus = sink, self.pipeline.get_bus()
        if self.pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
            raise Refused("pipewire_pipeline_failed")

    def require_session(self, control=False, clipboard=False):
        self.assert_access(refresh=True)
        if not self.started or self.session is None:
            raise Refused("session_not_open")
        if control and (not self.control or self.devices & 3 != 3):
            raise Refused("control_not_granted")
        if clipboard and not self.clipboard:
            raise Refused("clipboard_not_granted", fatal=False)

    def frame(self):
        self.require_session()
        deadline = time.monotonic() + 5
        while True:
            self.pump()
            self.assert_access()
            sample = self.sink.emit("try-pull-sample", 0)
            if sample is not None:
                break
            if time.monotonic() >= deadline:
                raise Refused("frame_unavailable")
            time.sleep(0.01)
        caps = sample.get_caps()
        structure = caps.get_structure(0)
        if structure.get_string("format") != "BGRA":
            raise Refused("frame_format_invalid")
        info = self.GstVideo.VideoInfo.new_from_caps(caps)
        width, height = integer(info.width, 1, MAX_WIDTH), integer(info.height, 1, MAX_HEIGHT)
        if self.width and (width != self.width or height != self.height):
            raise Refused("frame_geometry_changed")
        buffer = sample.get_buffer()
        if buffer.get_size() > MAX_LAYOUT:
            raise Refused("frame_layout_invalid")
        meta = self.GstVideo.buffer_get_video_meta(buffer)
        if meta is not None and (meta.width != width or meta.height != height or meta.n_planes != 1
                                 or meta.format != self.GstVideo.VideoFormat.BGRA):
            raise Refused("frame_layout_invalid")
        stride, offset = ((meta.stride[0], meta.offset[0]) if meta is not None
                          else (info.stride[0], info.offset[0]))
        row = width * 4
        if type(stride) is not int or not row <= stride <= row + 4096 or not 0 <= offset <= 4096:
            raise Refused("frame_layout_invalid")
        success, mapped = buffer.map(self.Gst.MapFlags.READ)
        if not success:
            raise Refused("frame_map_failed")
        memory = None
        try:
            memory = memoryview(mapped.data)
            last = offset + (height - 1) * stride + row
            if len(memory) > MAX_LAYOUT or last > len(memory):
                raise Refused("frame_layout_invalid")
            if stride == row:
                pixels = bytes(memory[offset:offset + row * height])
            else:
                pixels = bytearray(row * height)
                for index in range(height):
                    pixels[index * row:(index + 1) * row] = memory[offset + index * stride:offset + index * stride + row]
        finally:
            if memory is not None:
                memory.release()
            buffer.unmap(mapped)
        self.require_session()
        return width, height, pixels

    def notify(self, method, signature, arguments, cleanup=False):
        if not cleanup:
            self.require_session(control=True)
        self.rpc(self.bus, self.owner, PORTAL_PATH, REMOTE, method, signature,
                 (self.session, {}, *arguments), timeout=0.1 if cleanup else 5)

    def key(self, keysym, down):
        self.require_session(control=True)
        if down and len(self.keys) >= 256 and keysym not in self.keys:
            raise Refused("held_input_limit")
        # Record possible held state before IO, including a lost method reply.
        if down:
            self.keys.add(keysym)
        signed = keysym if keysym < 0x80000000 else keysym - 0x100000000
        self.notify("NotifyKeyboardKeysym", "(oa{sv}iu)", (signed, int(down)))
        if not down:
            self.keys.discard(keysym)
        return {"ok": True}

    def pointer(self, command):
        self.require_session(control=True)
        if command["width"] != self.width or command["height"] != self.height or command["previous"] != self.buttons:
            raise Refused("input_sequence_changed")
        logical_width, logical_height = self.logical_size or (self.width, self.height)
        x = min(command["x"], self.width - 1) * logical_width / self.width
        y = min(command["y"], self.height - 1) * logical_height / self.height
        self.notify("NotifyPointerMotionAbsolute", "(oa{sv}udd)", (self.node, x, y))
        target = command["buttons"] & 7
        for mask, button in ((1, 272), (2, 274), (4, 273)):
            if bool(target & mask) != bool(self.buttons & mask):
                down = bool(target & mask)
                if down:
                    self.buttons |= mask
                self.notify("NotifyPointerButton", "(oa{sv}iu)", (button, int(down)))
                if not down:
                    self.buttons &= ~mask
        for mask, axis, steps in ((8, 0, -1), (16, 0, 1), (32, 1, -1), (64, 1, 1)):
            if command["buttons"] & mask:
                self.notify("NotifyPointerAxisDiscrete", "(oa{sv}ui)", (axis, steps))
        return {"ok": True}

    def selection_changed(self, _connection, _sender, _path, _iface, _signal, parameters, _data):
        if not self.started or not self.clipboard or self.closing:
            return
        try:
            session, options = parameters.unpack()
            if session != self.session:
                return
            if not isinstance(options, dict) or len(options) > 128:
                raise Refused("clipboard_protocol_invalid")
            values = options.get("mime_types", [])
            if not isinstance(values, (list, tuple)) or len(values) > 256:
                raise Refused("clipboard_protocol_invalid")
            self.mime_types = tuple(value for value in values if value in TEXT_MIMES)
            self.clipboard_owner = options.get("session_is_owner") is True
            if not self.clipboard_owner:
                self.clipboard_payload = None
        except (Refused, TypeError, ValueError):
            self.revoke("clipboard_protocol_invalid")

    def fd_transfer(self, fd, data=None):
        self.clipboard_fds.add(fd)
        deadline, offset, result = time.monotonic() + 5, 0, bytearray()
        try:
            os.set_blocking(fd, False)
            while True:
                self.pump()
                self.assert_access()
                if time.monotonic() >= deadline:
                    raise Refused("clipboard_transfer_timeout", fatal=False)
                try:
                    if data is None:
                        chunk = os.read(fd, min(65536, MAX_TEXT + 1 - len(result)))
                        if not chunk:
                            return bytes(result)
                        result.extend(chunk)
                        if len(result) > MAX_TEXT:
                            raise Refused("clipboard_length", fatal=False)
                    else:
                        if offset == len(data):
                            return None
                        offset += os.write(fd, data[offset:offset + 65536])
                except BlockingIOError:
                    select.select([fd] if data is None else [], [fd] if data is not None else [], [], 0.01)
        finally:
            self.clipboard_fds.discard(fd)
            os.close(fd)

    def selection_transfer(self, _connection, _sender, _path, _iface, _signal, parameters, _data):
        if self.closing or not self.started or not self.clipboard:
            return
        try:
            session, mime, serial = parameters.unpack()
            if session != self.session:
                return
            integer(serial, 0, 0xffffffff)
            if serial in self.transfer_serials or len(self.transfer_serials) >= 1024:
                raise Refused("clipboard_transfer_identity_changed")
            self.transfer_serials.add(serial)
            if type(mime) is not str or len(mime) > 256 or len(self.pending_transfers) >= 32:
                raise Refused("clipboard_protocol_invalid")
            # Capture the offered bytes now; an owner-change signal must not
            # silently substitute another selection during the transfer.
            self.pending_transfers.append((mime, serial, self.clipboard_payload))
        except (Refused, TypeError, ValueError):
            self.revoke("clipboard_protocol_invalid")

    def complete_transfer(self, mime, serial, payload):
        success = False
        try:
            self.require_session(clipboard=True)
            if mime not in TEXT_MIMES or payload is None:
                raise Refused("clipboard_format_unsupported", fatal=False)
            fd = self.fd_call(CLIP, "SelectionWrite", "(ou)", (self.session, serial))
            self.fd_transfer(fd, payload)
            success = True
        except Refused as error:
            if error.fatal:
                self.revoke(error.code)
        except (OSError, TypeError, ValueError):
            success = False
        finally:
            if self.started and self.clipboard and not self.closing and not self.stopped.is_set():
                try:
                    self.rpc(self.bus, self.owner, PORTAL_PATH, CLIP, "SelectionWriteDone",
                             "(oub)", (self.session, serial, success))
                except Refused:
                    self.revoke("clipboard_transfer_failed")

    def clipboard_read(self):
        self.require_session(clipboard=True)
        if self.clipboard_owner:
            return {"ok": True, "present": False}, b""
        candidates = self.mime_types if self.mime_types is not None else TEXT_MIMES
        for mime in candidates:
            try:
                fd = self.fd_call(CLIP, "SelectionRead", "(os)", (self.session, mime))
            except Refused as error:
                if error.code != "portal_call_failed":
                    raise
                continue
            data = self.fd_transfer(fd)
            try:
                data.decode("utf-8")
            except UnicodeError:
                raise Refused("clipboard_encoding_unsupported", fatal=False) from None
            self.require_session(clipboard=True)
            return {"ok": True, "present": True}, data
        return {"ok": True, "present": False}, b""

    def clipboard_write(self, text):
        self.require_session(control=True, clipboard=True)
        self.clipboard_payload = text.encode("utf-8")
        self.clipboard_owner = True
        self.rpc(self.bus, self.owner, PORTAL_PATH, CLIP, "SetSelection", "(oa{sv})",
                 (self.session, self.options({"mime_types": list(TEXT_MIMES)})))
        return {"ok": True}

    def close(self):
        if self.closing:
            return
        self.closing = True
        deadline = time.monotonic() + 1.5
        try:
            if self.started and self.control and self.session is not None:
                for key in tuple(self.keys):
                    if time.monotonic() >= deadline:
                        break
                    try:
                        signed = key if key < 0x80000000 else key - 0x100000000
                        self.notify("NotifyKeyboardKeysym", "(oa{sv}iu)", (signed, 0), cleanup=True)
                    except Exception:
                        break
                for mask, button in ((1, 272), (2, 274), (4, 273)):
                    if self.buttons & mask and time.monotonic() < deadline:
                        try:
                            self.notify("NotifyPointerButton", "(oa{sv}iu)", (button, 0), cleanup=True)
                        except Exception:
                            break
        finally:
            self.keys.clear()
            self.buttons = 0
            self.clipboard_payload = None
            self.pending_transfers.clear()
            self.transfer_serials.clear()
            for fd in tuple(self.clipboard_fds):
                try:
                    os.close(fd)
                except OSError:
                    pass
            self.clipboard_fds.clear()
            if self.pipeline is not None:
                try:
                    self.pipeline.set_state(self.Gst.State.NULL)
                    self.pipeline.get_state(self.Gst.SECOND)
                except Exception:
                    pass
                finally:
                    self.pipeline = self.sink = self.pipeline_bus = None
            if self.pipewire_fd is not None:
                try:
                    os.close(self.pipewire_fd)
                except OSError:
                    pass
                self.pipewire_fd = None
            # Each teardown step is independent. A failed pipeline or request
            # close must still disconnect the uniquely owned portal connection,
            # which also ends all its sessions and virtual input devices.
            if self.bus is not None and not self.bus.is_closed():
                for path in tuple(self.requests):
                    try:
                        self.rpc(self.bus, self.owner, path, "org.freedesktop.portal.Request", "Close", timeout=0.1)
                    except Exception:
                        pass
                if self.session is not None:
                    try:
                        self.rpc(self.bus, self.owner, self.session, "org.freedesktop.portal.Session", "Close", timeout=0.2)
                    except Exception:
                        pass
            self.requests.clear()
            for connection, sub in self.subscriptions:
                try:
                    connection.signal_unsubscribe(sub)
                except Exception:
                    pass
            self.subscriptions.clear()
            for connection in (self.bus, self.system):
                if connection is not None:
                    try:
                        if not connection.is_closed():
                            connection.close_sync(None)
                    except Exception:
                        pass
            self.bus = self.system = None
            self.started = False


def write_response(portal, metadata, payload=b""):
    header = response_header(metadata, len(payload))
    descriptor = sys.stdout.buffer.fileno()
    os.set_blocking(descriptor, False)
    portal.responding = True
    deadline = time.monotonic() + 10
    for source in (memoryview(header), memoryview(payload)):
        offset = 0
        while offset < len(source):
            portal.pump()
            if portal.started:
                portal.assert_access()
            if time.monotonic() >= deadline:
                raise Refused("parent_output_closed")
            try:
                written = os.write(descriptor, source[offset:offset + 262144])
                if written <= 0:
                    raise Refused("parent_output_closed")
                offset += written
            except BlockingIOError:
                select.select([], [descriptor], [], 0.01)
    portal.responding = False


def plain_response(metadata):
    # Used only before a Portal can be constructed, never carries user data.
    descriptor, data = sys.stdout.buffer.fileno(), response_header(metadata)
    try:
        os.set_blocking(descriptor, False)
        offset, deadline = 0, time.monotonic() + 1
        while offset < len(data):
            if time.monotonic() >= deadline:
                return
            try:
                written = os.write(descriptor, data[offset:])
                if written <= 0:
                    return
                offset += written
            except BlockingIOError:
                select.select([], [descriptor], [], 0.01)
    except (OSError, ValueError):
        return


def unavailable_probe(code):
    return {"ok": True, "available": False, "control": False, "clipboard": False,
            "error": code, "screencast_version": 0, "remote_desktop_version": 0,
            "clipboard_version": 0}


def stdin_reader(commands, stopped):
    try:
        while not stopped.is_set():
            raw = sys.stdin.buffer.readline(MAX_COMMAND + 1)
            if not raw:
                stopped.set()
                return
            try:
                item = parse_command(raw)
            except Refused as error:
                item = error
            try:
                commands.put(item, timeout=0.1)
            except queue.Full:
                stopped.set()
                return
            if isinstance(item, Refused):
                return
    except (OSError, ValueError):
        stopped.set()


def main():
    # Native library diagnostics can contain paths. This owned helper never
    # prints framebuffer/text/paths or uncaught exception details to stderr.
    null = os.open(os.devnull, os.O_WRONLY)
    os.dup2(null, 2)
    os.close(null)
    os.environ["GST_DEBUG"] = "0"
    os.environ.pop("GST_DEBUG_DUMP_DOT_DIR", None)
    if sys.argv[1:] not in ([], ["--probe"]):
        plain_response({"ok": False, "error": "invalid_arguments"})
        return 1
    probing = sys.argv[1:] == ["--probe"]
    stopped, parent = threading.Event(), os.getppid()
    if sys.platform != "linux" or parent <= 1:
        plain_response(unavailable_probe("local_wayland_required") if probing
                       else {"ok": False, "error": "local_wayland_required"})
        return 0 if probing else 1
    for event in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(event, lambda *_: stopped.set())
    try:
        native = ctypes.CDLL(None, use_errno=True)
        if native.prctl(1, signal.SIGTERM, 0, 0, 0) != 0 or os.getppid() != parent:
            raise Refused("parent_or_input_closed")
    except (OSError, Refused):
        plain_response(unavailable_probe("parent_or_input_closed") if probing
                       else {"ok": False, "error": "parent_or_input_closed"})
        return 0 if probing else 1
    portal = None
    try:
        portal = Portal.__new__(Portal)
        portal.__init__(stopped, parent)
        if probing:
            write_response(portal, portal.probe())
            return 0
        commands = queue.Queue(maxsize=4)
        threading.Thread(target=stdin_reader, args=(commands, stopped), daemon=True).start()
        while not stopped.is_set():
            portal.pump()
            portal.assert_access()
            try:
                command = commands.get(timeout=0.01)
            except queue.Empty:
                continue
            try:
                if isinstance(command, Refused):
                    raise command
                op, payload = command["op"], b""
                if op == "open":
                    result = portal.open(command["control"])
                elif op == "frame":
                    width, height, payload = portal.frame()
                    result = {"ok": True, "width": width, "height": height}
                elif op == "key":
                    result = portal.key(command["keysym"], command["down"])
                elif op == "pointer":
                    result = portal.pointer(command)
                elif op == "clipboard_read":
                    result, payload = portal.clipboard_read()
                elif op == "clipboard_write":
                    result = portal.clipboard_write(command["text"])
                else:
                    portal.close()
                    plain_response({"ok": True})
                    return 0
                write_response(portal, result, payload)
            except Refused as error:
                if not portal.responding:
                    plain_response({"ok": False, "error": error.code})
                if error.fatal or portal.responding:
                    return 1
        return 0
    except Refused as error:
        if portal is None or not portal.responding:
            plain_response(unavailable_probe(error.code) if probing else {"ok": False, "error": error.code})
        return 0 if probing else 1
    except Exception:
        if portal is None or not portal.responding:
            plain_response(unavailable_probe("wayland_helper_failed") if probing
                           else {"ok": False, "error": "wayland_helper_failed"})
        return 0 if probing else 1
    finally:
        if portal is not None:
            try:
                portal.close()
            except Exception:
                # D-Bus connection teardown still closes its uniquely owned session.
                pass


if __name__ == "__main__":
    raise SystemExit(main())
