"""Capture golden LoginReply bytes from the real C# SmilegateAuth server.

Builds a LoginRequest using the layout we believe C# marshals, sends it, and records the
reply. If C# echoes back the username we put in, our request layout matches its
[StructLayout] exactly -- that is the point of the exercise, not just the reply bytes.
"""

import socket
import struct
import sys

MAGIC = 0xF1
LOGIN_REQUEST = 0x0312


def fixed(value: str, size: int) -> bytes:
    raw = value.encode("utf-8")[: size - 1]
    return raw + b"\0" * (size - len(raw))


def build_request(username: str, password: str, unknown: int = 0, unknown2: str = "") -> bytes:
    body = struct.pack("<i", unknown) + fixed(unknown2, 32) + fixed(username, 50) + fixed(password, 32)
    total = 5 + len(body)
    return struct.pack("<BHH", MAGIC, total, LOGIN_REQUEST) + body


def main() -> int:
    host, port = "127.0.0.1", 10106
    username, password = sys.argv[1], sys.argv[2]

    request = build_request(username, password)
    print(f"request: {len(request)} bytes")

    with socket.create_connection((host, port), timeout=5) as sock:
        sock.sendall(request)
        reply = b""
        while len(reply) < 1095:
            chunk = sock.recv(4096)
            if not chunk:
                break
            reply += chunk

    print(f"reply: {len(reply)} bytes")
    magic, length, packet_id = struct.unpack_from("<BHH", reply, 0)
    result, unknown = struct.unpack_from("<ii", reply, 5)
    gap = reply[13:21]
    echoed = reply[21:71].split(b"\0")[0].decode()
    token = reply[71:1095].split(b"\0")[0].decode()

    print(f"  magic={magic:#04x} length={length} id={packet_id:#06x}")
    print(f"  result={result} unknown={unknown} gap={gap.hex()}")
    print(f"  username={echoed!r}  (sent {username!r})")
    print(f"  token={token!r}")

    assert echoed == username, "C# did not echo our username -> request layout is wrong"

    out = sys.argv[3]
    with open(out, "w") as handle:
        handle.write(f"# request  ({len(request)} bytes) username={username!r} password={password!r}\n")
        handle.write(request.hex() + "\n")
        handle.write(f"# reply    ({len(reply)} bytes) token={token!r}\n")
        handle.write(reply.hex() + "\n")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
