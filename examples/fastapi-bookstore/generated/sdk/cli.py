from __future__ import annotations

import argparse
import json
import sys
from typing import Any, Optional

from pydantic import BaseModel

from .client import Client
from .errors import ApiError
from .models import (
    Book,
    BookFilters,
)

_PROGRAM = "bookstore"
_DEFAULT_BASE_URL = "http://localhost:8000"
_VERSION = "bookstore 0.0.0"
_DESCRIPTION = "Bookstore API"

def _load_body(args: argparse.Namespace) -> Any:
    if getattr(args, "body", None) is not None:
        return json.loads(args.body)
    path = getattr(args, "body_file", None)
    if path is None:
        return None
    if path == "-":
        raw = sys.stdin.read()
    else:
        with open(path, encoding="utf-8") as handle:
            raw = handle.read()
    return json.loads(raw)


def _build_client(base_url: str) -> Client:
    return Client(base_url)


def _jsonable(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return value.model_dump(mode="json")
    if isinstance(value, list):
        return [_jsonable(item) for item in value]
    if isinstance(value, dict):
        return {key: _jsonable(item) for key, item in value.items()}
    return value


def _print_result(result: Any) -> None:
    if isinstance(result, (bytes, bytearray)):
        sys.stdout.buffer.write(result)
        return
    json.dump(_jsonable(result), sys.stdout, indent=2)
    sys.stdout.write("\n")


def _cmd_list_books(args: argparse.Namespace) -> Any:
    client = _build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    if args.cursor is not None:
        kwargs["cursor"] = args.cursor
    kwargs["genre"] = args.genre
    if args.sort is not None:
        kwargs["sort"] = args.sort
    return client.list_books(**kwargs)


def _cmd_create_book(args: argparse.Namespace) -> Any:
    client = _build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    payload = _load_body(args)
    if payload is not None:
        kwargs["body"] = Book.model_validate(payload)
    return client.create_book(**kwargs)


def _cmd_get_book(args: argparse.Namespace) -> Any:
    client = _build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    kwargs["book_id"] = args.book_id
    if args.fmt is not None:
        kwargs["fmt"] = args.fmt
    return client.get_book(**kwargs)


def _cmd_update_book(args: argparse.Namespace) -> Any:
    client = _build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    kwargs["book_id"] = args.book_id
    payload = _load_body(args)
    if payload is not None:
        kwargs["body"] = BookFilters.model_validate(payload)
    return client.update_book(**kwargs)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=_PROGRAM,
        description=_DESCRIPTION,
    )
    parser.add_argument(
        "--version",
        action="version",
        version=_VERSION,
    )
    subparsers = parser.add_subparsers(
        dest="_command",
        required=True,
    )
    cmd_list_books = subparsers.add_parser(
        "list-books",
        help="List books in one genre.",
        description=(
            "List books in one genre.\n\nResults are ordered by title and paginated wit"
            "h an opaque cursor. Pass the\ncursor from the previous page to continue; o"
            "mit it to start from the\nbeginning."
        ),
    )
    cmd_list_books.add_argument(
        "--base-url",
        dest="base_url",
        default=_DEFAULT_BASE_URL,
    )
    cmd_list_books.add_argument(
        "--cursor",
        dest="cursor",
    )
    cmd_list_books.add_argument(
        "--genre",
        dest="genre",
        required=True,
    )
    cmd_list_books.add_argument(
        "--sort",
        dest="sort",
    )
    cmd_list_books.set_defaults(_handler=_cmd_list_books)
    cmd_create_book = subparsers.add_parser(
        "create-book",
        help="Add a book to the catalogue.",
        description=(
            "Add a book to the catalogue.\n\nThe book is created immediately and its ge"
            "nerated identifier is returned."
        ),
    )
    cmd_create_book.add_argument(
        "--base-url",
        dest="base_url",
        default=_DEFAULT_BASE_URL,
    )
    cmd_create_book_body = cmd_create_book.add_mutually_exclusive_group(required=True)
    cmd_create_book_body.add_argument(
        "--body",
        dest="body",
    )
    cmd_create_book_body.add_argument(
        "--body-file",
        dest="body_file",
    )
    cmd_create_book.set_defaults(_handler=_cmd_create_book)
    cmd_get_book = subparsers.add_parser(
        "get-book",
        help="Fetch one book by its identifier.",
        description=(
            "Fetch one book by its identifier.\n\nReturns the book when it is in stock,"
            " and an out-of-stock notice otherwise."
        ),
    )
    cmd_get_book.add_argument(
        "--base-url",
        dest="base_url",
        default=_DEFAULT_BASE_URL,
    )
    cmd_get_book.add_argument(
        "--book-id",
        dest="book_id",
        required=True,
        type=int,
    )
    cmd_get_book.add_argument(
        "--fmt",
        dest="fmt",
        choices=("hardcover", "paperback",),
    )
    cmd_get_book.set_defaults(_handler=_cmd_get_book)
    cmd_update_book = subparsers.add_parser(
        "update-book",
        help="Update the stored filters for one book.",
        description=(
            "Update the stored filters for one book.\n\nFilters left unset in the paylo"
            "ad keep their current values."
        ),
    )
    cmd_update_book.add_argument(
        "--base-url",
        dest="base_url",
        default=_DEFAULT_BASE_URL,
    )
    cmd_update_book.add_argument(
        "--book-id",
        dest="book_id",
        required=True,
        type=int,
    )
    cmd_update_book_body = cmd_update_book.add_mutually_exclusive_group(required=True)
    cmd_update_book_body.add_argument(
        "--body",
        dest="body",
    )
    cmd_update_book_body.add_argument(
        "--body-file",
        dest="body_file",
    )
    cmd_update_book.set_defaults(_handler=_cmd_update_book)
    return parser


def main(argv: Optional[list[str]] = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    handler = getattr(args, "_handler", None)
    if handler is None:
        parser.print_help(sys.stderr)
        return 2
    try:
        result = handler(args)
        _print_result(result)
        return 0
    except ApiError as exc:
        print(
            f"{_PROGRAM}: {exc.status_code} {exc.message} ({exc.slug})",
            file=sys.stderr,
        )
        return 1


if __name__ == "__main__":
    sys.exit(main())
