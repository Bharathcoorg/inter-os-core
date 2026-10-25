"""Truth contracts for the browser's media transfer paths.

An upload is complete only when the gateway reports how many bytes it
received. A speed test is a measurement only when the clock and the
gateway's own byte count produced it. A download can be opened only when
the platform's index names the media type that was written. Each test
below pins one of those claims to the code that makes it.

WHY most of these tests speak HTTP: the gateway's media routes touch no
QEMU state, so `StudioAPIHandler` is started in-process on an ephemeral
loopback port, exactly as `tests/test_studio_security.py` and
`tests/test_adversarial_security.py` do. The routes are then exercised
with real requests rather than searched for in the source. The Rust app
runs on the guest and cannot be executed here; those assertions are
marked as static and are scoped to the function that implements the
claim.
"""

import ast
import hashlib
import http.client
import importlib.util
import io
import json
import operator
import os
import re
import socket
import struct
import threading
import urllib.parse
import wave
import zlib
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
APP_MAIN = "apps/browser/src/main.rs"
APP_LIB = "apps/browser/src/lib.rs"

_GATEWAY = None


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def code_only(source: str) -> str:
    """Drops Rust comments so a rule is asserted against the program.

    WHY: the comments explain the refusals these tests pin, and several
    quote the numbers and phrases the tests look for.
    """
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def gateway():
    """Imports the Studio gateway once.

    WHY importlib and not `import server`: the test must load the file in
    this repository, not a module that happens to be named `server` on
    `sys.path`.
    """
    global _GATEWAY
    if _GATEWAY is None:
        spec = importlib.util.spec_from_file_location(
            "interos_studio_gateway_contract", ROOT / "emulator" / "server.py"
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _GATEWAY = module
    return _GATEWAY


class _Studio:
    """A gateway handler bound to an ephemeral loopback port."""

    def __init__(self, module, server):
        self.gateway = module
        self.server = server
        self.host = "127.0.0.1"
        self.port = server.server_address[1]

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection(self.host, self.port, timeout=30)
        try:
            conn.request(method, path, body=body, headers=headers or {})
            response = conn.getresponse()
            data = response.read()
            return (
                response.status,
                {key.lower(): value for key, value in response.getheaders()},
                data,
            )
        finally:
            conn.close()

    def get(self, path):
        return self.request("GET", path)

    def post(self, path, body, headers=None):
        return self.request("POST", path, body=body, headers=headers)

    def invalid_content_length_post(self, path):
        """Sends a POST whose Content-Length is not a number at all."""
        conn = http.client.HTTPConnection(self.host, self.port, timeout=15)
        try:
            conn.putrequest("POST", path)
            conn.putheader("Host", f"{self.host}:{self.port}")
            conn.putheader("Content-Type", "application/octet-stream")
            conn.putheader("Content-Length", "not-a-number")
            conn.putheader("Connection", "close")
            conn.endheaders()
            response = conn.getresponse()
            return response.status, response.read()
        finally:
            conn.close()

    def raw_post(self, path, body, headers=None):
        """POSTs a body while reading the reply concurrently.

        WHY raw sockets: an oversized upload is refused after the gateway
        drains 1 MiB, so it closes the connection with unread bytes still
        in flight. A client that writes the whole body before reading sees
        a reset and loses the 413. Reading while writing captures the
        refusal the gateway actually sent.
        """
        request_headers = {
            "Host": f"{self.host}:{self.port}",
            "Content-Type": "application/octet-stream",
            "Content-Length": str(len(body)),
            "Connection": "close",
        }
        request_headers.update(headers or {})
        head = f"POST {path} HTTP/1.1\r\n" + "".join(
            f"{key}: {value}\r\n" for key, value in request_headers.items()
        ) + "\r\n"

        sock = socket.create_connection((self.host, self.port), timeout=30)
        collected = {}

        def reader():
            chunks = []
            try:
                sock.settimeout(30)
                while True:
                    chunk = sock.recv(65536)
                    if not chunk:
                        break
                    chunks.append(chunk)
            except OSError as error:  # a reset after the reply is harmless
                collected["read_error"] = error
            collected["data"] = b"".join(chunks)

        def writer():
            try:
                sock.sendall(head.encode("ascii") + body)
            except OSError as error:
                collected["write_error"] = error

        read_thread = threading.Thread(target=reader, daemon=True)
        write_thread = threading.Thread(target=writer, daemon=True)
        read_thread.start()
        write_thread.start()
        read_thread.join(60)
        write_thread.join(60)
        sock.close()

        raw = collected.get("data", b"")
        header_block, separator, payload = raw.partition(b"\r\n\r\n")
        assert separator and header_block.startswith(b"HTTP/"), (
            f"the gateway sent no HTTP reply (read={collected.get('read_error')!r}, "
            f"write={collected.get('write_error')!r})"
        )
        return int(header_block.split(b" ", 2)[1]), payload


@pytest.fixture(scope="module")
def studio():
    module = gateway()

    class QuietHandler(module.StudioAPIHandler):
        def log_message(self, fmt, *args):  # noqa: A003 - base-class signature
            pass

    server = module.ThreadedTCPServer(("127.0.0.1", 0), QuietHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield _Studio(module, server)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


# -----------------------------------------------------------------------------
# Reading the Rust side without executing it
# -----------------------------------------------------------------------------
def rust_function(source: str, name: str) -> str:
    """Returns the body of `fn name(...)`, braces included.

    WHY brace matching and not a source grep: two functions in the same
    file can contain the same arm, and an assertion that searches the whole
    file cannot say which function it found. Matching the body keeps each
    claim attached to its own function and tolerates reformatting.
    """
    match = re.search(r"\bfn\s+" + re.escape(name) + r"\s*\(", source)
    assert match is not None, f"fn {name} is missing"
    start = source.find("{", match.end())
    assert start >= 0, f"fn {name} has no body"
    depth = 0
    pos = start
    while pos < len(source):
        char = source[pos]
        if char == '"':
            pos += 1
            while pos < len(source):
                if source[pos] == "\\":
                    pos += 2
                    continue
                if source[pos] == '"':
                    break
                pos += 1
        elif source.startswith("//", pos):
            newline = source.find("\n", pos)
            pos = len(source) if newline < 0 else newline
            continue
        elif source.startswith("/*", pos):
            end = source.find("*/", pos + 2)
            pos = len(source) if end < 0 else end + 1
        elif char == "'":
            # A lifetime such as 'a has no closing quote; a char literal does.
            close = source.find("'", pos + 1)
            if 0 <= close - pos <= 4:
                pos = close
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start:pos + 1]
        pos += 1
    raise AssertionError(f"fn {name} is not terminated")


def _compact(source: str) -> str:
    return re.sub(r"\s+", " ", source).strip()


def _rust_int(expr: str) -> int:
    tree = ast.parse(expr.strip(), mode="eval")

    def walk(node):
        if isinstance(node, ast.Expression):
            return walk(node.body)
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.BinOp):
            left, right = walk(node.left), walk(node.right)
            if isinstance(node.op, ast.Mult):
                return operator.mul(left, right)
            if isinstance(node.op, ast.Add):
                return operator.add(left, right)
            if isinstance(node.op, ast.Sub):
                return operator.sub(left, right)
        raise ValueError(f"unsupported constant expression {expr!r}")

    return walk(tree)


