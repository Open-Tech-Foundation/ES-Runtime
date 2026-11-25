#!/usr/bin/env python3
"""Records the db_wire seeds from real servers.

A proxy in front of a server keeps what the server sends, read by read, in
the order the client received it; the seed is the harness's mode byte, then
each read as a length byte and that many bytes (a read longer than 255 bytes
becomes several), which is how ScriptedNet hands reads back. Replayed, the
driver sees exactly what the server said.

    python3 -I fuzz/record-db/record.py <name> <mode> <listen-port> <server-port> -- <command...>

The command is the client, normally `esrun --allow-net=127.0.0.1 --allow-env
fuzz/record-db/client.mjs <mode> <listen-port>`: the harness's own connect and
query. The seed is written to fuzz/seeds/db_wire/<name>. See fuzz/README.md.
"""
import os
import socket
import subprocess
import sys
import threading

name, mode, listen_port, server_port = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
command = sys.argv[sys.argv.index("--") + 1:]
reads = []
lock = threading.Lock()


def pump(source, sink, record):
    while True:
        try:
            data = source.recv(65536)
        except OSError:
            data = b""
        if not data:
            try:
                sink.shutdown(socket.SHUT_WR)
            except OSError:
                pass
            return
        if record:
            with lock:
                reads.append(data)
        sink.sendall(data)


listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", listen_port))
listener.listen()


def serve():
    while True:
        try:
            client, _ = listener.accept()
        except OSError:
            return
        server = socket.create_connection(("127.0.0.1", server_port))
        threading.Thread(target=pump, args=(client, server, False), daemon=True).start()
        threading.Thread(target=pump, args=(server, client, True), daemon=True).start()


threading.Thread(target=serve, daemon=True).start()
result = subprocess.run(command, capture_output=True, text=True)
listener.close()
print(result.stdout, result.stderr, file=sys.stderr)
if result.returncode != 0:
    sys.exit(f"the client failed ({result.returncode})")

seed = bytearray([mode])
for data in reads:
    for at in range(0, len(data), 255):
        part = data[at:at + 255]
        seed.append(len(part))
        seed += part
path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "seeds", "db_wire", name)
with open(path, "wb") as f:
    f.write(seed)
print(f"{name}: {len(reads)} reads, {len(seed)} bytes")
