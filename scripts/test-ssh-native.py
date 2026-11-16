#!/usr/bin/env python3
"""Exercise the real Rust/libssh2 transport against a read-only loopback fixture.
Requires an existing paramiko installation; installs and downloads nothing.
"""
import errno
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import time
import paramiko


class Auth(paramiko.ServerInterface):
    key = None

    def check_auth_password(self, username, password):
        return paramiko.AUTH_SUCCESSFUL if (username, password) == ("test", "fixture") else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return "password,publickey"

    def check_auth_publickey(self, username, key):
        return paramiko.AUTH_SUCCESSFUL if username == "test" and key == self.key else paramiko.AUTH_FAILED

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED


class Handle(paramiko.SFTPHandle):
    def __init__(self, path):
        super().__init__()
        self.readfile = path.open("rb")
        self.slow = path.name == "slow.mrc"

    def stat(self):
        return paramiko.SFTPAttributes.from_stat(os.fstat(self.readfile.fileno()))

    def read(self, offset, length):
        if self.slow:
            time.sleep(0.03)
        return super().read(offset, length)


class Files(paramiko.SFTPServerInterface):
    root = None

    def local(self, path):
        candidate = (self.root / path.lstrip("/")).resolve()
        if candidate != self.root and self.root not in candidate.parents:
            raise OSError(errno.EACCES, "outside fixture")
        return candidate

    def canonicalize(self, path):
        return "/" + str(self.local(path).relative_to(self.root)).replace("\\", "/").removeprefix(".")

    def stat(self, path):
        try:
            return paramiko.SFTPAttributes.from_stat(self.local(path).stat())
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    lstat = stat

    def list_folder(self, path):
        try:
            result = []
            for p in self.local(path).iterdir():
                attr = paramiko.SFTPAttributes.from_stat(p.lstat())
                attr.filename = p.name
                result.append(attr)
            return result
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)

    def open(self, path, flags, attr):
        if flags & (os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC):
            return paramiko.SFTP_PERMISSION_DENIED
        try:
            return Handle(self.local(path))
        except OSError as error:
            return paramiko.SFTPServer.convert_errno(error.errno)


def main():
    key = paramiko.RSAKey.generate(2048)
    wrong_key = paramiko.RSAKey.generate(2048)
    client_key = paramiko.RSAKey.generate(2048)
    Auth.key = client_key
    transports = []
    stop = threading.Event()
    with tempfile.TemporaryDirectory(prefix="tmv-sftp-") as temporary:
        root = Path(temporary).resolve()
        Files.root = root
        client_key.write_private_key_file(str(root / "client-key"))
        client_key.write_private_key_file(str(root / "client-key-encrypted"), password="fixture-key-passphrase")
        (root / "中文 folder").mkdir()
        (root / "image space.mrc").write_bytes(b"SFTP binary fixture\0\xff")
        header = bytearray(1024)
        struct.pack_into("<4i", header, 0, 2, 2, 1, 2)
        struct.pack_into("<3i", header, 28, 2, 2, 1)
        header[208:212] = b"MAP "
        header[212:216] = b"DA\0\0"
        (root / "valid.mrc").write_bytes(header + struct.pack("<4f", 1.0, 2.0, 3.0, 4.0))
        (root / "notes.txt").write_text("not an image")
        (root / "slow.mrc").write_bytes(b"x" * (8 * 1024 * 1024))
        (root / "folder link").symlink_to(root / "中文 folder", target_is_directory=True)
        (root / "file link.mrc").symlink_to(root / "image space.mrc")
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(8)
        listener.settimeout(0.25)

        def accept():
            while not stop.is_set():
                try:
                    client, _ = listener.accept()
                except socket.timeout:
                    continue
                except OSError:
                    return
                transport = paramiko.Transport(client)
                transports.append(transport)
                transport.add_server_key(key)
                transport.set_subsystem_handler("sftp", paramiko.SFTPServer, Files)
                try:
                    transport.start_server(server=Auth())
                except paramiko.SSHException:
                    transport.close()

        worker = threading.Thread(target=accept, daemon=True)
        worker.start()
        env = dict(os.environ, TMV_SSH_TEST_PORT=str(listener.getsockname()[1]), TMV_SSH_WRONG_HOST_KEY=wrong_key.get_base64(),
                   TMV_SSH_TEST_KEY=str(root / "client-key"), TMV_SSH_TEST_ENCRYPTED_KEY=str(root / "client-key-encrypted"))
        try:
            result = subprocess.run(["cargo", "test", "--offline", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "--no-default-features", "--features", "ssh", "plugins::ssh::tests::live_sftp_roundtrip", "--", "--ignored", "--nocapture"], env=env)
        finally:
            stop.set()
            listener.close()
            for transport in transports:
                transport.close()
            worker.join(2)
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
