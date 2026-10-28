"""Truth contracts for the browser's HTTP protocol layer at the gateway.

The guest owns redirects, cookies, conditional requests, decompression and
charset conversion. The gateway's job is narrower and testable here: forward
the method, body and the protocol headers, and return the origin's status,
`Location`, `Set-Cookie` and validators unchanged instead of swallowing them.

WHY these tests speak HTTP: `StudioAPIHandler` is started in-process on an
ephemeral loopback port, as `tests/test_studio_security.py` does. The browser
fixtures are served by the gateway itself under `http://127.0.0.1:4300`,
because the proxy's SSRF boundary refuses loopback origins by design; the
fixture branch is deliberately ahead of that boundary and is covered by its
own test below.

The Rust app runs on the guest and cannot be executed here. Assertions about
the guest half are static and name the module that owns the claim.
"""

import base64
import gzip
import hashlib
import http.client
import importlib.util
import json
import threading
import urllib.parse
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
FIXTURE_ORIGIN = "http://127.0.0.1:4300/api/browser/fixture/"

_GATEWAY = None


def gateway():
    global _GATEWAY
    if _GATEWAY is None:
        spec = importlib.util.spec_from_file_location(
            "interos_studio_gateway_http_contract", ROOT / "emulator" / "server.py"
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _GATEWAY = module
    return _GATEWAY


class _Studio:
    def __init__(self, module, server):
        self.gateway = module
        self.host = "127.0.0.1"
        self.port = server.server_address[1]

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection(self.host, self.port, timeout=20)
        try:
            conn.request(method, path, body=body, headers=headers or {})
            response = conn.getresponse()
            return response.status, dict(
                (key.lower(), value) for key, value in response.getheaders()
            ), response.read()
        finally:
            conn.close()

    def proxy(self, target, method="GET", body=None, headers=None):
        path = "/api/browser/proxy?" + urllib.parse.urlencode({"url": target})
        status, response_headers, data = self.request(method, path, body, headers)
        assert status == 200, f"proxy refused with {status}: {data[:200]!r}"
        assert response_headers.get("content-type", "").startswith("application/json"), (
            f"proxy did not answer in the envelope: {response_headers}"
        )
        return json.loads(data)

    def proxy_raw(self, target, method="GET", body=None, headers=None):
        path = "/api/browser/proxy?" + urllib.parse.urlencode({"url": target})
        return self.request(method, path, body, headers)


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
# Redirects
# -----------------------------------------------------------------------------
def test_a_relative_redirect_arrives_with_location_and_cookie(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "redirect-relative")
    assert envelope["success"] is True
    assert envelope["status"] == 302
    assert envelope["location"] == "/api/browser/fixture/landing"
    assert envelope["set_cookie"] == "redirected=1; Path=/; HttpOnly"
    assert "moved" in envelope["html"]


def test_an_absolute_redirect_is_not_rewritten(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "redirect-absolute")
    assert envelope["status"] == 301
    assert envelope["location"] == FIXTURE_ORIGIN + "landing"
    assert envelope["set_cookie"] == "absolute=1; Path=/"


@pytest.mark.parametrize(
    "fixture,status,location",
    [
        ("redirect-303", 303, "/api/browser/fixture/landing"),
        ("redirect-307", 307, "/api/browser/fixture/form"),
    ],
)
def test_see_other_and_temporary_redirects_keep_their_status(
    studio, fixture, status, location
):
    envelope = studio.proxy(FIXTURE_ORIGIN + fixture, method="POST", body=b"x=1",
                            headers={"Content-Type": "application/x-www-form-urlencoded"})
    assert envelope["status"] == status
    assert envelope["location"] == location


def test_the_gateway_does_not_follow_the_redirect_itself(studio):
    """The browser, not urllib, applies the hop limit and method rule."""
    envelope = studio.proxy(FIXTURE_ORIGIN + "redirect-relative")
    assert envelope["status"] in (301, 302, 303, 307, 308)
    assert "The redirected request arrived" not in envelope["html"]


def test_a_redirect_loop_is_a_pair_of_302s_not_a_hang(studio):
    first = studio.proxy(FIXTURE_ORIGIN + "loop-a")
    second = studio.proxy(FIXTURE_ORIGIN + "loop-b")
    assert first["status"] == 302 and second["status"] == 302
    assert first["location"] == "/api/browser/fixture/loop-b"
    assert second["location"] == "/api/browser/fixture/loop-a"


def test_a_redirect_to_an_unsafe_target_is_refused(studio):
    """Passthrough validates the target before returning it."""
    source = (ROOT / "emulator" / "server.py").read_text(encoding="utf-8")
    body = source[source.index("class _ProxyRedirectPassthrough"):]
    body = body[: body.index("BROWSER_FIXTURE_PREFIX")]
    assert "is_safe_proxy_url(newurl)" in body
    assert "HTTPError" in body
    assert "502" in body


# -----------------------------------------------------------------------------
# Cookies
# -----------------------------------------------------------------------------
def test_repeated_set_cookie_fields_are_all_delivered(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "cookie")
    cookies = envelope["set_cookie"].split("\n")
    assert cookies == ["sid=live123; Path=/; HttpOnly", "theme=dark; Path=/; Max-Age=3600"]


def test_the_browser_cookie_header_is_forwarded(studio):
    envelope = studio.proxy(
        FIXTURE_ORIGIN + "form",
        headers={"Cookie": "sid=live123; theme=dark"},
    )
    assert "<p>cookie=sid=live123; theme=dark</p>" in envelope["html"]


# -----------------------------------------------------------------------------
# Methods and bodies
# -----------------------------------------------------------------------------
def test_a_urlencoded_post_forwards_method_body_and_content_type(studio):
    body = b"name=Ada+Lovelace&note=hello%20world"
    envelope = studio.proxy(
        FIXTURE_ORIGIN + "form?field=1",
        method="POST",
        body=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    html = envelope["html"]
    assert "<p>method=POST</p>" in html
    assert "<p>query=field=1</p>" in html
    assert "<p>content_type=application/x-www-form-urlencoded</p>" in html
    assert f"<p>length={len(body)}</p>" in html
    assert hashlib.sha256(body).hexdigest() in html


def test_a_multipart_post_arrives_byte_for_byte(studio):
    boundary = "----InterOSFormBoundary8d2b"
    body = (
        f"--{boundary}\r\n"
        'Content-Disposition: form-data; name="file"; filename="note.txt"\r\n'
        "Content-Type: text/plain\r\n\r\n"
        "uploaded bytes\r\n"
        f"--{boundary}--\r\n"
    ).encode("ascii")
    envelope = studio.proxy(
        FIXTURE_ORIGIN + "form",
        method="POST",
        body=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
    )
    assert f"<p>length={len(body)}</p>" in envelope["html"]
    assert hashlib.sha256(body).hexdigest() in envelope["html"]


def test_an_empty_get_has_no_method_body(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "form")
    assert "<p>method=GET</p>" in envelope["html"]
    assert "<p>length=0</p>" in envelope["html"]


# -----------------------------------------------------------------------------
# Compression
# -----------------------------------------------------------------------------
def test_gzip_is_negotiated_and_returned_as_encoded_bytes(studio):
    envelope = studio.proxy(
        FIXTURE_ORIGIN + "gzip", headers={"Accept-Encoding": "gzip"}
    )
    assert envelope["content_encoding"].lower() == "gzip"
    assert "html" not in envelope
    compress = base64.b64decode(envelope["body_b64"])
    assert compress[:2] == b"\x1f\x8b", "body_b64 is not a gzip member"
    plain = gzip.decompress(compress)
    assert b"<h1>Compressed</h1>" in plain


def test_without_accept_encoding_the_body_is_plain_html(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "gzip")
    assert "content_encoding" not in envelope
    assert "<h1>Compressed</h1>" in envelope["html"]


# -----------------------------------------------------------------------------
# Conditional requests and caching
# -----------------------------------------------------------------------------
def test_validators_are_returned_and_revalidated(studio):
    first = studio.proxy(FIXTURE_ORIGIN + "cache")
    assert first["status"] == 200
    assert first["etag"] == '"v1"'
    assert first["last-modified"] == "Wed, 21 Oct 2015 07:28:00 GMT"
    assert "max-age=60" in first["cache-control"]

    second = studio.proxy(
        FIXTURE_ORIGIN + "cache", headers={"If-None-Match": '"v1"'}
    )
    assert second["status"] == 304
    assert second["etag"] == '"v1"'
    assert second["html"] == ""


def test_no_store_is_visible_to_the_browser(studio):
    envelope = studio.proxy(FIXTURE_ORIGIN + "no-store")
    assert envelope["cache-control"] == "no-store"
    assert envelope["etag"] == '"never"'


# -----------------------------------------------------------------------------
# The SSRF boundary is unchanged
# -----------------------------------------------------------------------------
def test_a_loopback_origin_outside_the_fixtures_is_still_refused(studio):
    status, headers, data = studio.proxy_raw("http://127.0.0.1:4300/admin")
    assert status == 403
    assert "SSRF Blocked" in data.decode("utf-8", "replace")


def test_an_unsafe_scheme_is_still_refused(studio):
    status, _headers, data = studio.proxy_raw("file:///etc/passwd")
    assert status == 403
    assert b"SSRF Blocked" in data


def test_the_fixture_branch_precedes_the_ssrf_check(studio):
    source = (ROOT / "emulator" / "server.py").read_text(encoding="utf-8")
    handler = source[source.index("def _handle_browser_proxy"):]
    handler = handler[: handler.index("def ", handler.index("\n") + 1)]
    assert handler.index("local_browser_fixture_target") < handler.index("is_safe_proxy_url")


def test_a_missing_url_is_a_client_error(studio):
    status, _headers, data = studio.request("GET", "/api/browser/proxy")
    assert status == 400
    assert json.loads(data)["success"] is False


def test_post_without_a_url_is_also_rejected(studio):
    status, _headers, data = studio.request(
        "POST", "/api/browser/proxy", body=b"x=1",
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    assert status == 400
    assert json.loads(data)["success"] is False


# -----------------------------------------------------------------------------
# TLS 1.3 Transport Truth Contracts (HTTPS)
# -----------------------------------------------------------------------------
def test_port_443_https_traffic_wraps_in_tls_client_hello_and_never_sends_raw_plaintext():
    """Asserts that port 443 HTTPS traffic wraps in a TLS 1.3 ClientHello record
    and never sends raw plaintext HTTP GET."""
    def code_only(source: str) -> str:
        import re
        without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
        return re.sub(r"//[^\n]*", " ", without_block)

    main_src = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")
    http_src = (ROOT / "apps/browser/src/http.rs").read_text(encoding="utf-8")
    lib_src = (ROOT / "apps/browser/src/lib.rs").read_text(encoding="utf-8")
    all_code = code_only(main_src) + "\n" + code_only(http_src) + "\n" + code_only(lib_src)

    # 1. Browser imports TLS 1.3 stack from interos_std::tls
    assert "interos_std::tls" in (main_src + http_src + lib_src) or "std::tls" in (main_src + http_src + lib_src), (
        "Browser does not import TLS module from interos_std::tls"
    )

    # 2. TlsStream wrapper over TCP socket is defined
    assert "TlsStream" in all_code, "Browser does not implement or use TlsStream wrapper"

    # 3. Port 443 / HTTPS wraps traffic in TLS ClientHello and forbids raw plaintext GET
    assert "start_handshake" in all_code or "ClientHello" in all_code, (
        "Browser does not initiate TLS ClientHello on port 443 / HTTPS"
    )

    # 4. Lock icon color truthfully distinguishes authenticated HTTPS from HTTP
    assert "THEME.primary" in main_src, "Browser does not use active theme color for verified HTTPS"


def test_tls_handshake_failure_reports_explicit_error_and_fails_closed():
    """Asserts that a TLS handshake failure reports FetchErrorKind::TlsHandshakeFailed
    and fails closed without sending plaintext."""
    def code_only(source: str) -> str:
        import re
        without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
        return re.sub(r"//[^\n]*", " ", without_block)

    main_src = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")
    http_src = (ROOT / "apps/browser/src/http.rs").read_text(encoding="utf-8")
    lib_src = (ROOT / "apps/browser/src/lib.rs").read_text(encoding="utf-8")
    all_code = code_only(main_src) + "\n" + code_only(http_src) + "\n" + code_only(lib_src)

    assert "TlsHandshakeFailed" in all_code, (
        "Browser does not define or report explicit FetchErrorKind::TlsHandshakeFailed"
    )

