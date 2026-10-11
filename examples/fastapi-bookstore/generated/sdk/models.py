from __future__ import annotations

import enum
from typing import Any, Literal, Optional, Union

from pydantic import BaseModel, ConfigDict, Field


class Author(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    bio: Optional[str]
    name: str

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> Author:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        if self.bio is None:
            _data["bio"] = None
        return _data


class Book(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    author: Author
    format: BookFormat
    id: int
    rating: Optional[Union[int, float]] = Field(default=None)
    tags: Optional[list[str]] = Field(default=None)
    title: str

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> Book:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        _data["author"] = self.author.to_dict()
        if self.rating is None and "rating" in self.model_fields_set:
            _data["rating"] = None
        return _data


class BookFilters(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    genre: str
    in_stock: Optional[bool] = Field(default=None)
    published: Optional[int]
    sort: Optional[Literal["asc", "desc"]] = Field(default=None)

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> BookFilters:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        if self.published is None:
            _data["published"] = None
        if self.sort is None and "sort" in self.model_fields_set:
            _data["sort"] = None
        return _data


class BookFormat(str, enum.Enum):
    HARDCOVER = "hardcover"
    PAPERBACK = "paperback"


BookOrError = "Union[Book, OutOfStock]"


class CreatedMessage(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    id: int
    message: str

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> CreatedMessage:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        return self.model_dump(mode="json", by_alias=True, exclude_none=True)


class ListBooksResponse(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    books: list[Book]
    next_cursor: Optional[str]
    total: int

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> ListBooksResponse:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        _data["books"] = [_item.to_dict() for _item in self.books]
        if self.next_cursor is None:
            _data["next_cursor"] = None
        return _data


class OutOfStock(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    reason: str

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> OutOfStock:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        return self.model_dump(mode="json", by_alias=True, exclude_none=True)
