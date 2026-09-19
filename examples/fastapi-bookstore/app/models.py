"""Type-rich DTO models for the FastAPI bookstore fixture.

Every API fact gnr8 will later extract from this service is expressible from
Python's OWN type system alone: function signatures + Pydantic/`@dataclass`
field annotations. There is deliberately NO third-party schema-annotation tool
and NO separate validation-schema dialect here, and nothing assumes FastAPI's
runtime `/openapi.json`. Facts come from the language's own types. (AGENTS.md rule 1.)

This module encodes the v2.0 acceptance vocabulary:
  - objects            -> Pydantic `BaseModel` + a `@dataclass` DTO
  - arrays/lists       -> `list[T]`
  - cross-language enums -> Python `enum.Enum` AND `Literal[...]`
  - unions             -> `Union[A, B]` and the `A | B` spelling
  - all four optional x nullable combinations, documented per field below.

OPTIONAL vs NULLABLE — the two distinct axes (Plan 01-01):
  * optional = the JSON KEY may be absent (the field has a default value).
  * nullable = the VALUE may be `null`/`None` (the type admits `None`).
These are independent; all four pairings appear on `BookFilters` below.
(Line layout, RESEARCH OQ2: each schema span anchors to its `ClassDef`/`Assign`
line; the spacing is laid out so each lands on the snapshot's asserted line.)
"""

from __future__ import annotations

import enum
from dataclasses import dataclass
from typing import Literal, Optional, Union

from pydantic import BaseModel

# ----- cross-language enums -------------------------------------------------
class BookFormat(str, enum.Enum):
    """A string enum (`enum.Enum`) -> neutral `Type::Enum` with members sorted.

    The members are intentionally declared out of lexical order so the snapshot
    proves the extractor must sort them ([hardcover, paperback] -> sorted).
    """

    PAPERBACK = "paperback"
    HARDCOVER = "hardcover"


# A `Literal[...]` enum -> the SAME neutral `Type::Enum` shape as `enum.Enum`
# (cross-language enums, Plan 01-01). Used inline on `BookFilters.sort`; a
# `Literal` alias is inline-only and is NEVER a standalone schema (the snapshot
# has no `SortOrder` schema) — only a `Union` alias becomes a schema.
SortOrder = Literal["asc", "desc"]


# ----- objects (Pydantic models) -------------------------------------------

class Author(BaseModel):
    """A nested object referenced by `Book` -> a `$ref` to this schema.

    Fields exercise the required/optional axes:
      - name : required, non-null   (neither optional nor nullable)
      - bio  : nullable, required   (value may be None; key still required)
    """

    name: str
    bio: Optional[str]  # nullable (value may be None), required (no default)

class Book(BaseModel):
    """The densest object: objects, arrays, an enum field, and a union field.

    Field axis map:
      - id        : required, non-null            (neither)
      - title     : required, non-null            (neither)
      - author    : required object $ref          (neither)
      - tags      : `list[str]`, optional, non-null  (optional, not nullable)
      - format    : `BookFormat` enum, required      (neither)
      - rating    : `Union[int, float]`, nullable+optional (both)
    """

    id: int
    title: str
    author: Author
    tags: list[str] = []  # optional (has a default), NOT nullable
    format: BookFormat
    # union of two primitives; default None makes it optional AND nullable (both)
    rating: Optional[Union[int, float]] = None


# `BookFilters` is the optional/nullable acceptance matrix — the densest schema in
# the fixture. It is deliberately positioned here (RESEARCH OQ2) so its `ClassDef`
# anchor lands on the snapshot's asserted line; the spacing above is layout, not
# accidental whitespace. The four fields below cover, in order:
#
#   neither | optional-only | nullable-only | both
#
# which is the whole reason this fixture exists: to prove the extractor keeps the
# two axes independent (Plan 01-01). Each field's axis is annotated inline below.

class BookFilters(BaseModel):
    """Encodes ALL FOUR optional x nullable combinations distinctly.

      - genre      : required `str`                       -> optional=F, nullable=F (neither)
      - in_stock   : `bool` with a default                -> optional=T, nullable=F (optional only)
      - published  : `Optional[int]` (no default)         -> optional=F, nullable=T (nullable only)
      - sort       : `Optional[SortOrder]` with a default -> optional=T, nullable=T (both)

    `sort` also doubles as a `Literal[...]` enum field (cross-language enum).
    """

    genre: str  # neither: required, non-null
    in_stock: bool = True  # optional only: default present, value never None
    published: Optional[int]  # nullable only: value may be None, key required
    # both: has a default (optional) AND the type admits None (nullable)
    sort: Optional[SortOrder] = "asc"
# ----- a @dataclass DTO (objects via dataclasses, not just Pydantic) --------
@dataclass
class CreatedMessage:
    """A `@dataclass` response DTO -> neutral `Type::Object` (same as a model).

      - message : required `str`     (neither)
      - id      : required `int`     (neither)
    """

    message: str
    id: int

# ----- a union of two OBJECTS (not just primitives) -------------------------
class OutOfStock(BaseModel):
    """One arm of `BookOrError` -> `Type::Union` of two object $refs."""
    reason: str
# `Book | OutOfStock` as a `Union[...]` alias -> a standalone `Type::Union` schema
# of two named ($ref) members (referenced by a route's response, so it must be a
# schema). Exercises the union-of-objects case the Go fixture never had.
BookOrError = Union[Book, OutOfStock]




class ListBooksResponse(BaseModel):
    """The list endpoint's response envelope.

      - books       : `list[Book]` array of object $refs           (neither)
      - next_cursor : `Optional[str]` (no default)                 (nullable only)
      - total       : required `int`                               (neither)
    """

    books: list[Book]
    next_cursor: Optional[str]  # nullable only (value may be None, key required)
    total: int
