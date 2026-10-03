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
    '"output":"Book: id, title, author","seeAlso":["books get"]},{"arguments":[],"examp'
    'les":["bookstore books create --body \'{\\"title\\":\\"Dune\\"}\'"],"flags":[{"hel'
    'p":"request body, as an inline JSON document","name":"body","required":false,"type'
    '":"string"},{"help":"read the request body from a file, or - for stdin","name":"bo'
    'dy-file","required":false,"type":"string"}],"invocation":"books create","operation'
    '":"create_book","output":"CreatedMessage","seeAlso":[]},{"arguments":["book_id"],"'
    'examples":["bookstore books get 1"],"flags":[{"enum":["hardcover","paperback"],"na'
    'me":"fmt","required":false,"type":"string"}],"invocation":"books get","operation":'
    '"get_book","output":"BookOrError","seeAlso":[]},{"arguments":["book_id"],"examples'
    '":["bookstore books update 1 --title Dune"],"flags":[{"name":"genre","required":fa'
    'lse,"type":"string"},{"name":"in-stock","required":false,"type":"string"},{"name":'
    '"published","required":false,"type":"string"},{"name":"sort","required":false,"typ'
    'e":"string"},{"help":"request body, as an inline JSON document","name":"body","req'
    'uired":false,"type":"string"},{"help":"read the request body from a file, or - for'
    ' stdin","name":"body-file","required":false,"type":"string"}],"invocation":"books '
    'update","operation":"update_book","output":"CreatedMessage","seeAlso":[]}],"progra'
    'm":"bookstore"}'
)
