from __future__ import annotations

import email.message
import io
import json
import unittest
import urllib.parse
import urllib.request
import urllib.response

from .client import Client
from .errors import ApiError
from .models import (
    OrderInput,
    Price,
)


BASE_URL = "http://gnr8.test"


class _ContractRequest:
    """One request the generated client handed to its transport."""

    def __init__(self, method: str, url: str, headers, body: bytes) -> None:
        parts = urllib.parse.urlsplit(url)
        self.method = method
        self.path = parts.path
        self.query = urllib.parse.parse_qs(parts.query, keep_blank_values=True)
        self.headers = {name.lower(): value for name, value in headers}
        self.body = body


class _ContractHandler(urllib.request.HTTPHandler):
    """Answers canned responses and records what the client sent.

    Installed through the ``opener`` seam the generated Client already exposes, so
    urllib's own HTTPErrorProcessor turns a canned 4xx into the HTTPError the client
    handles in production.
    """

    def __init__(self) -> None:
        super().__init__()
        self.requests: list[_ContractRequest] = []
        self.responses: list[tuple[int, dict[str, str], bytes]] = []

    def queue(self, status: int, headers: dict[str, str], body: str) -> None:
        self.responses.append((status, headers, body.encode("utf-8")))

    def http_open(self, req):  # noqa: D102 - urllib handler protocol
        self.requests.append(
            _ContractRequest(
                req.get_method(), req.full_url, req.header_items(), req.data or b""
            )
        )
        if not self.responses:
            raise AssertionError("contract transport ran out of canned responses")
        status, headers, payload = self.responses.pop(0)
        message = email.message.Message()
        for name, value in headers.items():
            message[name] = value
        response = urllib.response.addinfourl(
            io.BytesIO(payload), message, req.full_url, status
        )
        response.msg = "Contract"
        return response


def _contract_client(handler: _ContractHandler, **credentials) -> Client:
    return Client(BASE_URL, opener=urllib.request.build_opener(handler), **credentials)


class ContractTest(unittest.TestCase):
    """The wire contract the API graph states, asserted against a fake transport."""

    def _single_request(self, handler: _ContractHandler) -> _ContractRequest:
        self.assertEqual(len(handler.requests), 1, "expected exactly one request")
        return handler.requests[0]

    def _assert_wire(self, request, method, path, query, headers) -> None:
        self.assertEqual(request.method, method, "method")
        self.assertEqual(request.path, path, "path")
        self.assertEqual(request.query, query, "query")
        for name, value in headers.items():
            self.assertEqual(request.headers.get(name), value, name)

    def _assert_body(self, request, expected: str) -> None:
        self.assertEqual(json.loads(request.body), json.loads(expected), "request body")

    def test_request_shape_list_orders(self) -> None:
        handler = _ContractHandler()
        handler.queue(200, {"content-type": "application/json"}, "{\"availability\":\"in_stock\",\"lines\":[{\"amount\":1.5,\"currency\":\"eur\"}],\"message\":\"gnr8\",\"order_id\":7}")
        client = _contract_client(handler)
        result = client.list_orders(status="gnr8")
        request = self._single_request(handler)
        self._assert_wire(request, "GET", "/orders/", {"status": ["gnr8"]}, {})
        self.assertEqual(result.message, "gnr8", "message")

    def test_request_shape_create_order(self) -> None:
        handler = _ContractHandler()
        handler.queue(201, {"content-type": "application/json"}, "{\"availability\":\"in_stock\",\"lines\":[{\"amount\":1.5,\"currency\":\"eur\"}],\"message\":\"gnr8\",\"order_id\":7}")
        client = _contract_client(handler)
        result = client.create_order(body=OrderInput(book_id=7, price=Price(amount=1.5, currency="eur")))
        request = self._single_request(handler)
        self._assert_wire(request, "POST", "/orders/", {}, {"content-type": "application/json"})
        self._assert_body(request, "{\"book_id\":7,\"price\":{\"amount\":1.5,\"currency\":\"eur\"}}")
        self.assertEqual(result.message, "gnr8", "message")

    def test_request_shape_create_order_raw(self) -> None:
        handler = _ContractHandler()
        handler.queue(201, {}, "")
        client = _contract_client(handler)
        result = client.create_order_raw()
        request = self._single_request(handler)
        self._assert_wire(request, "POST", "/orders/raw", {}, {})
        del result

    def test_request_shape_get_order(self) -> None:
        handler = _ContractHandler()
        handler.queue(200, {"content-type": "application/json"}, "{\"availability\":\"in_stock\",\"lines\":[{\"amount\":1.5,\"currency\":\"eur\"}],\"message\":\"gnr8\",\"order_id\":7}")
        client = _contract_client(handler)
        result = client.get_order(order_id=7)
        request = self._single_request(handler)
        self._assert_wire(request, "GET", "/orders/7", {}, {})
        self.assertEqual(result.message, "gnr8", "message")

    def test_response_decode_list_orders_present(self) -> None:
        handler = _ContractHandler()
        handler.queue(200, {"content-type": "application/json"}, "{\"availability\":\"in_stock\",\"lines\":[{\"amount\":1.5,\"currency\":\"eur\"}],\"message\":\"gnr8\",\"order_id\":7}")
        client = _contract_client(handler)
        result = client.list_orders(status="gnr8")
        request = self._single_request(handler)
        self._assert_wire(request, "GET", "/orders/", {"status": ["gnr8"]}, {})
        self.assertEqual(result.message, "gnr8", "message")

    def test_response_decode_create_order_present(self) -> None:
        handler = _ContractHandler()
        handler.queue(201, {"content-type": "application/json"}, "{\"availability\":\"in_stock\",\"lines\":[{\"amount\":1.5,\"currency\":\"eur\"}],\"message\":\"gnr8\",\"order_id\":7}")
        client = _contract_client(handler)
        result = client.create_order(body=OrderInput(book_id=7, price=Price(amount=1.5, currency="eur")))
        request = self._single_request(handler)
        self._assert_wire(request, "POST", "/orders/", {}, {"content-type": "application/json"})
        self._assert_body(request, "{\"book_id\":7,\"price\":{\"amount\":1.5,\"currency\":\"eur\"}}")
        self.assertEqual(result.message, "gnr8", "message")

    def test_typed_error_list_orders_400(self) -> None:
        handler = _ContractHandler()
        handler.queue(400, {"content-type": "application/json"}, "{\"message\":\"contract test error\",\"slug\":\"contract_test_error\"}")
        client = _contract_client(handler)
        with self.assertRaises(ApiError) as caught:
            client.list_orders(status="gnr8")
        self.assertEqual(caught.exception.status_code, 400, "status")
        request = self._single_request(handler)
        self._assert_wire(request, "GET", "/orders/", {"status": ["gnr8"]}, {})

    def test_redirect_policy_list_orders(self) -> None:
        handler = _ContractHandler()
        handler.queue(302, {"location": "http://gnr8.test/moved"}, "")
        client = _contract_client(handler)
        with self.assertRaises(ApiError) as caught:
            client.list_orders(status="gnr8")
        self.assertEqual(caught.exception.status_code, 302, "status")
        # The 0.11 contract: a redirect is surfaced, never followed, unless the
        # caller opts in with follow_redirects.
        request = self._single_request(handler)
        self._assert_wire(request, "GET", "/orders/", {"status": ["gnr8"]}, {})


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
