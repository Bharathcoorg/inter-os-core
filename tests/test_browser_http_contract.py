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
import re
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
BROWSER_SOURCES = (
    "apps/browser/src/main.rs",
    "apps/browser/src/http.rs",
    "apps/browser/src/lib.rs",
)


def code_only(source: str) -> str:
    """Strips comments so a rule is asserted against the program."""
    without_block = re.sub(r"/\*.*?\*/", " ", source, flags=re.S)
    return re.sub(r"//[^\n]*", " ", without_block)


def browser_code() -> str:
    """The browser's own sources with every comment removed.

    WHY the comments are removed: each rule below is a rule about the program.
    A contract that scans comments tests the author's prose instead, and the
    superseded port rule is quoted in the comments that explain why it went.
    """
    return "\n".join(
        code_only((ROOT / path).read_text(encoding="utf-8")) for path in BROWSER_SOURCES
    )


def function_body(code: str, name: str) -> str:
    """One top-level `fn`, from its signature to the line the next one starts on.

    WHY a line slice and not a brace count: these transports hold braces inside
    string literals, and this file's style is to cut a known function out of the
    source rather than to parse Rust.
    """
    lines = code.splitlines()
    start = next(
        index
        for index, line in enumerate(lines)
        if re.match(r"(pub )?fn %s\b" % re.escape(name), line)
    )
    end = next(
        (
            index
            for index in range(start + 1, len(lines))
            if re.match(r"(pub )?fn \w", lines[index])
        ),
        len(lines),
    )
    return "\n".join(lines[start:end])