def rust_usize_const(source: str, name: str) -> int:
    match = re.search(
        r"\bconst\s+" + re.escape(name) + r"\s*:\s*usize\s*=\s*([^;]+);", source
    )
    assert match is not None, f"const {name}: usize is missing"
    return _rust_int(match.group(1))


def rust_test_fixtures(source: str):
    block = source[source.index("const TEST_FIXTURES"):]
    block = block[: block.index("];")]
    return re.findall(r'b"([a-z0-9.]+)",\s*b"([a-z0-9/+.-]+)"', block)


# -----------------------------------------------------------------------------
# Decoders: a fixture is real only if its own format parses
# -----------------------------------------------------------------------------
def _decode_png(data: bytes):
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    pos = 8
    kinds = []
    width = height = None
    idat = bytearray()
    while pos < len(data):
        length = int.from_bytes(data[pos:pos + 4], "big")
        kind = data[pos + 4:pos + 8]
        payload = data[pos + 8:pos + 8 + length]
        stored_crc = int.from_bytes(data[pos + 8 + length:pos + 12 + length], "big")
        assert len(payload) == length
        assert zlib.crc32(kind + payload) & 0xFFFFFFFF == stored_crc, (
            f"the {kind!r} chunk has a bad CRC"
        )
        kinds.append(kind)
        if kind == b"IHDR":
            width, height, depth, colour, _, _, _ = struct.unpack(">IIBBBBB", payload)
            assert (depth, colour) == (8, 2), "the fixture is not 8-bit RGB"
        elif kind == b"IDAT":
            idat.extend(payload)
        pos += 12 + length
    assert kinds[0] == b"IHDR" and kinds[-1] == b"IEND"
    raw = zlib.decompress(bytes(idat))
    stride = 1 + width * 3
    assert len(raw) == height * stride, "the inflated image is not its declared size"
    assert all(raw[row * stride] == 0 for row in range(height)), "a row uses an unknown filter"
    return width, height


_JPEG_START_OF_FRAME = {0xC0, 0xC1, 0xC2, 0xC3, 0xC5, 0xC6, 0xC7,
                        0xC9, 0xCA, 0xCB, 0xCD, 0xCE, 0xCF}


