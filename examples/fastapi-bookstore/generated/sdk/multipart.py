from __future__ import annotations


class MultipartFile:
    """A named binary part in a multipart request body."""

    __slots__ = ("filename", "content")

    def __init__(self, filename: str, content: bytes) -> None:
        if not isinstance(filename, str):
            raise TypeError("multipart filename must be a string")
        if not filename:
            raise ValueError("multipart filename must not be empty")
        if "\r" in filename or "\n" in filename:
            raise ValueError("multipart filename must not contain newlines")
        if not isinstance(content, bytes):
            raise TypeError("multipart file content must be bytes")
        self.filename = filename
        self.content = content