def test_the_hop_scheme_decides_tls_wrapping_and_never_the_port():
    """Wrapping is decided by the scheme of the hop, not by the port it lands on.

    WHY this replaced the port rule: `dst_port == 443` sent `https://host:8443`
    and `https://host:4300` to the wire as a plaintext GET while the omnibox
    still drew the mark the scheme implied, and it sent an `http://host:443`
    request into a TLS ClientHello. The scheme is the only thing the address
    states; a port is only where that scheme happens to be served.
    """
    code = browser_code()
    main_src = (ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8")

    # 1. Browser imports the TLS 1.3 stack from interos_std::tls.
    assert "interos_std::tls" in code or "std::tls" in code, (
        "Browser does not import TLS module from interos_std::tls"
    )

    # 2. A TLS stream over the TCP socket is what carries the request.
    assert "TlsStream" in code, "Browser does not implement or use TlsStream wrapper"

    # 3. The TLS path opens with a ClientHello and never with a plaintext GET.
    assert "start_handshake" in code or "ClientHello" in code, (
        "Browser does not initiate a TLS ClientHello for an https hop"
    )

    # 4. The transport receives the decision from its caller and reads no port.
    tcp = function_body(code, "fetch_over_tcp")
    assert "use_tls: bool" in tcp, "fetch_over_tcp does not take the hop's scheme decision"
    assert "if use_tls {" in tcp, "fetch_over_tcp does not wrap on the decision it was given"
    assert "443" not in tcp, "fetch_over_tcp still reads a port to decide whether to wrap"
    assert "dst_port ==" not in tcp, "fetch_over_tcp still compares the destination port"

    # 5. The navigation guard is the scheme alone, so https on any port wraps
    #    and an http URL naming 443 does not.
    assert "let hop_is_https = hop_parsed.scheme == UriScheme::Https;" in code, (
        "the hop's TLS decision is not read from the scheme alone"
    )
    assert "port == 443" not in code, "a port clause still decides whether a hop is https"

    # 6. Every call site names the security of its own hop; none infers it from
    #    a port. The five gateway, speedtest and lab fetches are plaintext hops.
    calls = re.findall(r"(?<!fn )fetch_over_tcp\s*\(([^;]*)\)", code)
    assert len(calls) == 6, f"expected six fetch_over_tcp call sites, found {len(calls)}"
    guards = [call.strip().rstrip(",").split(",")[-1].strip() for call in calls]
    assert all(guard in ("false", "hop_is_https") for guard in guards), (
        f"a call site passes {guards!r} instead of naming its hop's scheme"
    )
    assert guards.count("hop_is_https") == 1, "the document fetch is not the one TLS hop"
    assert not any("443" in call for call in calls), "a call site still passes port 443 as its TLS test"

    # 7. The secure mark still distinguishes the two channels it can observe.
    assert "THEME.primary" in main_src, "Browser does not use active theme color for a TLS channel"


def test_tls_handshake_failure_reports_explicit_error_and_fails_closed():
    """A failed handshake says which stage failed, sends no plaintext, and leaves
    the channel unmarked.

    WHY the last clause joined the rule: the screen's mark is read from
    `FetchOutcome.tls_verified`, so every path that returns before the handshake
    completes has to return with that flag false. A mark set for a channel that
    never completed its handshake is the claim this file exists to prevent.
    """
    code = browser_code()
    assert "TlsHandshakeFailed" in code, (
        "Browser does not define or report explicit FetchErrorKind::TlsHandshakeFailed"
    )

    tls = function_body(code, "fetch_over_tls")
    handshake = tls.index("perform_tls_handshake")
    marked = tls.index("outcome.tls_verified = true;")
    written = tls.index("tls_client.write")

    # The failure path returns before the request is written, so nothing of the
    # plaintext ever reaches the wire, and before the channel is marked.
    assert tls.index("outcome.kind = kind;") < written
    assert handshake < marked < written
    assert tls.count("outcome.tls_verified = true;") == 1, (
        "more than one path claims a completed TLS handshake"
    )

    # Both transports start unmarked; only the handshake above can change it.
    assert "tls_verified: false" in tls
    assert "tls_verified: false" in function_body(code, "fetch_over_tcp")


def test_the_secure_mark_is_read_from_the_transport_that_delivered_the_document():
    """The omnibox mark reports the observed channel of the document on screen.

    WHY this is a contract and not a detail: the bytes and the mark come from the
    same `FetchOutcome`. The assignment happens once, per hop, so a redirect that
    leaves a TLS channel cannot keep the previous hop's mark, and a navigation
    that has delivered nothing yet carries the mark of the page it replaced.
    """
    code = browser_code()
    assert "tls_verified: bool," in code, "FetchOutcome does not carry the transport's observation"

    assigned = code.index("self.document_tls_verified = outcome.tls_verified;")
    assert code.count("self.document_tls_verified = outcome.tls_verified;") == 1, (
        "more than one place decides the document's transport claim"
    )
    assert code.rindex("fetch_over_tcp(") < assigned, (
        "the secure mark is not read from the fetch that delivered the document"
    )
    assert "self.document_tls_verified = false;" in code, (
        "a new navigation does not drop the previous document's mark"
    )

    toolbar = function_body(code, "toolbar_pixel")
    assert "app.document_tls_verified" in toolbar, (
        "the omnibox mark is drawn without reading the document's transport"
    )
    assert "return if is_https { THEME.primary } else { THEME.on_surface_variant };" in toolbar, (
        "the mark no longer distinguishes the two channels it can observe"
    )


def test_the_secure_mark_cannot_be_derived_from_the_typed_scheme_alone():
    """The lock is a claim about the channel, so the channel has to be read.

    WHY this test stands on its own: the mark used to be drawn from the scheme in
    the address bar, which called a page whose request went out as plaintext a
    secure one. Restoring that single `&&` would put the claim back, so every
    scheme test the renderer performs has to be conjoined with the observed
    transport. The scan is scoped to the renderer on purpose -- inside the
    navigation loop `scheme == UriScheme::Https` is a dialling decision, not a
    mark shown to a person.
    """
    code = browser_code()
    toolbar = function_body(code, "toolbar_pixel")

    for line in toolbar.splitlines():
        if "UriScheme::Https" in line:
            assert "app.document_tls_verified" in line, (
                "the omnibox derives its security mark from the typed scheme alone: "
                f"{line.strip()!r}"
            )

    assert (
        "let is_https = parsed.scheme == UriScheme::Https && app.document_tls_verified;"
        in toolbar
    ), "the renderer's secure flag is not the scheme and the observed transport together"

    # Nothing has loaded, nothing is claimed: the mark stays undrawn.
    assert "app.status != PageStatus::Loaded" in toolbar, (
        "the mark is drawn for a document the transport never delivered"
    )


def test_no_screen_claims_a_trusted_connection_the_verifier_cannot_back():
    """The mark says what the verifier actually checks, and never more.

    WHY this is pinned: the browser's verifier checks the certificate's
    structure, its validity dates, the name match against the address dialled
    and a signature chain to the roots the browser ships -- today one synthetic
    local test root. Until that store holds a real trust anchor, no string drawn
    by the browser may say a connection is trusted or secured; it reports an
    encrypted channel that completed a handshake.
    """
    code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))
    drawn = re.findall(r'b"((?:[^"\\]|\\.)*)"', code)
    assert drawn, "no drawn strings found -- the scan has stopped testing anything"
    for text in drawn:
        lowered = text.lower()
        for overclaim in ("trusted", "secured", "fully verified", "end-to-end encrypted"):
            assert overclaim not in lowered, (
                f"the screen says {overclaim!r} in {text[:80]!r} while the verifier's "
                "root store is a local test root"
            )

