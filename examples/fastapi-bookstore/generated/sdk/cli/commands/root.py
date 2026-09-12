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
        default=DEFAULT_BASE_URL,
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
    cmd_list_books.set_defaults(_handler=_list_books)
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
        default=DEFAULT_BASE_URL,
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
    cmd_create_book.set_defaults(_handler=_create_book)
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
        default=DEFAULT_BASE_URL,
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
        choices=("hardcover", "paperback"),
    )
    cmd_get_book.set_defaults(_handler=_get_book)
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
        default=DEFAULT_BASE_URL,
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
    cmd_update_book.set_defaults(_handler=_update_book)


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
