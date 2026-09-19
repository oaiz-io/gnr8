// Typed DTO classes for the NestJS bookstore fixture.
//
// BRIGHT LINE (AGENTS.md rule 1, the whole product premise): every API fact gnr8
// extracts from these DTOs is carried by ORDINARY TypeScript property types —
// string-literal-union enums, `A | B` unions, `field?: T` optional vs
// `field: T | null` nullable. There is deliberately NO third-party
// schema-annotation decorator and NO separate validation-schema dialect; gnr8
// derives the facts the language's own type system already carries (Phase 4).
//
// optional = the property may be absent (`field?: T`); nullable = the value may
// be `null` (`field: T | null`). Both, neither and each alone appear below.
//
// PROVENANCE NOTE (non-fact prose only — rule 1): the blank lines / comment
// blocks below are SPACING ONLY. They carry no API fact (no decorator, property
// or type) and exist solely so each declaration's source line anchors to the
// committed graph snapshot's asserted span. The snapshot is authoritative.
// ----- cross-language enums (string-literal unions) -----------------------
//
// A string-literal-union enum lowers to a neutral `Type::Enum` with its members
// SORTED ([hardcover, paperback]); `BookFormat` is declared out of lexical order
// on purpose to exercise that sort. It becomes a STANDALONE schema only because a
// DTO field (`format`) and a route query param (`fmt`) reference it by name; an
// alias used solely inline (`SortOrder`, below) never becomes a standalone schema.
//
// (The remaining lines in this block are spacing only — no API fact is encoded in
// any comment, per AGENTS.md rule 1. They exist so the declaration lines anchor to
// the committed snapshot's asserted spans.)
//
//
//
//
//
//
//
//
export type BookFormat = 'paperback' | 'hardcover';
export type SortOrder = 'asc' | 'desc';
// ----- objects (DTO classes) ----------------------------------------------
//
// A nested DTO referenced by `BookDto` -> a `$ref` to this schema. (spacing only)
export class AuthorDto {
  name: string;
  bio: string | null;
}
//
// The densest DTO: a nested object, an array, an enum field and a both-axes field.
export class BookDto {
  id: number;
  title: string;
  author: AuthorDto;
  tags?: string[];
  format: BookFormat;
  rating?: number | null;
}
// A DTO exercising all four optional/nullable combinations (spacing follows).
export class BookFilters {
  genre: string;
  inStock?: boolean;
  published: number | null;
  sort?: SortOrder | null;
}
//
// One arm of a union of two OBJECTS (TS has real sum types, unlike Go). The lines
// between declarations here are SPACING ONLY — no API fact is carried in any
// comment (rule 1); they anchor each declaration to the snapshot's asserted span.
//
//
//
//
export class OutOfStockDto {
  reason: string;
}
export type BookOrError = BookDto | OutOfStockDto;
//
export class CreatedMessage {
  message: string;
  id: number;
}
//
export class ListBooksResponse {
  books: BookDto[];
  nextCursor: string | null;
  total: number;
}
