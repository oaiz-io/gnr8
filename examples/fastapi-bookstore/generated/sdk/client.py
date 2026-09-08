from __future__ import annotations

import copy
import enum
import json
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable
from typing import Any, Optional

from pydantic import BaseModel

from .errors import ApiError
from .models import (
    Book,
    BookFilters,
    BookFormat,
    BookOrError,
    CreatedMessage,
    ListBooksResponse,
)

#: First transport-error backoff step; doubles per attempt up to the ceiling below.
BASE_RETRY_DELAY_SECONDS = 0.1
#: Ceiling for the TOTAL time spent waiting between retries, including any
#: server-supplied Retry-After. A per-wait cap alone still lets
#: max_retries x cap accumulate, so the budget is spent down across the whole
#: retry sequence and retrying stops once it is exhausted.
MAX_RETRY_DELAY_SECONDS = 60.0


def _header_value(headers: dict[str, str], name: str) -> str:
    """Case-insensitive header lookup.

    HTTP header names are case-insensitive, and the response header mapping
    keeps whatever casing the server sent, so an exact-match lookup silently
    misses a spelling like `X-Request-Id`.
    """
    target = name.lower()
    for key, value in headers.items():
        if key.lower() == target:
            return value
    return ""


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def _origin(url: str) -> tuple[str, str, Optional[int]]:
    parts = urllib.parse.urlsplit(url)
    port = parts.port
    if port is None:
        port = 443 if parts.scheme.lower() == "https" else 80
    return parts.scheme.lower(), (parts.hostname or "").lower(), port


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        sensitive = set(getattr(req, "_gnr8_sensitive_headers", ()))
        sensitive.update(("Authorization", "Cookie", "Proxy-Authorization"))
        setattr(redirected, "_gnr8_sensitive_headers", tuple(sensitive))
        if _origin(req.full_url) != _origin(newurl):
            # Request.add_header() stores keys capitalize()-normalized while
            # remove_header() pops the exact key, so a configured spelling such as
            # X-API-Key never matches what is stored. Match the stored names.
            unwanted = {name.lower() for name in sensitive}
            for name, _value in redirected.header_items():
                if name.lower() in unwanted:
                    redirected.remove_header(name)
        return redirected


def _opener_with_redirect_policy(
    opener: Optional[urllib.request.OpenerDirector],
    redirect_handler: urllib.request.HTTPRedirectHandler,
) -> urllib.request.OpenerDirector:
    if opener is None:
        return urllib.request.build_opener(redirect_handler)
    handlers = [
        copy.copy(handler)
        for handler in opener.handlers
        if not isinstance(handler, urllib.request.HTTPRedirectHandler)
    ]
    policy_opener = urllib.request.build_opener(*handlers, redirect_handler)
    policy_opener.addheaders = list(opener.addheaders)
    return policy_opener


class RequestOptions:
    """Per-request SDK runtime overrides."""

    def __init__(
        self,
        *,
        timeout: Optional[float] = None,
        max_retries: Optional[int] = None,
        idempotency_key: Optional[str] = None,
        metadata: Optional[dict[str, str]] = None,
        follow_redirects: bool = False,
    ) -> None:
        self.timeout = timeout
        self.max_retries = max_retries
        self.idempotency_key = idempotency_key
        self.metadata = metadata or {}
        self.follow_redirects = follow_redirects


class HookContext:
    """Context passed to generated SDK runtime hooks."""

    def __init__(
        self,
        *,
        operation_id: str,
        method: str,
        path_template: str,
        url: str,
        headers: dict[str, str],
        request_metadata: dict[str, str],
    ) -> None:
        self.operation_id = operation_id
        self.method = method
        self.path_template = path_template
        self.url = url
        self.headers = headers
        self.request_metadata = request_metadata
        self.status: Optional[int] = None
        self.response_headers: dict[str, str] = {}
        self.response_body: bytes = b""


