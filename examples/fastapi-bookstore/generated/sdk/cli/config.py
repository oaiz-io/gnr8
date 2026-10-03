"""Constants fixed when this client was generated."""

PROGRAM = "bookstore"
DEFAULT_BASE_URL = "http://127.0.0.1:8000"
VERSION = "bookstore 0.1.0"
FORMAT_ENV = "BOOKSTORE_FORMAT"
DEBUG_ENV = "BOOKSTORE_DEBUG"
NO_INPUT_ENV = "BOOKSTORE_NO_INPUT"
OUTPUT_DIR_ENV = "BOOKSTORE_OUTPUT_DIR"
PAGER_ENV = "BOOKSTORE_PAGER"
DESCRIPTION = "Bookstore API"
HELP_SPEC = (
    '{"commands":[{"arguments":[],"docsUrl":"https://example.com/cli/books/list","examp'
    'les":["bookstore books list --genre fiction"],"flags":[{"help":"required","name":"'
    'genre","required":true,"type":"string"},{"name":"sort","required":false,"type":"st'
    'ring"},{"help":"stop after this many items","name":"limit","required":false,"type"'
    ':"integer"},{"help":"keep following pages until the last one","name":"all","requir'
    'ed":false,"type":"boolean"},{"help":"resume from this cursor","name":"cursor","req'
    'uired":false,"type":"string"}],"invocation":"books list","operation":"list_books",'
    '"output":"ListBooksResponse","seeAlso":["books get"]},{"arguments":[],"examples":['
    '"bookstore books create --body \'{\\"title\\":\\"Dune\\"}\'"],"flags":[{"help":"re'
    'quest body, as an inline JSON document","name":"body","required":false,"type":"str'
    'ing"},{"help":"read the request body from a file, or - for stdin","name":"body-fil'
    'e","required":false,"type":"string"}],"invocation":"books create","operation":"cre'
    'ate_book","output":"CreatedMessage","seeAlso":[]},{"arguments":["book_id"],"exampl'
    'es":["bookstore books get 1"],"flags":[{"enum":["hardcover","paperback"],"name":"f'
    'mt","required":false,"type":"string"}],"invocation":"books get","operation":"get_b'
    'ook","output":"BookOrError","seeAlso":[]},{"arguments":["book_id"],"examples":["bo'
    'okstore books update 1 --title Dune"],"flags":[{"name":"genre","required":false,"t'
    'ype":"string"},{"name":"in-stock","required":false,"type":"string"},{"name":"publi'
    'shed","required":false,"type":"string"},{"name":"sort","required":false,"type":"st'
    'ring"},{"help":"request body, as an inline JSON document","name":"body","required"'
    ':false,"type":"string"},{"help":"read the request body from a file, or - for stdin'
    '","name":"body-file","required":false,"type":"string"}],"invocation":"books update'
    '","operation":"update_book","output":"CreatedMessage","seeAlso":[]}],"program":"bo'
    'okstore"}'
)
