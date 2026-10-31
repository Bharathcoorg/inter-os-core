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


def method_body(code: str, name: str) -> str:
    """One method inside an `impl` block, sliced the same way.

    WHY this is not `function_body`: that matcher is anchored at column zero
    because every transport it has ever cut is a free function. The navigation
    loop and the two subresource fetches live inside `impl BrowserApp`, so a
    contract about where *those* calls happen needs the indented form -- and
    needs it to end at the next method of the same indent rather than at the
    next line that merely mentions `fn`.
    """
    lines = code.splitlines()
    start = next(
        index
        for index, line in enumerate(lines)
        if re.match(r"[ \t]+(pub )?fn %s\b" % re.escape(name), line)
    )
    indent = len(lines[start]) - len(lines[start].lstrip())
    end = next(
        (
            index
            for index in range(start + 1, len(lines))
            if len(lines[index]) - len(lines[index].lstrip()) == indent
            and re.match(r"(pub )?fn \w", lines[index].strip())
        ),
        len(lines),
    )
    return "\n".join(lines[start:end])


def images_code() -> str:
    """The inline-image policy module with its comments removed.

    WHY read separately: the mixed-content rule lives where the picture
    refusals already live, and pulling the file into `browser_code()` would
    move every call-site count this file already pins.
    """
    return code_only((ROOT / "apps/browser/src/images.rs").read_text(encoding="utf-8"))



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
    and a signature chain to the roots the browser ships. The shipped store
    holds no root at all, so every chain is refused; until a real trust anchor
    is provisioned, no string drawn by the browser may say a connection is
    trusted or secured; it reports an encrypted channel that completed a
    handshake.
    """
    code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))
    drawn = re.findall(r'b"((?:[^"\\]|\\.)*)"', code)
    assert drawn, "no drawn strings found -- the scan has stopped testing anything"
    for text in drawn:
        lowered = text.lower()
        for overclaim in ("trusted", "secured", "fully verified", "end-to-end encrypted"):
            assert overclaim not in lowered, (
                f"the screen says {overclaim!r} in {text[:80]!r} while the verifier's "
                "root store holds no anchor"
            )


def test_a_server_name_the_browser_cannot_pin_is_refused_not_truncated():
    """The name sent in SNI and the name the verifier pins are one whole name.

    WHY this is pinned: the address bar and the handshake are two places that
    must agree on the host. A name that does not fit the buffer, or that is not
    text, cannot be agreed on -- a truncated name is a prefix, and a
    certificate issued for the prefix is not a certificate for the host that
    was dialled. The browser refuses such a name before it opens a connection
    instead of quietly shortening it.
    """
    code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))

    assert "from_utf8_unchecked" not in code, (
        "the server name is turned into a string without checking that it is text"
    )
    setter = code.split("pub fn set_current_server_name", 1)[1].split("\n}", 1)[0]
    assert ".min(" not in setter, (
        "the server name is truncated to fit the buffer instead of being refused"
    )
    assert not re.search(
        r"\.min\(\s*(?:TLS_HOSTNAME_CAPACITY|name\.len\(\)|name_bytes\.len\(\))", code
    ), "the server name is truncated to fit the buffer instead of being refused"
    assert "buf[..name.len()].copy_from_slice(name);" in code, (
        "the whole server name is not copied into the buffer"
    )

    assert "TLS_HOSTNAME_CAPACITY" in code, (
        "the buffer capacity is not named, so the refusal has nothing to compare against"
    )
    assert "name.len() > TLS_HOSTNAME_CAPACITY" in code, (
        "a name longer than the buffer is not refused"
    )
    assert "core::str::from_utf8(name).is_err()" in code, (
        "a server name that is not text is not refused"
    )
    assert "TLS_HOSTNAME_REFUSED = true" in code, (
        "a refused server name is not remembered, so the fetch path cannot see it"
    )

    # The refusal has to reach the screen as its own outcome, not as a
    # generic handshake failure: the two mean different things to the user.
    assert "FetchErrorKind::TlsNameRefused" in code, (
        "a refused server name is not carried to the screen as its own kind"
    )
    assert "ConnectionFailure::TlsNameRefused" in code, (
        "a refused server name is not carried to the screen as its own failure"
    )
    assert "BODY_TLS_NAME_REFUSED" in code, (
        "a refused server name is not explained on the screen"
    )


def test_the_screen_separates_a_missing_trust_anchor_from_a_refused_certificate():
    """A device with no root and a device that rejected a certificate are not the same fault.

    WHY this is pinned: the shipped verifier holds no trust anchor, so every
    certificate chain is refused. If the screen said "the certificate was
    rejected" for that, it would be describing a fault the device never
    observed -- the device never had a root to accept the certificate with.
    """
    code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))

    assert "fn tls_trust_store_is_empty() -> bool" in code, (
        "the browser does not ask its own store whether it holds a trust anchor"
    )
    assert "tls_trust_store_is_empty()" in code, (
        "the trust anchor state is never consulted when reporting a TLS failure"
    )
    assert "ConnectionFailure::TlsNoTrustAnchor" in code, (
        "a missing trust anchor is not a distinct failure on the screen"
    )
    assert "BODY_TLS_NO_TRUST_ANCHOR" in code, (
        "a missing trust anchor is not explained on the screen"
    )
    bodies = dict(
        re.findall(r'const (BODY_TLS_\w+): &\[u8\] = b"((?:[^"\\]|\\[\s\S])*)"', code)
    )
    assert bodies.get("BODY_TLS_NO_TRUST_ANCHOR") != bodies.get(
        "BODY_TLS_HANDSHAKE_FAILED"
    ), "the missing anchor and the failed handshake say the same thing"
    assert "trust anchor" in bodies.get("BODY_TLS_NO_TRUST_ANCHOR", ""), (
        "the missing-anchor body does not name the missing anchor"
    )
    assert "ConnectionFailure::TlsNoTrustAnchor => (b\"No trust anchor\"" in code, (
        "the status line does not name the missing trust anchor"
    )
    assert "ConnectionFailure::TlsNameRefused => (b\"Address refused\"" in code, (
        "the status line does not name the refused address"
    )


def test_an_unresolved_host_name_is_refused_and_never_dialled_as_the_gateway():
    """A failed lookup used to answer with the QEMU slirp gateway's own address.

    WHY this is a truth defect and not a convenience: `dns_resolve` returns a
    `Result`, and `unwrap_or([10, 0, 2, 2])` threw the error away. So a name that
    resolved to nothing was dialled at port 443 on the development host while the
    address field still showed the name the person typed, and a peer who never
    held that certificate answered. The two facts this browser is built to keep
    apart -- "I could not find it" and "I found it" -- collapsed into one, and
    the one it reported was the success.

    The screen already had the right words and no way to reach them:
    `TransportFailure::DnsFailed` was declared, given a title, a body and a
    banner, and never assigned. The fix hands the refusal to the channel the app
    already built for it.
    """
    code = browser_code()
    navigate = method_body(code, "navigate_again")

    # The scan has to be able to fail: if the loop no longer resolves anything,
    # every ordering assertion below reads a function that cannot hold the
    # defect this contract watches.
    assert "std::dns_resolve(" in navigate, (
        "the navigation loop holds no DNS lookup -- this contract is slicing "
        "empty air and would pass against any source at all"
    )

    # The invented answer is gone, in both spacings the literal was written with.
    for invented in ("unwrap_or([10, 0, 2, 2])", "unwrap_or([10,0,2,2])"):
        assert invented not in code, (
            f"a failed resolution is being answered with {invented!r} again, "
            "which dials the development gateway under whatever name was typed"
        )

    # Inside the loop the gateway address appears exactly once, and only after
    # the lookup: it is the configured plaintext proxy hop, never a stand-in for
    # a host the resolver could not find.
    assert navigate.count("[10, 0, 2, 2]") == 1, (
        "the gateway literal has come back to more than the proxy hop, so a "
        "resolution failure has somewhere to be quietly sent"
    )
    lookup = navigate.index("std::dns_resolve(")
    assert navigate.index("[10, 0, 2, 2]") > lookup, (
        "an address sits inside the lookup rather than after it"
    )

    # The refusal ends the navigation before the transport is ever asked, and
    # names itself as a transport that was not used rather than a peer that
    # refused.
    refused = navigate.index("TransportFailure::DnsFailed")
    dialled = navigate.index("fetch_over_tcp(")
    assert lookup < refused < dialled, "an unresolvable name still reaches the transport"
    assert "return false;" in navigate[lookup:dialled], (
        "the refused lookup falls through instead of ending the navigation"
    )
    assert "PageStatus::NoTransport" in navigate[lookup:dialled], (
        "the refused lookup is not put on the screen as its own page"
    )

    # And the screen says what was and was not done.
    main_code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))
    assert (
        'TransportFailure::DnsFailed => (b"Address cannot be resolved", BODY_DNS_FAILED)'
        in main_code
    ), "an unresolved name has no status line of its own"
    assert 'TransportFailure::DnsFailed => b"DNS RESOLUTION FAILED"' in main_code, (
        "an unresolved name has no banner of its own"
    )
    bodies = dict(
        re.findall(r'const (BODY_\w+): &\[u8\] = b"((?:[^"\\]|\\[\s\S])*)"', main_code)
    )
    dns_body = bodies.get("BODY_DNS_FAILED", "").lower()
    assert "no address was dialled" in dns_body and "no request was sent" in dns_body, (
        "the resolution-failure page claims more than the browser did: it must "
        "say that nothing was opened, not invite the person to check a network"
    )


def test_a_connection_is_not_opened_with_a_starting_number_written_at_the_desk():
    """The initial sequence number used to have a hardcoded fallback.

    WHY a constant is not a fallback: `0x1F2E_3D4C` and `0x2A3B_4C5D` are the
    same value on every connection the device opens, so anyone who has seen one
    connection knows the starting number of the next. A number the protocol asks
    to be unpredictable was made entirely predictable by a value typed in at the
    desk, and nothing on the screen or in the log said so.

    The kernel already answers this: `CryptoEntropyRead` is granted to the
    browser in the kernel's own spawn table -- the same narrow read that has
    seeded its TLS key material since the entropy gate was narrowed. So a refusal
    means the entropy service said no, and the honest answer to that is a
    connection that never opened, named as such.
    """
    code = browser_code()

    # The scan has to be able to fail.
    helper = function_body(code, "connection_sequence_number")
    assert "std::get_random(&mut seq_bytes)" in helper, (
        "the sequence number is no longer read from the entropy service, so the "
        "three clauses below are comparing nothing to anything"
    )
    assert "-> Option<u32>" in helper, "the helper no longer has a way to say it obtained no number"

    for invented in ("0x1F2E_3D4C", "0x2A3B_4C5D", "0x1F2E3D4C", "0x2A3B4C5D"):
        assert invented not in code, (
            f"the fixed initial sequence number {invented!r} is back, and every "
            "connection the device opens shares it"
        )

    # Every transport asks the one helper, and ends before the socket when the
    # read is refused -- so no connection is opened with an invented number.
    for name, marker in (
        ("fetch_over_tls", "FetchErrorKind::TlsEntropyRefused"),
        ("fetch_over_tcp", "FetchErrorKind::EntropyRefused"),
        ("upload_over_tcp", "ENTROPY_REFUSED"),
    ):
        body = function_body(code, name)
        assert "connection_sequence_number()" in body, (
            f"{name} chooses its starting number without the shared helper"
        )
        gate = body.index("connection_sequence_number()")
        opened = body.index("std::tcp_connect(")
        assert gate < opened, f"{name} opens a socket before it has a number"
        assert body.index(marker) < opened, (
            f"{name} reports its refusal after opening the connection it refused"
        )
        assert "return outcome;" in body[gate:opened], (
            f"{name} continues with a number it did not get"
        )

    # One read per connection, not three copies of a block that could drift.
    # The match is what identifies a call: the helper's own definition spells the
    # bare name, so counting the name alone would credit it with opening a socket.
    assert code.count("match connection_sequence_number() {") == 3, (
        "the sequence number is being chosen in more places than the transports"
    )

    # The plaintext refusal is its own outcome, not a network fault: the network
    # was never asked, and blaming the path sends the operator to a gateway that
    # was never dialled.
    main_code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))
    assert "FetchErrorKind::EntropyRefused => {" in main_code, (
        "a refused entropy read on a plaintext hop is not carried to the screen"
    )
    assert (
        'TransportFailure::NoEntropy => (b"No starting number", BODY_NO_ENTROPY)' in main_code
    ), "an entropy refusal on a plaintext hop has no status line of its own"
    assert 'TransportFailure::NoEntropy => b"ENTROPY REFUSED"' in main_code, (
        "an entropy refusal on a plaintext hop has no banner of its own"
    )
    assert "get_ticks" not in helper, (
        "the starting number is being manufactured from the tick counter, which "
        "is a per-connection variation at best and must not be written as if it "
        "were entropy"
    )


def test_a_verified_document_does_not_pull_a_plaintext_subresource_in_silence():
    """A page shown under the padlock had every one of its pictures fetched in
    the open.

    WHY this is the other half of #83: the secure mark was fixed to report the
    transport that delivered the *document* and nothing more. That made the mark
    true and the page around it false -- the document's own bytes were verified
    while its images, its page icon and its linked stylesheets were dialled at
    the gateway with TLS switched off, so a middlebox could restyle the page or
    replace its pictures and the lock would not move.

    The chosen honest behaviour is to refuse the plaintext subresource of a
    verified document and to say so where the reader can see it: the layout
    engine already draws a typed placeholder for every picture it did not load,
    and the refusal is given its own reason on that card. The rule runs one way
    only -- a document that itself arrived over the plaintext hop claimed no
    protection, so blocking its subresources would report a security it never had
    and would break every gateway page this reader is built for.
    """
    code = browser_code()
    policy = images_code()

    # The scan has to be able to fail: the rule is a conjunction of two facts,
    # and if either half disappears the checks below silently stop meaning.
    assert (
        "pub fn mixed_content_block(document_tls_verified: bool, "
        "subresource_is_plaintext: bool) -> bool" in policy
    ), "the mixed-content rule is no longer a decision between the document's channel and the subresource's"
    assert "document_tls_verified && subresource_is_plaintext" in policy, (
        "the rule no longer requires both halves, so it either blocks every "
        "gateway page or blocks nothing"
    )

    # The refusal is its own reason, with its own words on the card.
    assert "InsecureSubresource," in policy, (
        "a blocked subresource has no reason of its own to show"
    )
    assert (
        'ImageRefusal::InsecureSubresource => b"Image not shown: plaintext on a verified page"'
        in policy
    ), "the placeholder card no longer says why the picture was not fetched"

    # Both subresource transports ask before either is opened, and read the
    # document's own transport rather than a constant -- so the block can never
    # become unconditional, and can never be dropped without failing here.
    image = method_body(code, "fetch_inline_image")
    stylesheet = method_body(code, "fetch_stylesheet")
    assert "fetch_over_tcp(" in image and "fetch_over_tcp(" in stylesheet, (
        "a subresource transport no longer fetches over TCP, so the ordering "
        "checks below have stopped testing the path this contract is about"
    )
    for name, body in (("image", image), ("stylesheet", stylesheet)):
        assert "images::mixed_content_block(self.document_tls_verified, true)" in body, (
            f"the {name} transport no longer consults the mixed-content rule "
            "with the document's own observed transport"
        )
        assert body.index("mixed_content_block") < body.index("fetch_over_tcp("), (
            f"the {name} transport fetches in plaintext and only then decides"
        )

    # The refusal reaches the reader: a typed placeholder for the picture, and a
    # line of its own for the stylesheet, because no request went out for either.
    assert "ImageRefusal::InsecureSubresource" in image, (
        "a blocked picture is refused with some other reason, or not refused"
    )
    assert "log_stylesheet_blocked_as_insecure" in stylesheet, (
        "a blocked stylesheet returns no rules and says nothing about why"
    )
    main_code = code_only((ROOT / "apps/browser/src/main.rs").read_text(encoding="utf-8"))
    assert "Linked stylesheet blocked" in main_code, (
        "the stylesheet refusal is not in the log the page author can read"
    )

    # The padlock itself did not move: it is still the document's transport and
    # nothing else, which is what makes the refusal above the right answer.
    toolbar = function_body(code, "toolbar_pixel")
    assert (
        "let is_https = parsed.scheme == UriScheme::Https && app.document_tls_verified;"
        in toolbar
    ), "the secure mark changed instead of the page's subresources being held to it"