def _jpeg_extent(data: bytes, start: int = 0) -> int:
    """Returns the offset just past the EOI marker of one JPEG stream."""
    assert data[start:start + 2] == b"\xff\xd8", "the stream does not start with SOI"
    pos = start + 2
    dimensions = None
    while pos < len(data):
        assert data[pos] == 0xFF, f"expected a marker at offset {pos}"
        while pos < len(data) and data[pos] == 0xFF:
            pos += 1
        marker = data[pos]
        pos += 1
        if marker == 0xD9:
            return pos
        if marker == 0x01 or 0xD0 <= marker <= 0xD7:
            continue
        segment_length = int.from_bytes(data[pos:pos + 2], "big")
        assert segment_length >= 2 and pos + segment_length <= len(data)
        if marker in _JPEG_START_OF_FRAME:
            assert data[pos + 2] == 8, "the fixture is not 8-bit precision"
            height = int.from_bytes(data[pos + 3:pos + 5], "big")
            width = int.from_bytes(data[pos + 5:pos + 7], "big")
            dimensions = (width, height)
        if marker == 0xDA:  # start of scan: entropy data follows
            pos += segment_length
            while pos + 1 < len(data):
                if data[pos] == 0xFF and data[pos + 1] == 0xD9:
                    assert dimensions is not None, "the scan has no frame header"
                    return pos + 2
                pos += 1
            raise AssertionError("the JPEG stream has no EOI")
        pos += segment_length
    raise AssertionError("the JPEG stream ends without EOI")


def _decode_gif(data: bytes):
    assert data[:6] == b"GIF89a"
    width, height = struct.unpack("<HH", data[6:10])
    packed = data[10]
    pos = 13
    if packed & 0x80:
        pos += 3 * (2 << (packed & 0x07))
    # Graphic-control and comment extensions may precede the first frame.
    while data[pos] == 0x21:
        pos += 2  # extension introducer and label
        while True:
            size = data[pos]
            pos += 1
            if size == 0:
                break
            pos += size
    assert data[pos] == 0x2C, "the fixture has no image descriptor"
    _, _, image_width, image_height = struct.unpack("<HHHH", data[pos + 1:pos + 9])
    image_packed = data[pos + 9]
    pos += 10
    if image_packed & 0x80:
        pos += 3 * (2 << (image_packed & 0x07))
    assert data[pos] >= 2, "the LZW minimum code size is invalid"
    pos += 1
    while True:
        size = data[pos]
        pos += 1
        if size == 0:
            break
        pos += size
    assert data[pos] == 0x3B and pos == len(data) - 1, "the stream has no trailer"
    return image_width or width, image_height or height


