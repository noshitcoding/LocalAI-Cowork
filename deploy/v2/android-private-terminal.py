"""Original actor-private PTY, called only by the owned PID-1 guest supervisor."""
import base64
import errno
import fcntl
import hashlib
import json
import os
import pathlib
import pty
import selectors
import signal
import subprocess
import termios
import time


def run(payload, supervisor):
    require = supervisor['require']
    parse = supervisor['parse']
    emit = supervisor['emit']
    terminate = supervisor['terminate_children']
    root = supervisor['ROOT']
    require(os.getpid() == 1 and os.getuid() == 0)
    session = payload['session_id']
    require(isinstance(session, str) and len(session) == 36)
    program = payload['program']
    require(isinstance(program, str) and program.startswith('/') and '\0' not in program)
    arguments = payload.get('arguments', [])
    require(isinstance(arguments, list) and len(arguments) <= 128)
    require(all(isinstance(a, str) and '\0' not in a for a in arguments))
    columns, rows = payload['columns'], payload['rows']
    require(type(columns) is int and 20 <= columns <= 400)
    require(type(rows) is int and 5 <= rows <= 200)
    timeout = payload['timeout_seconds']
    require(type(timeout) is int and 1 <= timeout <= 900)
    master, slave = pty.openpty()
    # A selected executable sees a real controlling terminal; no host shell or inherited environment.
    termios.tcsetwinsize(slave, (rows, columns))
    child = None
    selector = selectors.DefaultSelector()
    started = time.monotonic()
    input_sequence = 0
    output_sequence = 0
    input_bytes = 0
    output_bytes = 0
    buffer = bytearray()
    reason = 'process_exit'
    try:
        child = subprocess.Popen(
            [program, *arguments], cwd=root,
            env={'PATH': '/opt/cowork/python/bin:/usr/local/bin:/usr/bin:/bin',
                 'HOME': '/tmp', 'TMPDIR': '/tmp', 'LANG': 'C.UTF-8',
                 'TERM': 'xterm-256color'},
            stdin=slave, stdout=slave, stderr=slave, close_fds=True,
            start_new_session=True, user=1000, group=1000, extra_groups=[],
            preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        os.close(slave)
        slave = None
        os.set_blocking(master, False)
        os.set_blocking(0, False)
        selector.register(master, selectors.EVENT_READ, 'pty')
        selector.register(0, selectors.EVENT_READ, 'controller')
        emit({'kind': 'terminal_ready', 'nonce': payload['nonce'], 'session_id': session,
              'process_id': child.pid})
        closing = False
        while selector.get_map():
            if time.monotonic() - started >= timeout:
                reason = 'deadline'
                closing = True
                terminate(child)
            for key, _ in selector.select(timeout=.05):
                try:
                    raw = os.read(key.fd, 32768)
                except OSError as error:
                    if key.data == 'pty' and error.errno == errno.EIO:
                        raw = b''
                    else:
                        raise
                if not raw:
                    selector.unregister(key.fd)
                    if key.data == 'controller' and not closing:
                        reason = 'controller_closed'
                        closing = True
                        terminate(child)
                    continue
                if key.data == 'pty':
                    output_bytes += len(raw)
                    require(output_bytes <= 32 * 1024 * 1024 and output_sequence < 4000)
                    output_sequence += 1
                    emit({'kind': 'terminal_output', 'nonce': payload['nonce'],
                          'session_id': session, 'sequence': output_sequence,
                          'data': base64.b64encode(raw).decode(),
                          'sha256': hashlib.sha256(raw).hexdigest()})
                    continue
                buffer.extend(raw)
                require(len(buffer) <= 128 * 1024)
                while b'\n' in buffer:
                    position = buffer.index(b'\n')
                    frame = parse(bytes(buffer[:position]))
                    del buffer[:position + 1]
                    require(isinstance(frame, dict) and frame.get('nonce') == payload['nonce'])
                    require(frame.get('session_id') == session and not closing)
                    sequence = frame.get('sequence')
                    require(type(sequence) is int and sequence == input_sequence + 1 and sequence <= 4000)
                    input_sequence = sequence
                    operation = frame.get('operation')
                    written = 0
                    if operation == 'input':
                        value = base64.b64decode(frame['data'], validate=True)
                        require(0 < len(value) <= 65536)
                        input_bytes += len(value)
                        require(input_bytes <= 32 * 1024 * 1024)
                        # A partial write is reported as uncertain and is never replayed.
                        try:
                            written = os.write(master, value)
                        except BlockingIOError:
                            written = 0
                        complete = written == len(value)
                    elif operation == 'resize':
                        columns, rows = frame['columns'], frame['rows']
                        require(type(columns) is int and 20 <= columns <= 400)
                        require(type(rows) is int and 5 <= rows <= 200)
                        termios.tcsetwinsize(master, (rows, columns))
                        complete = True
                    elif operation == 'close':
                        closing = True
                        reason = 'explicit_close'
                        terminate(child)
                        complete = True
                    else:
                        raise ValueError('invalid_private_terminal_operation')
                    emit({'kind': 'terminal_input', 'nonce': payload['nonce'],
                          'session_id': session, 'sequence': sequence, 'operation': operation,
                          'written_bytes': written, 'complete': complete})
                    if not complete:
                        reason = 'partial_input'
                        closing = True
                        terminate(child)
            if child.poll() is not None:
                terminate(child)
                if 0 in selector.get_map():
                    selector.unregister(0)
        code = child.wait(timeout=5)
        terminate(child)
        emit({'kind': 'terminal_closed', 'nonce': payload['nonce'], 'session_id': session,
              'exit_code': code, 'input_sequence': input_sequence, 'output_sequence': output_sequence,
              'input_bytes': input_bytes, 'output_bytes': output_bytes, 'reason': reason,
              'descendants_ended': True, 'files': supervisor['export_files']()})
    finally:
        if child is not None:
            terminate(child)
        selector.close()
        os.close(master)
        if slave is not None:
            os.close(slave)
