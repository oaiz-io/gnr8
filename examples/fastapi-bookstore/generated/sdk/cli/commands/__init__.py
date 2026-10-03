"""One module per command group; each registers its own subparsers."""

from . import books

__all__ = ["books"]
