from __future__ import annotations

import email.message
import email.parser
import email.policy
import io
import json
import urllib.parse
import urllib.request
import urllib.response
from typing import Any

from example_wire import (
    Client,
    ClientHooks,
    CreateUploadRequest,
    RequestOptions,
    UploadFileFormRequest,
)


class FakeHTTPHandler(urllib.request.BaseHandler):
    handler_order = 100

    def __init__(self, requests: list[Any]) -> None:
        self.requests = requests

    def http_open(self, request: Any) -> Any:
        self.requests.append(request)
        if request.full_url.endswith("/redirect"):
            return fake_response(
                request,
                307,
                {"Location": "/v1/files/final", "X-Session-ID": "session-123"},
            )
        if request.full_url.endswith("/final"):
            return fake_response(request, 204)
        if urllib.parse.urlsplit(request.full_url).path.endswith("/search"):
            return fake_response(
                request,
                200,
                {"Content-Type": "application/json"},
                json.dumps(
                    {
                        "q": "",
                        "limit": 0,
                        "offset": 0,
                        "page": 1,
                        "days": 0,
                        "sort": "",
                        "cursor": "",
                        "token": "",
                    }
                ).encode(),
            )
        return fake_response(request, 204)

    https_open = http_open


def fake_response(
    request: Any,
    status: int,
    headers: dict[str, str] | None = None,
    body: bytes = b"",
) -> Any:
    message = email.message.Message()
    for name, value in (headers or {}).items():
        message[name] = value
    response = urllib.response.addinfourl(
        io.BytesIO(body), message, request.full_url, code=status
    )
    response.msg = "fixture"
    return response


def multipart_parts(request: Any) -> dict[str, list[bytes]]:
    content_type = request.get_header("Content-type")
    assert content_type and content_type.startswith("multipart/form-data; boundary=")
    message = email.parser.BytesParser(policy=email.policy.default).parsebytes(
        b"Content-Type: " + content_type.encode() + b"\r\nMIME-Version: 1.0\r\n\r\n" + request.data
    )
    parts: dict[str, list[bytes]] = {}
    for part in message.iter_parts():
        name = part.get_param("name", header="content-disposition")
        assert isinstance(name, str)
        parts.setdefault(name, []).append(part.get_payload(decode=True))
    return parts


def main() -> None:
    requests: list[Any] = []
    opener = urllib.request.build_opener(FakeHTTPHandler(requests))
    response_contexts: list[Any] = []
    client = Client(
        "https://api.test",
        opener=opener,
        hooks=ClientHooks(response=[response_contexts.append]),
    )

    client.upload_file(("application/json", CreateUploadRequest(title="json")))
    json_request = requests[-1]
    assert json_request.get_header("Content-type") == "application/json"
    assert json.loads(json_request.data) == {"title": "json"}

    cases = [
        UploadFileFormRequest(request='{"name":"multipart"}'),
        UploadFileFormRequest(request='{"name":"multipart"}', files=[b"one"]),
        UploadFileFormRequest(request='{"name":"multipart"}', files=[b"one", b"two"]),
    ]
    expected_files = [[], [b"one"], [b"one", b"two"]]
    for body, expected in zip(cases, expected_files):
        client.upload_file(("multipart/form-data", body))
        parts = multipart_parts(requests[-1])
        assert parts.get("files", []) == expected, parts
        assert parts["request"] == [b'{"name":"multipart"}'], parts
        if not expected:
            assert "files" not in parts, parts

    client.search_items(1, q="")
    client.search_items(1, q="", offset=0)
    search_urls = [
        request.full_url
        for request in requests
        if urllib.parse.urlsplit(request.full_url).path.endswith("/search")
    ]
    first_query = urllib.parse.parse_qs(urllib.parse.urlsplit(search_urls[0]).query, keep_blank_values=True)
    second_query = urllib.parse.parse_qs(urllib.parse.urlsplit(search_urls[1]).query, keep_blank_values=True)
    assert "offset" not in first_query and "sort" not in first_query and "cursor" not in first_query
    assert second_query["offset"] == ["0"]

    client.redirect_file("file-1")
    client.redirect_file("file-1", RequestOptions(follow_redirects=True))
    assert len([request for request in requests if request.full_url.endswith("/redirect")]) == 2
    assert len([request for request in requests if request.full_url.endswith("/final")]) == 1
    redirect_contexts = [context for context in response_contexts if context.operation_id == "redirectFile"]
    assert [context.status for context in redirect_contexts] == [307, 204]
    assert redirect_contexts[0].response_headers["X-Session-ID"] == "session-123"


if __name__ == "__main__":
    main()
