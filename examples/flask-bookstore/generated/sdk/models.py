from __future__ import annotations

import enum
from typing import Any, Literal, Optional, Union

from pydantic import BaseModel, ConfigDict, Field


class Availability(str, enum.Enum):
    IN_STOCK = "in_stock"
    OUT_OF_STOCK = "out_of_stock"


class OrderConfirmation(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    availability: Availability
    lines: list[Price]
    message: Optional[str]
    order_id: int

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> OrderConfirmation:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        _data["lines"] = [_item.to_dict() for _item in self.lines]
        if self.message is None:
            _data["message"] = None
        return _data


class OrderInput(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    book_id: int
    coupon: Optional[str] = Field(default=None)
    discount: Optional[Union[int, float]] = Field(default=None)
    note: Optional[str] = Field(default=None)
    price: Price
    quantity: Optional[int] = Field(default=None)
    tags: Optional[list[str]] = Field(default=None)

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> OrderInput:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        _data = self.model_dump(mode="json", by_alias=True, exclude_none=True)
        if self.coupon is None and "coupon" in self.model_fields_set:
            _data["coupon"] = None
        if self.discount is None and "discount" in self.model_fields_set:
            _data["discount"] = None
        if self.note is None and "note" in self.model_fields_set:
            _data["note"] = None
        _data["price"] = self.price.to_dict()
        return _data


class Price(BaseModel):
    model_config = ConfigDict(populate_by_name=True, extra="ignore")
    amount: float
    currency: Literal["eur", "usd"]

    @classmethod
    def from_dict(cls, _data: dict[str, Any]) -> Price:
        return cls.model_validate(_data)

    def to_dict(self) -> dict[str, Any]:
        return self.model_dump(mode="json", by_alias=True, exclude_none=True)
