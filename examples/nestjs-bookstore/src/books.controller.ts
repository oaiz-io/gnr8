// NestJS bookstore controller — STATIC fixture source (Phase 1).
//
// Routing uses @nestjs/common's framework-native decorators (@Controller, @Get,
// @Post, @Put, @Param, @Query, @Body) — the direct Gin analog. Every
// request/response/param fact is derived from the method SIGNATURE + the typed
// DTO classes in `books.dto.ts`; nothing reads a third-party schema-annotation
// decorator or a runtime schema export (AGENTS.md rule 1).
//
// The static @Controller('books') prefix is composed into the neutral graph
// operation paths (`/books/`, `/books/{bookId}`). No app runs this phase; this
// is the static source tsextract reads.
//
// PROVENANCE NOTE (non-fact prose only — rule 1): blank lines / comments below
// are SPACING ONLY, present so each method-name and param-name line anchors to
// the committed graph snapshot's asserted span. The snapshot is authoritative.
import {
  Body,
  Controller,
  Get,
  Param,
  Post,
  Put,
  Query,
} from '@nestjs/common';

import {
  BookDto,
  BookFilters,
  BookFormat,
  BookOrError,
  CreatedMessage,
  ListBooksResponse,
} from './books.dto';
//
// Each handler carries an ordinary JSDoc block. gnr8 reads the LEADING DESCRIPTION
// only — the compiler excludes JSDoc tags, so an `@param`/`@openapi` tag is invisible
// to it and no API fact is ever encoded in a comment (rule 1). Method, path, params,
// body, and status stay derived from the decorators and the typed signature.
@Controller('books')
export class BooksController {
  /**
   * List books in one genre.
   *
   * Results are ordered by title and paginated with an opaque cursor. Pass the
   * cursor from the previous page to continue; omit it to start from the beginning.
   */
  @Get('/')
  listBooks(
    @Query('genre') genre: string,
    @Query('sort') sort: string = 'asc',
    @Query('cursor') cursor?: string,
  ): ListBooksResponse {
    throw new Error('static fixture: never executed this phase');
  }
  /**
   * Add a book to the catalogue.
   *
   * The book is created immediately and its generated identifier is returned.
   */
  @Post('/')
  createBook(@Body() book: BookDto): CreatedMessage {
    throw new Error('static fixture: never executed this phase');
  }
  /**
   * Fetch one book by its identifier.
   *
   * Returns the book when it is in stock, and an out-of-stock notice otherwise.
   */
  @Get('/:bookId')
  getBook(
    @Param('bookId') bookId: number,
    @Query('fmt') fmt?: BookFormat,
  ): BookOrError {
    throw new Error('static fixture: never executed this phase');
  }
  /**
   * Update the stored filters for one book.
   *
   * Filters left unset in the payload keep their current values.
   */
  @Put('/:bookId')
  updateBook(
    @Param('bookId') bookId: number,
    @Body() filters: BookFilters,
  ): CreatedMessage {
    throw new Error('static fixture: never executed this phase');
  }
}