def _decode_bmp(data: bytes):
    assert data[:2] == b"BM"
    assert int.from_bytes(data[2:6], "little") == len(data)
    pixel_offset = int.from_bytes(data[10:14], "little")
    assert int.from_bytes(data[14:18], "little") == 40, "the DIB header is not BITMAPINFOHEADER"
    width = int.from_bytes(data[18:22], "little", signed=True)
    height = int.from_bytes(data[22:26], "little", signed=True)
    planes, bits = struct.unpack("<HH", data[26:30])
    compression = int.from_bytes(data[30:34], "little")
    image_size = int.from_bytes(data[34:38], "little")
    assert (planes, bits, compression) == (1, 24, 0), "the fixture is not uncompressed 24-bit"
    stride = ((width * 3 + 3) // 4) * 4
    assert image_size == stride * abs(height)
    assert pixel_offset + image_size == len(data)
    return width, height


def _decode_ppm(data: bytes):
    match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+(\d+)\s", data)
    assert match is not None, "the fixture has no P6 header"
    width, height, maximum = (int(value) for value in match.groups())
    assert maximum == 255
    assert len(data) - match.end() == width * height * 3
    return width, height


def _decode_qoi(data: bytes):
    assert data[:4] == b"qoif"
    width = int.from_bytes(data[4:8], "big")
    height = int.from_bytes(data[8:12], "big")
    channels = data[12]
    assert channels in (3, 4)
    assert data[13] in (0, 1)
    previous = (0, 0, 0, 255)
    index = [(0, 0, 0, 0)] * 64
    pixels = 0
    run = 0
    pos = 14
    end = len(data) - 8
    while pos < end:
        if run > 0:
            run -= 1
        else:
            tag = data[pos]
            pos += 1
            if tag == 0xFE:
                previous = (data[pos], data[pos + 1], data[pos + 2], previous[3])
                pos += 3
            elif tag == 0xFF:
                previous = tuple(data[pos:pos + 4])
                pos += 4
            elif tag >> 6 == 0:
                previous = index[tag]
            elif tag >> 6 == 1:
                previous = (
                    (previous[0] + ((tag >> 4) & 3) - 2) & 0xFF,
                    (previous[1] + ((tag >> 2) & 3) - 2) & 0xFF,
                    (previous[2] + (tag & 3) - 2) & 0xFF,
                    previous[3],
                )
            elif tag >> 6 == 2:
                delta_green = (tag & 0x3F) - 32
                next_byte = data[pos]
                pos += 1
                previous = (
                    (previous[0] + delta_green + ((next_byte >> 4) & 0x0F) - 8) & 0xFF,
                    (previous[1] + delta_green) & 0xFF,
                    (previous[2] + delta_green + (next_byte & 0x0F) - 8) & 0xFF,
                    previous[3],
                )
            else:
                run = tag & 0x3F
            index[(previous[0] * 3 + previous[1] * 5 + previous[2] * 7 + previous[3] * 11) % 64] = previous
        pixels += 1
    assert pos == end, "the QOI payload does not end on an op boundary"
    assert data[end:] == b"\x00" * 7 + b"\x01", "the QOI end marker is missing"
    assert pixels == width * height, f"decoded {pixels} pixels, expected {width * height}"
    return width, height


def _decode_wav(data: bytes):
    with wave.open(io.BytesIO(data), "rb") as wav:
        assert wav.getnchannels() == 1
        assert wav.getsampwidth() == 2
        assert wav.getframerate() == 8000
        frames = wav.readframes(wav.getnframes())
        frame_count = wav.getnframes()
    assert len(frames) == frame_count * 2
    return frame_count


def _decode_mp3(data: bytes):
    pos = 0
    frames = 0
    while pos < len(data):
        assert data[pos] == 0xFF and data[pos + 1] & 0xE0 == 0xE0, "no frame sync"
        version = (data[pos + 1] >> 3) & 3
        layer = (data[pos + 1] >> 1) & 3
        bitrate_index = (data[pos + 2] >> 4) & 0x0F
        sample_index = (data[pos + 2] >> 2) & 3
        padding = (data[pos + 2] >> 1) & 1
        assert version == 3 and layer == 1, "not MPEG-1 Layer III"
        assert bitrate_index == 9 and sample_index == 0 and padding == 0, (
            "not 128 kbps at 44.1 kHz"
        )
        frame_length = 144 * 128000 // 44100
        assert pos + frame_length <= len(data), "a frame runs past the end"
        pos += frame_length
        frames += 1
    return frames


def _mp4_boxes(data: bytes, start: int = 0, end: int = None):
    end = len(data) if end is None else end
    pos = start
    boxes = []
    while pos < end:
        size = int.from_bytes(data[pos:pos + 4], "big")
        kind = data[pos + 4:pos + 8]
        assert size >= 8 and pos + size <= end, "a box does not fit its parent"
        boxes.append((kind, pos, size))
        pos += size
    assert pos == end, "the boxes do not tile their parent"
    return boxes


def _decode_mp4(data: bytes):
    boxes = _mp4_boxes(data)
    assert [kind for kind, _, _ in boxes] == [b"ftyp", b"moov"]
    assert boxes[0][2] == 28 and data[8:12] == b"isom"
    moov_start, moov_size = boxes[1][1], boxes[1][2]
    assert [kind for kind, _, _ in _mp4_boxes(data, moov_start + 8, moov_start + moov_size)] == [
        b"mvhd"
    ]
    # The fixture is documented as a container with no tracks. The guest has
    # no H.264 decoder, so this is a structural file check, not a playback
    # claim: there is nothing here that could be played and nothing pretends
    # there is.
    assert b"trak" not in data and b"mdat" not in data
    return len(boxes)


def _decode_raw(data: bytes):
    # RAW has no container signature. The video player's decoder expects
    # 36x72 RGB24 frames, so the exact length is the whole contract.
    assert len(data) == 36 * 72 * 3
    return 36, 72


def _decode_mjpeg(data: bytes):
    first = _jpeg_extent(data)
    assert first == len(data) // 2, "the first JPEG frame is not half the stream"
    second = _jpeg_extent(data, first)
    assert second == len(data), "the second JPEG frame does not end the stream"
    return 2


_MAGIC = {
    "sample.png": b"\x89PNG\r\n\x1a\n",
    "sample.jpg": b"\xff\xd8",
    "sample.gif": b"GIF89a",
    "sample.bmp": b"BM",
    "sample.ppm": b"P6\n",
    "sample.qoi": b"qoif",
    "sample.wav": b"RIFF",
    "sample.mp3": b"\xff\xfb",
    "sample.mjpeg": b"\xff\xd8",
    "sample.mp4": None,  # the type is the ftyp box at offset 4
    "sample.raw": None,  # raw frames have no container signature
}

_DECODERS = {
    "sample.png": _decode_png,
    "sample.jpg": _jpeg_extent,
    "sample.gif": _decode_gif,
    "sample.bmp": _decode_bmp,
    "sample.ppm": _decode_ppm,
    "sample.qoi": _decode_qoi,
    "sample.wav": _decode_wav,
    "sample.mp3": _decode_mp3,
    "sample.mjpeg": _decode_mjpeg,
    "sample.mp4": _decode_mp4,
    "sample.raw": _decode_raw,
}


# -----------------------------------------------------------------------------
# Speed test
# -----------------------------------------------------------------------------
def test_the_speedtest_payload_is_exactly_the_size_the_request_names(studio):
    """A speed test that pads or truncates its payload measures its own bug.

    WHY this is runtime and not a source scan: the size is the measurement.
    The route reads `bytes=N` from the query and sends that many bytes, so
    the device's byte count can be checked against a number the gateway
    chose rather than one the device inferred.
    """
    module = studio.gateway
    sizes = (module.SPEEDTEST_MIN_BYTES, 1024, 65536, module.SPEEDTEST_MAX_BYTES)
    for size in sizes:
        status, headers, payload = studio.get(f"/api/speedtest/download?bytes={size}")
        assert status == 200, f"a {size}-byte request was refused"
        assert len(payload) == size, f"a {size}-byte request produced {len(payload)} bytes"
        assert payload == module.speedtest_payload(size), "the payload is not the measured pattern"
        assert headers["content-length"] == str(size)
        assert headers["x-interos-speedtest-bytes"] == str(size)
        assert headers["content-type"] == "application/octet-stream"

    # Deterministic across requests, not merely within one helper call.
    _, _, first = studio.get("/api/speedtest/download?bytes=65536")
    _, _, second = studio.get("/api/speedtest/download?bytes=65536")
    assert first == second

    # A request with no size uses the route's declared default, not a guess.
    status, headers, payload = studio.get("/api/speedtest/download")
    assert status == 200 and len(payload) == 65536
    assert headers["x-interos-speedtest-bytes"] == "65536"

    # The route refuses a size outside its own bounds instead of clamping.
    for outside in (
        module.SPEEDTEST_MIN_BYTES - 1,
        0,
        -1,
        module.SPEEDTEST_MAX_BYTES + 1,
        "not-a-number",
    ):
        status, _, body = studio.get(f"/api/speedtest/download?bytes={outside}")
        assert status == 400, f"bytes={outside} was served instead of refused"
        assert json.loads(body)["success"] is False


def test_the_speedtest_upload_route_confirms_the_exact_payload(studio):
    """The gateway's count is the only proof the device's bytes travelled.

    WHY the body is the app's own pattern: the browser fills its 64 KiB
    test buffer with `(index * 31 + 7) & 0xFF`, so the hash checked here is
    the hash of what the device will actually send.
    """
    main = read(APP_MAIN)
    expected = rust_usize_const(main, "SPEEDTEST_UPLOAD_BYTES")
    assert expected == 64 * 1024, "the speed test's payload size changed"
    body = bytes((index * 31 + 7) & 0xFF for index in range(expected))

    status, _, data = studio.post(
        "/api/speedtest/upload", body, {"Content-Type": "application/octet-stream"}
    )
    assert status == 200
    answer = json.loads(data)
    assert answer["success"] is True
    assert answer["received_bytes"] == expected
    assert answer["sha256"] == hashlib.sha256(body).hexdigest(), (
        "the gateway's hash is not the hash of the body that was sent"
    )


# -----------------------------------------------------------------------------
# Browser upload
# -----------------------------------------------------------------------------
def test_the_upload_route_reports_the_bytes_it_actually_received(studio):
    """The gateway's count is the only proof an upload travelled.

    WHY: the device can say how many bytes it handed to the transport, but
    only the receiver can say how many arrived. The route answers with the
    length of the body it read, and the browser compares that answer to the
    file it sent.
    """
    body = bytes((index * 17 + 3) & 0xFF for index in range(64 * 1024))
    status, _, data = studio.post(
        "/api/browser/upload",
        body,
        {
            "Content-Type": "image/png",
            "X-Interos-Filename": "../contract upload.txt",
        },
    )
    assert status == 200
    answer = json.loads(data)
    assert answer["received_bytes"] == len(body)
    assert answer["sha256"] == hashlib.sha256(body).hexdigest()
    assert answer["content_type"] == "image/png"
    # The declared name is reduced to one safe component with the extension
    # the media type implies; the traversal in it cannot escape the sink.
    assert answer["stored_name"].endswith("_contract_upload.png"), answer["stored_name"]
    stored_path = Path(answer["stored_path"])
    try:
        assert stored_path.read_bytes() == body, "the bytes written are not the bytes received"
    finally:
        stored_path.unlink(missing_ok=True)

    # A non-numeric Content-Length is refused before anything is buffered.
    status, body = studio.invalid_content_length_post("/api/browser/upload")
    assert status == 400
    assert json.loads(body)["success"] is False


def test_an_upload_at_the_limit_is_accepted_and_one_byte_over_is_refused(studio):
    """The cap is inclusive at 4 MiB and refuses the next byte.

    WHY both sides of the boundary: a bound that is off by one either
    refuses a legal upload or accepts one the storage sink was never sized
    for. The over-limit body is a real body, not a declared length, so the
    route has to make its decision before writing anything.
    """
    module = studio.gateway
    limit = rust_usize_const(read(APP_LIB), "MAX_UPLOAD_BYTES")
    assert limit == module.MAX_UPLOAD_BYTES == 4 * 1024 * 1024

    at_limit = b"L" * limit
    status, _, data = studio.post(
        "/api/browser/upload",
        at_limit,
        {"Content-Type": "application/octet-stream", "X-Interos-Filename": "edge.bin"},
    )
    assert status == 200
    answer = json.loads(data)
    assert answer["received_bytes"] == limit
    stored_path = Path(answer["stored_path"])
    try:
        assert stored_path.read_bytes() == at_limit
    finally:
        stored_path.unlink(missing_ok=True)

    marker = b"OVER-LIMIT-UPLOAD-MARKER"
    over_limit = marker + b"Z" * (limit + 1 - len(marker))
    assert len(over_limit) == limit + 1
    upload_dir = Path(module.UPLOAD_DIR)
    before = set(os.listdir(upload_dir)) if upload_dir.is_dir() else set()

    status, payload = studio.raw_post(
        "/api/browser/upload",
        over_limit,
        {"X-Interos-Filename": "too-big.bin"},
    )
    assert status == 413, f"a {len(over_limit)}-byte body was answered with {status}"
    assert b"received_bytes" not in payload, "the refusal claimed a received byte count"

    after = set(os.listdir(upload_dir)) if upload_dir.is_dir() else set()
    for name in after - before:
        with open(upload_dir / name, "rb") as written:
            assert not written.read(len(marker)).startswith(marker), (
                "the refused body was written to the upload sink"
            )


def test_the_browser_builds_the_upload_the_gateway_accepts():
    """The request the device sends and the route above are one contract.

    WHY static: `perform_upload` opens a TCP connection through the
    kernel's `std::net` on the guest. It has no host-side entry point, so
    it cannot be called from this test process; the HTTP test above pins
    the receiving end and this test pins the sender to the same route,
    header, and confirmation field.
    """
    module = gateway()
    lib = read(APP_LIB)
    main = read(APP_MAIN)

    assert rust_usize_const(lib, "MAX_UPLOAD_BYTES") == module.MAX_UPLOAD_BYTES

    formatter = _compact(rust_function(lib, "format_upload_request"))
    assert "POST /api/browser/upload HTTP/1.1" in formatter
    assert "Content-Length:" in formatter
    assert "append_decimal(out, n, content_length as u64)" in formatter
    assert "X-Interos-Filename:" in formatter

    perform = _compact(rust_function(main, "perform_upload"))
    assert "browser::format_upload_request(" in perform
    assert "entry.length > browser::MAX_UPLOAD_BYTES as u64" in perform, (
        "the browser sends a file the gateway has already refused by size"
    )
    assert 'json_u64_field(parsed.body, b"received_bytes")' in perform, (
        "the browser does not read the gateway's byte count"
    )
    assert re.search(
        r"Some\(confirmed\) if confirmed == entry\.length", perform
    ), "the browser marks an upload complete without matching the sent length"
    assert "self.upload_sent = outcome.sent" in perform, (
        "the bytes handed to the transport are not kept separately from the confirmed count"
    )
    assert "self.upload_confirmed = confirmed" in perform
    assert "TransferState::Complete" in perform


# -----------------------------------------------------------------------------
# Rates and clock
# -----------------------------------------------------------------------------
def test_the_browser_reports_no_rate_it_did_not_measure():
    """A rate needs a byte count, a duration, and a clock reading.

    WHY static: these are guest-only arithmetic and event-loop functions.
    Each assertion is scoped to one function so the download-clock branch
    cannot be satisfied by the upload branch's own `-1000`, which is what
    the previous whole-file assertion did.
    """
    source = read(APP_MAIN)
    rate = _compact(code_only(rust_function(source, "rate_per_second")))
    assert "if ms == 0 { return 0; }" in rate, (
        "a transfer inside one tick is divided by zero instead of reported as unmeasured"
    )
    assert "(bytes as u64).saturating_mul(1000) / ms" in rate

    measured = _compact(code_only(rust_function(source, "download_rate")))
    assert re.search(
        r"let \(Some\(start\), Some\(end\)\) = \(start, end\) else "
        r"\{ return Err\(-1000\); \};",
        measured,
    ), "a download without two clock readings is not reported as unmeasured"
    assert "outcome.truncated" in measured, (
        "a truncated response is accepted as a measured download"
    )
    assert "parsed.body.len() != expected" in measured, (
        "a download is accepted without checking that the declared payload arrived"
    )
    assert "rate_per_second(expected, ms)" in measured

    speed = code_only(rust_function(source, "perform_speed_test"))
    download_start = speed.index("match self.download_rate(")
    latency_start = speed.index("format_speedtest_download_request(SPEEDTEST_LATENCY_BYTES")
    upload_start = speed.index("browser::format_speedtest_upload_request(")
    download_arm = _compact(speed[download_start:latency_start])
    assert re.search(
        r"Err\(code\) => \{ self\.speed\.download = TransferState::Failed; "
        r"self\.speed\.error = code; \}",
        download_arm,
    ), "the download's clock failure is not propagated to the screen"
    assert "self.speed.error = -1000" not in download_arm, (
        "the download branch reports a clock failure it did not detect"
    )

    upload_block = _compact(speed[upload_start:])
    assert "confirmed == SPEEDTEST_UPLOAD_BYTES as u64" in upload_block, (
        "the upload rate is computed without the gateway's confirmation"
    )
    assert re.search(
        r"\(None, _, _\) => \{ self\.speed\.upload = TransferState::Failed; "
        r"self\.speed\.error = -1000; \}",
        upload_block,
    ), "an unavailable upload clock is not reported as such"

    # The screen prints the measured units, not a nominal link rate.
    assert "MiB/s" in source and "KiB/s" in source and "B/s" in source
    assert "Mbps" not in source, "the speed screen reports a unit the device never measures"

    units = _compact(code_only(rust_function(source, "append_rate_text")))
    assert "bps >= 1024 * 1024" in units and "bps >= 1024" in units


# -----------------------------------------------------------------------------
# Media fixtures
# -----------------------------------------------------------------------------
def test_the_media_fixtures_are_real_files_with_their_declared_types(studio):
    """Every fixture must decode as the type it claims.

    WHY: the fixtures exist so Gallery, Music and Video can be tested with
    known-good input. A renamed blob would make a decoder failure look like
    a bad sample. Each fixture is fetched over the route the guest uses and
    parsed by its own format, not merely checked for a prefix.

    `sample.mp4` is documented as a container with no tracks and the guest
    has no H.264 decoder, so it is verified structurally and no playback is
    claimed. `sample.raw` has no container: its exact frame length is its
    contract.
    """
    module = studio.gateway
    listed = rust_test_fixtures(read(APP_MAIN))
    assert {name for name, _ in listed} == set(module._MEDIA_FIXTURES)
    assert set(_MAGIC) == set(_DECODERS) == set(module._MEDIA_FIXTURES)

    for name, declared_type in listed:
        status, headers, data = studio.get(f"/api/media/fixture/{urllib.parse.quote(name)}")
        assert status == 200, f"{name} is not served"
        assert headers["content-type"] == declared_type, (
            f"{name} is served as {headers['content-type']}, not {declared_type}"
        )
        assert headers["x-interos-filename"] == name
        assert int(headers["content-length"]) == len(data)
        assert data == module.media_fixture(name)[0], (
            "the route serves bytes other than the fixture table's"
        )

        magic = _MAGIC[name]
        if magic is not None:
            assert data.startswith(magic), f"{name} does not begin with its format's signature"
        if name == "sample.mp4":
            assert data[4:8] == b"ftyp"
        _DECODERS[name](data)

    # A name outside the allowlist is not a path into the host filesystem.
    assert module.media_fixture("../../etc/passwd") is None
    assert module.media_fixture("missing.png") is None
    for refused in ("missing.png", "../../etc/passwd", "%2e%2e%2fetc%2fpasswd"):
        status, _, body = studio.get(f"/api/media/fixture/{refused}")
        assert status == 404, f"fixture {refused!r} was served"
        assert json.loads(body)["success"] is False


def test_the_proxy_serves_its_own_fixtures_without_an_outbound_fetch(studio):
    """The guest browses through the proxy, and the fixtures live on the gateway.

    WHY this is a local read: the SSRF boundary refuses loopback by design,
    so the proxy recognises its own allowlisted fixture URL and serves the
    bytes from the same table. Every other loopback address stays refused.
    """
    module = studio.gateway

    for host in ("127.0.0.1", "localhost"):
        for name in module._MEDIA_FIXTURES:
            url = f"http://{host}:4300/api/media/fixture/{name}"
            assert module.local_fixture_target(url) == name, f"{url} is not served locally"
            status, headers, data = studio.get(
                "/api/browser/proxy?url=" + urllib.parse.quote(url, safe="")
            )
            assert status == 200, f"the proxy refused its own fixture {url}"
            assert headers["x-interos-filename"] == name
            assert data == module.media_fixture(name)[0]

    # Each refusal below changes exactly one component of a URL the helper
    # has just been shown to accept; the message names the variable.
    url = "http://127.0.0.1:4300/api/media/fixture/sample.png"
    assert module.local_fixture_target(url) == "sample.png"
    refused = {
        "host outside the loopback allowlist": "http://127.0.0.2:4300/api/media/fixture/sample.png",
        "host is not loopback": "http://example.com/api/media/fixture/sample.png",
        "scheme is not http": "https://127.0.0.1:4300/api/media/fixture/sample.png",
        "port is not the gateway's": "http://127.0.0.1:4301/api/media/fixture/sample.png",
        "path is not the fixture route": "http://127.0.0.1:4300/",
        "path names another route": "http://127.0.0.1:4300/api/browser/proxy?url=x",
        "query string is present": "http://127.0.0.1:4300/api/media/fixture/sample.png?x=1",
        "fragment is present": "http://127.0.0.1:4300/api/media/fixture/sample.png#x",
        "name escapes the table": "http://127.0.0.1:4300/api/media/fixture/../../etc/passwd",
        "encoded name escapes the table": "http://127.0.0.1:4300/api/media/fixture/%2e%2e%2fetc%2fpasswd",
        "name is not allowlisted": "http://127.0.0.1:4300/api/media/fixture/missing.png",
    }
    for variable, candidate in refused.items():
        assert module.local_fixture_target(candidate) is None, (
            f"accepted although its {variable}: {candidate}"
        )

    # A loopback URL that is not the fixture route reaches the SSRF check
    # and is refused there, not mistaken for a local fixture.
    status, _, body = studio.get(
        "/api/browser/proxy?url="
        + urllib.parse.quote("http://127.0.0.1:4300/api/status", safe="")
    )
    assert status == 403
    assert json.loads(body)["success"] is False

    # The local branch must precede the SSRF check, or the fixture fetch
    # above could only work by weakening the outbound boundary.
    source = read("emulator/server.py")
    proxy = source[source.index("if parsed_path.path == '/api/browser/proxy':"):]
    branch = proxy[: proxy.index("safe, reason = is_safe_proxy_url(target_url)")]
    assert "fixture_name = local_fixture_target(target_url)" in branch
    assert "'X-Interos-Filename'" in branch, "a fixture download does not name its file"


def test_the_app_offers_only_fixtures_the_gateway_serves():
    """The tools list and the gateway's allowlist must name the same files."""
    module = gateway()
    main = read(APP_MAIN)
    listed = rust_test_fixtures(main)
    assert len(listed) == len(module._MEDIA_FIXTURES), "the app and gateway lists differ in length"
    assert {name for name, _ in listed} == set(module._MEDIA_FIXTURES)
    for name, content_type in listed:
        assert module._MEDIA_FIXTURES[name][0] == content_type, (
            f"{name} is offered as {content_type} but served as {module._MEDIA_FIXTURES[name][0]}"
        )

    prefix = re.search(
        r'const FIXTURE_URL_PREFIX:\s*&\[u8\]\s*=\s*b"([^"]+)"', main
    )
    assert prefix is not None, "the app does not name its fixture URL prefix"
    url_prefix = prefix.group(1)
    assert url_prefix == "http://127.0.0.1:4300/api/media/fixture/"
    assert module.local_fixture_target(url_prefix + "sample.png") == "sample.png"


# -----------------------------------------------------------------------------
# Downloads and the platform index
# -----------------------------------------------------------------------------
def test_a_download_remembers_the_type_the_index_gave_it():
    """Open must use the device's answer, not the origin's claim.

    WHY static: saving reads the kernel's content index and dispatching
    an intent are guest syscalls. The functions are checked one at a time
    so the index lookup and the header fallback cannot satisfy each other's
    assertions.
    """
    source = read(APP_MAIN)

    save = _compact(code_only(rust_function(source, "save_media_download")))
    assert "self.remember_download(path, content_type)" in save, (
        "a saved download is not recorded against the platform's index"
    )

    remember = _compact(code_only(rust_function(source, "remember_download")))
    assert "content_index_find_path(path" in remember, (
        "the saved file's media type is not read from the platform's index"
    )
    assert re.search(r"Some\(record\) if !record\.mime\.is_empty\(\)", remember), (
        "an empty index answer is used instead of the response header"
    )
    assert "browser::bare_mime(content_type" in remember, (
        "the response header is not reduced to a bare fallback media type"
    )

    opened = _compact(code_only(rust_function(source, "open_last_download")))
    assert "std::content::intent_dispatch(std::content::IntentKind::Mime, mime, path)" in opened
    assert "ERR_NO_HANDLER" in opened, "a file with no handler is not distinguished from a refusal"
    assert "OpenState::NoHandler" in opened

    # The action the screen draws and the tap handler that fires it are the
    # same rectangle.
    assert rust_function(source, "open_action_rect")
    draw = source[source.index("let btn = open_action_rect(f);"):]
    draw = draw[: draw.index("OpenState::NotAttempted")]
    assert re.search(
        r'let\s+label\s*=\s*b"Open"\s*;\s*let\s+lw\s*=\s*text_width\(label,\s*1\)\s*;',
        draw,
    ), "the Open control is drawn without its label"
    tap = source[source.index("if open_action_rect(&frame).contains(x, y)"):]
    tap = tap[: tap.index("return Tap::None;") + len("return Tap::None;")]
    assert "app.open_last_download()" in tap, "the drawn Open control has no tap action"
    assert 'b"No installed app handles this file\'s type."' in source

    # Opening a download is an intent dispatch, and the package must hold
    # the capability the kernel checks before it will deliver one.
    manifest = json.loads(read("apps/browser/manifest.json"))
    assert "CAP_CONTENT_INTENT" in manifest["permissions"], (
        "the browser offers Open without the capability to dispatch it"
    )


def test_the_file_manager_hands_a_download_to_its_registered_handler():
    """The registry decides what opens a file, and the panel says so.

    WHY: a file manager that names an app it can launch, or launches one
    without a registry answer, is guessing about the device. Static because
    the dispatch is a kernel syscall on the guest.
    """
    main = read("apps/file_manager/src/main.rs")

    handler = _compact(code_only(rust_function(main, "open_with_handler")))
    assert "std::content::intent_dispatch(" in handler
    assert "std::content::IntentKind::Mime" in handler
    assert "document.mime_str()" in handler and "document.path_str()" in handler

    assert re.search(r'b"Open with: "', main)
    assert re.search(r'b"no app handles this file"', main)
    assert "if open_with_handler(app)" in main, (
        "the Open action is offered without asking the registry"
    )


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
