"""The commands that sit directly under the program."""

from __future__ import annotations

import argparse
from typing import Any

from ...models import (
    Book,
    BookFilters,
)
from ..body import load_body
from ..config import DEFAULT_BASE_URL
from ..credentials import build_client


def register(subparsers: Any) -> None:
    """Add these commands to the program's subparsers."""
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
        help="host to send requests to (default: http://127.0.0.1:8000)",
        default=DEFAULT_BASE_URL,
    )
    cmd_list_books.add_argument(
        "--json",
        dest="json",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print the server body (shorthand for --format json)",
    )
    cmd_list_books.add_argument(
        "--format",
        dest="format",
        choices=("human", "ai-friendly", "json", "jsonl"),
        default=argparse.SUPPRESS,
        help="output format: human, ai-friendly, json, or jsonl",
    )
    cmd_list_books.add_argument(
        "--fields",
        dest="fields",
        default=argparse.SUPPRESS,
        help="comma-separated response fields, or help to list them",
    )
    cmd_list_books.add_argument(
        "-o",
        "--output",
        dest="output",
        default=argparse.SUPPRESS,
        help="write the full result to a file, or - for stdout",
    )
    cmd_list_books.add_argument(
        "--quiet",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_list_books.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_list_books.add_argument(
        "--debug",
        dest="debug",
        action="store_true",
        default=argparse.SUPPRESS,
        help="write a request trace to stderr",
    )
    cmd_list_books.add_argument(
        "--yes",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_list_books.add_argument(
        "-y",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_list_books.add_argument(
        "--no-input",
        dest="no_input",
        action="store_true",
        default=argparse.SUPPRESS,
        help="never prompt; refuse commands that would ask",
    )
    cmd_list_books.add_argument(
        "--cursor",
        dest="cursor",
    )
    cmd_list_books.add_argument(
        "--genre",
        dest="genre",
        required=True,
        help="required",
    )
    cmd_list_books.add_argument(
        "--sort",
        dest="sort",
    )
    cmd_list_books.set_defaults(
        _handler=_list_books,
        _command="list-books",
        _fields=(),
    )
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
        help="host to send requests to (default: http://127.0.0.1:8000)",
        default=DEFAULT_BASE_URL,
    )
    cmd_create_book.add_argument(
        "--json",
        dest="json",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print the server body (shorthand for --format json)",
    )
    cmd_create_book.add_argument(
        "--format",
        dest="format",
        choices=("human", "ai-friendly", "json", "jsonl"),
        default=argparse.SUPPRESS,
        help="output format: human, ai-friendly, json, or jsonl",
    )
    cmd_create_book.add_argument(
        "--fields",
        dest="fields",
        default=argparse.SUPPRESS,
        help="comma-separated response fields, or help to list them",
    )
    cmd_create_book.add_argument(
        "-o",
        "--output",
        dest="output",
        default=argparse.SUPPRESS,
        help="write the full result to a file, or - for stdout",
    )
    cmd_create_book.add_argument(
        "--quiet",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_create_book.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_create_book.add_argument(
        "--debug",
        dest="debug",
        action="store_true",
        default=argparse.SUPPRESS,
        help="write a request trace to stderr",
    )
    cmd_create_book.add_argument(
        "--yes",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_create_book.add_argument(
        "-y",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_create_book.add_argument(
        "--no-input",
        dest="no_input",
        action="store_true",
        default=argparse.SUPPRESS,
        help="never prompt; refuse commands that would ask",
    )
    cmd_create_book_body = cmd_create_book.add_mutually_exclusive_group(required=True)
    cmd_create_book_body.add_argument(
        "--body",
        dest="body",
        help="request body, as an inline JSON document",
    )
    cmd_create_book_body.add_argument(
        "--body-file",
        dest="body_file",
        help="read the request body from a file, or - for stdin",
    )
    cmd_create_book.set_defaults(
        _handler=_create_book,
        _command="create-book",
        _fields=(),
    )
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
        help="host to send requests to (default: http://127.0.0.1:8000)",
        default=DEFAULT_BASE_URL,
    )
    cmd_get_book.add_argument(
        "--json",
        dest="json",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print the server body (shorthand for --format json)",
    )
    cmd_get_book.add_argument(
        "--format",
        dest="format",
        choices=("human", "ai-friendly", "json", "jsonl"),
        default=argparse.SUPPRESS,
        help="output format: human, ai-friendly, json, or jsonl",
    )
    cmd_get_book.add_argument(
        "--fields",
        dest="fields",
        default=argparse.SUPPRESS,
        help="comma-separated response fields, or help to list them",
    )
    cmd_get_book.add_argument(
        "-o",
        "--output",
        dest="output",
        default=argparse.SUPPRESS,
        help="write the full result to a file, or - for stdout",
    )
    cmd_get_book.add_argument(
        "--quiet",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_get_book.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_get_book.add_argument(
        "--debug",
        dest="debug",
        action="store_true",
        default=argparse.SUPPRESS,
        help="write a request trace to stderr",
    )
    cmd_get_book.add_argument(
        "--yes",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_get_book.add_argument(
        "-y",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_get_book.add_argument(
        "--no-input",
        dest="no_input",
        action="store_true",
        default=argparse.SUPPRESS,
        help="never prompt; refuse commands that would ask",
    )
    cmd_get_book.add_argument(
        "--book-id",
        dest="book_id",
        required=True,
        type=int,
        help="required",
    )
    cmd_get_book.add_argument(
        "--fmt",
        dest="fmt",
        choices=("hardcover", "paperback"),
    )
    cmd_get_book.set_defaults(
        _handler=_get_book,
        _command="get-book",
        _fields=(),
    )
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
        help="host to send requests to (default: http://127.0.0.1:8000)",
        default=DEFAULT_BASE_URL,
    )
    cmd_update_book.add_argument(
        "--json",
        dest="json",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print the server body (shorthand for --format json)",
    )
    cmd_update_book.add_argument(
        "--format",
        dest="format",
        choices=("human", "ai-friendly", "json", "jsonl"),
        default=argparse.SUPPRESS,
        help="output format: human, ai-friendly, json, or jsonl",
    )
    cmd_update_book.add_argument(
        "--fields",
        dest="fields",
        default=argparse.SUPPRESS,
        help="comma-separated response fields, or help to list them",
    )
    cmd_update_book.add_argument(
        "-o",
        "--output",
        dest="output",
        default=argparse.SUPPRESS,
        help="write the full result to a file, or - for stdout",
    )
    cmd_update_book.add_argument(
        "--quiet",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_update_book.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS,
        help="print less on success",
    )
    cmd_update_book.add_argument(
        "--debug",
        dest="debug",
        action="store_true",
        default=argparse.SUPPRESS,
        help="write a request trace to stderr",
    )
    cmd_update_book.add_argument(
        "--yes",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_update_book.add_argument(
        "-y",
        dest="yes",
        action="store_true",
        default=argparse.SUPPRESS,
        help="do not ask before a destructive command",
    )
    cmd_update_book.add_argument(
        "--no-input",
        dest="no_input",
        action="store_true",
        default=argparse.SUPPRESS,
        help="never prompt; refuse commands that would ask",
    )
    cmd_update_book.add_argument(
        "--book-id",
        dest="book_id",
        required=True,
        type=int,
        help="required",
    )
    cmd_update_book_body = cmd_update_book.add_mutually_exclusive_group(required=True)
    cmd_update_book_body.add_argument(
        "--body",
        dest="body",
        help="request body, as an inline JSON document",
    )
    cmd_update_book_body.add_argument(
        "--body-file",
        dest="body_file",
        help="read the request body from a file, or - for stdin",
    )
    cmd_update_book.set_defaults(
        _handler=_update_book,
        _command="update-book",
        _fields=(),
    )


def _list_books(args: argparse.Namespace) -> Any:
    client = build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    if args.cursor is not None:
        kwargs["cursor"] = args.cursor
    kwargs["genre"] = args.genre
    if args.sort is not None:
        kwargs["sort"] = args.sort
    return client.list_books(**kwargs)


def _create_book(args: argparse.Namespace) -> Any:
    client = build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    payload = load_body(args)
    if payload is not None:
        kwargs["body"] = Book.model_validate(payload)
    return client.create_book(**kwargs)


def _get_book(args: argparse.Namespace) -> Any:
    client = build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    kwargs["book_id"] = args.book_id
    if args.fmt is not None:
        kwargs["fmt"] = args.fmt
    return client.get_book(**kwargs)


def _update_book(args: argparse.Namespace) -> Any:
    client = build_client(args.base_url)
    kwargs: dict[str, Any] = {}
    kwargs["book_id"] = args.book_id
    payload = load_body(args)
    if payload is not None:
        kwargs["body"] = BookFilters.model_validate(payload)
    return client.update_book(**kwargs)