class ClientHooks:
    """Generated SDK runtime hooks."""

    def __init__(
        self,
        *,
        request: Optional[
            list[Callable[[HookContext, urllib.request.Request], None]]
        ] = None,
        response: Optional[list[Callable[[HookContext], None]]] = None,
        error: Optional[list[Callable[[HookContext, BaseException], None]]] = None,
    ) -> None:
        self.request = request or []
        self.response = response or []
        self.error = error or []


class Client:
    """SDK client over urllib (no requests/httpx)."""

    def __init__(
        self,
        base_url: str,
        *,
        api_key: Optional[str] = None,
        opener: Optional[urllib.request.OpenerDirector] = None,
        timeout: Optional[float] = 30.0,
        max_retries: int = 0,
        hooks: Optional[ClientHooks] = None,
    ) -> None:
        self._base_url = base_url.rstrip("/")
        self._api_key = api_key
        self._opener = _opener_with_redirect_policy(opener, _NoRedirectHandler())
        self._redirect_opener = _opener_with_redirect_policy(
            opener, _SafeRedirectHandler()
        )
        self._timeout = timeout
        self._max_retries = max_retries
        self._retry_statuses = (408, 429)
        self._retry_unsafe_methods = False
        self._hooks = hooks or ClientHooks()

    def _body_value(self, body: Any, body_encoding: str) -> Any:
        if isinstance(body, BaseModel):
            mode = "python" if body_encoding == "multipart" else "json"
            body = body.model_dump(mode=mode, by_alias=True, exclude_unset=True)
        return self._wire_value(body)

    def _wire_value(self, value: Any) -> Any:
        if isinstance(value, enum.Enum):
            return self._wire_value(value.value)
        if isinstance(value, list):
            return [self._wire_value(item) for item in value]
        if isinstance(value, tuple):
            return tuple(self._wire_value(item) for item in value)
        if isinstance(value, dict):
            return {key: self._wire_value(item) for key, item in value.items()}
        return value

    @staticmethod
    def _parameter_scalar(value: Any) -> str:
        if isinstance(value, bool):
            return "true" if value else "false"
        return str(value)

    def _parameter_pairs(
        self,
        name: str,
        value: Any,
        style: str,
        explode: bool,
    ) -> list[tuple[str, str]]:
        value = self._wire_value(value)
        if style == "spaceDelimited":
            delimiter = " "
        elif style == "pipeDelimited":
            delimiter = "|"
        else:
            delimiter = ","
        if isinstance(value, (list, tuple)):
            parts = [self._parameter_scalar(item) for item in value]
            if explode and style == "form":
                return [(name, item) for item in parts]
            return [(name, delimiter.join(parts))]
        if isinstance(value, dict):
            entries = sorted(value.items())
            if style == "deepObject":
                return [
                    (f"{name}[{key}]", self._parameter_scalar(item))
                    for key, item in entries
                ]
            if explode and style == "form":
                return [
                    (str(key), self._parameter_scalar(item)) for key, item in entries
                ]
            parts = []
            for key, item in entries:
                if explode:
                    parts.append(f"{key}={self._parameter_scalar(item)}")
                else:
                    parts.extend((str(key), self._parameter_scalar(item)))
            return [(name, delimiter.join(parts))]
        return [(name, self._parameter_scalar(value))]

    @staticmethod
    def _encode_query(
        pairs: list[tuple[str, str]],
        allow_reserved: set[int],
    ) -> str:
        reserved = ":/?#[]@!$&'()*+,;="
        encoded = []
        for index, (key, value) in enumerate(pairs):
            safe = reserved if index in allow_reserved else ""
            encoded.append(
                urllib.parse.quote(str(key), safe="")
                + "="
                + urllib.parse.quote(str(value), safe=safe)
            )
        return "&".join(encoded)

    def _encode_body(
        self,
        body: Optional[Any],
        body_encoding: str,
        content_type: str,
    ) -> tuple[Optional[bytes], str]:
        if body is None:
            return None, content_type
        if body_encoding == "binary":
            if isinstance(body, bytes):
                return body, content_type
            if isinstance(body, bytearray):
                return bytes(body), content_type
            raise TypeError("binary request bodies must be bytes or bytearray")
        if body_encoding == "text":
            return str(body).encode(), content_type
        value = self._body_value(body, body_encoding)
        if body_encoding == "json":
            return json.dumps(value).encode(), content_type
        if body_encoding == "form":
            encoded = urllib.parse.urlencode(value, doseq=True).encode()
            return encoded, content_type
        if body_encoding == "multipart":
            boundary = f"gnr8-{secrets.token_hex(16)}"
            return (
                self._encode_multipart(value, boundary),
                f"multipart/form-data; boundary={boundary}",
            )
        raise ValueError(f"unsupported request body encoding: {body_encoding}")

    def _encode_multipart(self, value: Any, boundary: str) -> bytes:
        if not isinstance(value, dict):
            raise TypeError("multipart request bodies must encode to a dict")
        out = bytearray()
        for key, item in value.items():
            if item is None:
                continue
            items = item if isinstance(item, (list, tuple)) else (item,)
            for part in items:
                if part is None:
                    continue
                out.extend(f"--{boundary}\r\n".encode())
                if isinstance(part, (bytes, bytearray)):
                    out.extend(
                        (
                            f'Content-Disposition: form-data; name="{key}"; '
                            f'filename="{key}"\r\n'
                            "Content-Type: application/octet-stream\r\n\r\n"
                        ).encode()
                    )
                    out.extend(bytes(part))
                    out.extend(b"\r\n")
                else:
                    out.extend(
                        f'Content-Disposition: form-data; name="{key}"\r\n\r\n'.encode()
                    )
                    out.extend(str(part).encode())
                    out.extend(b"\r\n")
        out.extend(f"--{boundary}--\r\n".encode())
        return bytes(out)

    def _do(
        self,
        method: str,
        path: str,
        *,
        body: Optional[Any] = None,
        request_headers: Optional[dict[str, str]] = None,
        operation_id: str,
        path_template: str,
        content_type: str = "application/json",
        body_encoding: str = "json",
        request_options: Optional[RequestOptions] = None,
        idempotent: bool = False,
        idempotency_key_header: str = "Idempotency-Key",
        success_statuses: tuple[int, ...] = (),
        sensitive_headers: tuple[str, ...] = (),
    ) -> tuple:
        data, content_type = self._encode_body(body, body_encoding, content_type)
        options = request_options or RequestOptions()
        timeout = options.timeout if options.timeout is not None else self._timeout
        if options.max_retries is not None:
            max_retries = options.max_retries
        else:
            max_retries = self._max_retries
        if max_retries < 0:
            max_retries = 0
        if not (
            self._retry_unsafe_methods
            or idempotent
            or method in ("GET", "HEAD", "OPTIONS", "PUT", "DELETE")
        ):
            max_retries = 0
        headers: dict[str, str] = dict(request_headers or {})
        if data is not None:
            headers["Content-Type"] = content_type
        if idempotent and options.idempotency_key:
            headers[idempotency_key_header] = options.idempotency_key
        url = self._base_url + path
        last_error: Optional[BaseException] = None
        _retry_budget = MAX_RETRY_DELAY_SECONDS
        opener = self._redirect_opener if options.follow_redirects else self._opener
        for attempt in range(max_retries + 1):
            req = urllib.request.Request(url, data=data, method=method)
            for key, value in headers.items():
                req.add_header(key, value)
            setattr(req, "_gnr8_sensitive_headers", sensitive_headers)
            context = HookContext(
                operation_id=operation_id,
                method=method,
                path_template=path_template,
                url=url,
                headers=dict(headers),
                request_metadata=dict(options.metadata),
            )
            try:
                for hook in self._hooks.request:
                    hook(context, req)
                try:
                    with opener.open(req, timeout=timeout) as resp:
                        status = resp.status
                        response_headers = dict(resp.headers.items())
                        raw = resp.read()
                except urllib.error.HTTPError as e:
                    status = e.code
                    response_headers = dict(e.headers.items())
                    raw = e.read()
                context.status = status
                context.response_headers = response_headers
                context.response_body = raw
                for hook in self._hooks.response:
                    hook(context)
                if (
                    self._should_retry_status(status)
                    and attempt < max_retries
                    and _retry_budget > 0
                ):
                    _delay = min(
                        self._retry_delay(response_headers, attempt),
                        _retry_budget,
                    )
                    _retry_budget -= _delay
                    time.sleep(_delay)
                    continue
                if (status < 200 or status >= 300) and status not in success_statuses:
                    self._call_error_hooks(
                        context,
                        ApiError(
                            status,
                            "",
                            "",
                            headers=response_headers,
                            request_id=_header_value(response_headers, "X-Request-ID"),
                            raw_body=raw,
                        ),
                    )
                return status, response_headers, raw
            except urllib.error.URLError as e:
                last_error = e
                if attempt < max_retries and _retry_budget > 0:
                    # Back off before reconnecting: instant retries just
                    # multiply load on a service that is already restarting.
                    _delay = min(self._backoff_delay(attempt), _retry_budget)
                    _retry_budget -= _delay
                    time.sleep(_delay)
                    continue
                self._call_error_hooks(context, e)
                raise
        if last_error is not None:
            raise last_error
        raise RuntimeError("request failed without response")

    def _should_retry_status(self, status: int) -> bool:
        return status in self._retry_statuses or status >= 500

    @staticmethod
    def _backoff_delay(attempt: int) -> float:
        # 2 ** attempt is an exact int, so a large attempt count overflows the float
        # multiply; past the cap the answer is the cap anyway.
        if attempt >= 32:
            return MAX_RETRY_DELAY_SECONDS
        step = BASE_RETRY_DELAY_SECONDS * (2 ** max(attempt, 0))
        return min(step, MAX_RETRY_DELAY_SECONDS)

    @classmethod
    def _retry_delay(cls, headers: dict[str, str], attempt: int) -> float:
        retry_after = _header_value(headers, "Retry-After")
        if retry_after:
            try:
                seconds = int(retry_after)
            except ValueError:
                seconds = 0
            if seconds > 0:
                # A server may ask for an arbitrarily long wait. Honour it
                # only up to the ceiling, so a hostile or misconfigured origin
                # cannot park the caller for hours.
                return min(float(seconds), MAX_RETRY_DELAY_SECONDS)
        return cls._backoff_delay(attempt)

    def _call_error_hooks(self, context: HookContext, error: BaseException) -> None:
        for hook in self._hooks.error:
            hook(context, error)

    @staticmethod
    def _error(
        status: int,
        headers: dict[str, str],
        raw: bytes,
        error_model: Optional[type] = None,
    ) -> ApiError:
        try:
            json_body = json.loads(raw) if raw else None
        except ValueError:
            json_body = None
        body = json_body
        if error_model is not None and isinstance(json_body, dict):
            try:
                body = error_model.from_dict(json_body)
            except Exception:
                body = json_body
        decoded = json_body if isinstance(json_body, dict) else {}
        request_id = _header_value(headers, "X-Request-ID")
        return ApiError(
            status,
            decoded.get("message", ""),
            decoded.get("slug", ""),
            decoded.get("hints"),
            headers=headers,
            request_id=request_id,
            raw_body=raw,
            json_body=json_body,
            body=body,
        )

    def list_books(
        self,
        genre: str,
        cursor: Optional[str] = None,
        sort: Optional[str] = None,
        request_options: Optional[RequestOptions] = None,
    ) -> ListBooksResponse:
        """List books in one genre.

        Results are ordered by title and paginated with an opaque cursor. Pass the
        cursor from the previous page to continue; omit it to start from the
        beginning.
        """
        path = "/books/"
        _query: list[tuple[str, str]] = []
        _allow_reserved: set[int] = set()
        _query.extend(self._parameter_pairs("genre", genre, "form", True))
        if cursor is not None:
            _query.extend(self._parameter_pairs("cursor", cursor, "form", True))
        if sort is not None:
            _query.extend(self._parameter_pairs("sort", sort, "form", True))
        if _query:
            path = path + "?" + self._encode_query(_query, _allow_reserved)
        _status, _headers, _raw = self._do(
            "GET",
            path,
            operation_id="list_books",
            path_template="/books/",
            request_options=request_options,
            idempotent=False,
            idempotency_key_header="Idempotency-Key",
            success_statuses=(200,),
        )
        if _status < 200 or _status >= 300:
            raise self._error(_status, _headers, _raw)
        if _status in (200,):
            _data = json.loads(_raw) if _raw else {}
            return ListBooksResponse.model_validate(_data)
        raise self._error(_status, _headers, _raw)

    def create_book(
        self,
        body: Book,
        request_options: Optional[RequestOptions] = None,
    ) -> CreatedMessage:
        """Add a book to the catalogue.

        The book is created immediately and its generated identifier is returned.
        """
        path = "/books/"
        _status, _headers, _raw = self._do(
            "POST",
            path,
            body=body,
            content_type="application/json",
            body_encoding="json",
            operation_id="create_book",
            path_template="/books/",
            request_options=request_options,
            idempotent=False,
            idempotency_key_header="Idempotency-Key",
            success_statuses=(201,),
        )
        if _status < 200 or _status >= 300:
            raise self._error(_status, _headers, _raw)
        if _status in (201,):
            _data = json.loads(_raw) if _raw else {}
            return CreatedMessage.model_validate(_data)
        raise self._error(_status, _headers, _raw)

    def get_book(
        self,
        book_id: int,
        fmt: Optional[BookFormat] = None,
        request_options: Optional[RequestOptions] = None,
    ) -> BookOrError:
        """Fetch one book by its identifier.

        Returns the book when it is in stock, and an out-of-stock notice otherwise.
        """
        path = f"/books/{urllib.parse.quote(str(book_id), safe='')}"
        _query: list[tuple[str, str]] = []
        _allow_reserved: set[int] = set()
        if fmt is not None:
            _query.extend(self._parameter_pairs("fmt", fmt, "form", True))
        if _query:
            path = path + "?" + self._encode_query(_query, _allow_reserved)
        _status, _headers, _raw = self._do(
            "GET",
            path,
            operation_id="get_book",
            path_template="/books/{book_id}",
            request_options=request_options,
            idempotent=False,
            idempotency_key_header="Idempotency-Key",
            success_statuses=(200,),
        )
        if _status < 200 or _status >= 300:
            raise self._error(_status, _headers, _raw)
        if _status in (200,):
            _data = json.loads(_raw) if _raw else {}
            return BookOrError.model_validate(_data)
        raise self._error(_status, _headers, _raw)

    def update_book(
        self,
        book_id: int,
        body: BookFilters,
        request_options: Optional[RequestOptions] = None,
    ) -> CreatedMessage:
        """Update the stored filters for one book.

        Filters left unset in the payload keep their current values.
        """
        path = f"/books/{urllib.parse.quote(str(book_id), safe='')}"
        _status, _headers, _raw = self._do(
            "PUT",
            path,
            body=body,
            content_type="application/json",
            body_encoding="json",
            operation_id="update_book",
            path_template="/books/{book_id}",
            request_options=request_options,
            idempotent=False,
            idempotency_key_header="Idempotency-Key",
            success_statuses=(200,),
        )
        if _status < 200 or _status >= 300:
            raise self._error(_status, _headers, _raw)
        if _status in (200,):
            _data = json.loads(_raw) if _raw else {}
            return CreatedMessage.model_validate(_data)
        raise self._error(_status, _headers, _raw)
