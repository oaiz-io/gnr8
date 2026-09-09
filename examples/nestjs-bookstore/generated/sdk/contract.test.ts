import { Client } from "./client";
import { ApiError } from "./errors";

const BASE_URL = "http://gnr8.test";

/** One request the generated client handed to its transport. */
interface ContractRequest {
  method: string;
  path: string;
  query: Record<string, string[]>;
  headers: Record<string, string>;
  body: string | null;
  redirect: string;
}

/** One named contract-test case. */
export interface ContractCase {
  name: string;
  run: () => Promise<void>;
}

/**
 * Answers canned responses and records what the client sent.
 *
 * Installed through `ClientOptions.fetch`, the seam the generated client already exposes, so the
 * request under assertion is the one the client would really have sent.
 */
class ContractTransport {
  readonly requests: ContractRequest[] = [];
  private readonly responses: Array<() => Response> = [];

  queue(status: number, headers: Record<string, string>, body: string): void {
    this.responses.push(() => new Response(body === "" ? null : body, { status, headers }));
  }

  readonly fetch: typeof fetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    const url = new URL(String(input));
    const query: Record<string, string[]> = {};
    url.searchParams.forEach((value, key) => {
      (query[key] ??= []).push(value);
    });
    const headers: Record<string, string> = {};
    for (const [name, value] of Object.entries(
      (init?.headers ?? {}) as Record<string, string>,
    )) {
      headers[name.toLowerCase()] = value;
    }
    this.requests.push({
      method: init?.method ?? "GET",
      path: url.pathname,
      query,
      headers,
      body: typeof init?.body === "string" ? init.body : null,
      redirect: String(init?.redirect ?? ""),
    });
    const next = this.responses.shift();
    if (next === undefined) {
      throw new Error("contract transport ran out of canned responses");
    }
    return next();
  };
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonical);
  }
  if (value !== null && typeof value === "object") {
    const source = value as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(source).sort()) {
      out[key] = canonical(source[key]);
    }
    return out;
  }
  return value;
}

function assertEqual(actual: unknown, expected: unknown, what: string): void {
  const got = JSON.stringify(canonical(actual));
  const want = JSON.stringify(canonical(expected));
  if (got !== want) {
    throw new Error(`${what}: got ${got}, want ${want}`);
  }
}

function singleRequest(transport: ContractTransport): ContractRequest {
  if (transport.requests.length !== 1) {
    throw new Error(
      `expected exactly 1 request, got ${transport.requests.length}`,
    );
  }
  return transport.requests[0] as ContractRequest;
}

function assertWire(
  request: ContractRequest,
  method: string,
  path: string,
  query: Record<string, string[]>,
  headers: Record<string, string>,
): void {
  assertEqual(request.method, method, "method");
  assertEqual(request.path, path, "path");
  assertEqual(request.query, query, "query");
  for (const [name, value] of Object.entries(headers)) {
    assertEqual(request.headers[name], value, `header ${name}`);
  }
  assertEqual(request.redirect, "manual", "redirect policy");
}

function assertBody(request: ContractRequest, expected: string): void {
  assertEqual(JSON.parse(request.body ?? "null"), JSON.parse(expected), "request body");
}

function assertApiError(caught: unknown, status: number): void {
  if (!(caught instanceof ApiError)) {
    throw new Error(`expected an ApiError, got ${String(caught)}`);
  }
  assertEqual(caught.status, status, "status");
}

export const contractTests: ContractCase[] = [
  {
    name: "request_shape_list_books",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"books\":[{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"rating\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\"}],\"nextCursor\":\"gnr8\",\"total\":1.5}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.listBooks({ cursor: "gnr8", genre: "gnr8", sort: "gnr8" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/", { cursor: ["gnr8"], genre: ["gnr8"], sort: ["gnr8"] }, {});
      assertEqual(result.nextCursor, "gnr8", "nextCursor");
    },
  },
  {
    name: "request_shape_create_book",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(201, { "content-type": "application/json" }, "{\"id\":1.5,\"message\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.createBook({ author: { bio: "gnr8", name: "gnr8" }, format: "hardcover", id: 1.5, title: "gnr8" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "POST", "/books/", {}, { "content-type": "application/json" });
      assertBody(request, "{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"title\":\"gnr8\"}");
      assertEqual(result.id, 1.5, "id");
    },
  },
  {
    name: "request_shape_get_book",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"rating\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.getBook(1.5, { fmt: "hardcover" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/1.5", { fmt: ["hardcover"] }, {});
    },
  },
  {
    name: "request_shape_update_book",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"id\":1.5,\"message\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.updateBook(1.5, { genre: "gnr8", published: 1.5 });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "PUT", "/books/1.5", {}, { "content-type": "application/json" });
      assertBody(request, "{\"genre\":\"gnr8\",\"published\":1.5}");
      assertEqual(result.id, 1.5, "id");
    },
  },
  {
    name: "response_decode_list_books_present",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"books\":[{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"rating\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\"}],\"nextCursor\":\"gnr8\",\"total\":1.5}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.listBooks({ cursor: "gnr8", genre: "gnr8", sort: "gnr8" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/", { cursor: ["gnr8"], genre: ["gnr8"], sort: ["gnr8"] }, {});
      assertEqual(result.nextCursor, "gnr8", "nextCursor");
    },
  },
  {
    name: "response_decode_create_book_present",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(201, { "content-type": "application/json" }, "{\"id\":1.5,\"message\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.createBook({ author: { bio: "gnr8", name: "gnr8" }, format: "hardcover", id: 1.5, title: "gnr8" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "POST", "/books/", {}, { "content-type": "application/json" });
      assertBody(request, "{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"title\":\"gnr8\"}");
      assertEqual(result.id, 1.5, "id");
    },
  },
  {
    name: "response_decode_get_book_present",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"author\":{\"bio\":\"gnr8\",\"name\":\"gnr8\"},\"format\":\"hardcover\",\"id\":1.5,\"rating\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.getBook(1.5, { fmt: "hardcover" });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/1.5", { fmt: ["hardcover"] }, {});
    },
  },
  {
    name: "response_decode_update_book_present",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(200, { "content-type": "application/json" }, "{\"id\":1.5,\"message\":\"gnr8\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      const result = await client.updateBook(1.5, { genre: "gnr8", published: 1.5 });
      void result;
      const request = singleRequest(transport);
      assertWire(request, "PUT", "/books/1.5", {}, { "content-type": "application/json" });
      assertBody(request, "{\"genre\":\"gnr8\",\"published\":1.5}");
      assertEqual(result.id, 1.5, "id");
    },
  },
  {
    name: "typed_error_list_books_400",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(400, { "content-type": "application/json" }, "{\"message\":\"contract test error\",\"slug\":\"contract_test_error\"}");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      let caught: unknown = undefined;
      try {
        await client.listBooks({ cursor: "gnr8", genre: "gnr8", sort: "gnr8" });
      } catch (error) {
        caught = error;
      }
      assertApiError(caught, 400);
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/", { cursor: ["gnr8"], genre: ["gnr8"], sort: ["gnr8"] }, {});
    },
  },
  {
    name: "redirect_policy_list_books",
    run: async () => {
      const transport = new ContractTransport();
      transport.queue(302, { location: "http://gnr8.test/moved" }, "");
      const client = new Client({ baseUrl: BASE_URL, fetch: transport.fetch });
      let caught: unknown = undefined;
      try {
        await client.listBooks({ cursor: "gnr8", genre: "gnr8", sort: "gnr8" });
      } catch (error) {
        caught = error;
      }
      // The 0.11 contract: a redirect is surfaced, never followed, unless the
      // caller opts in with followRedirects.
      assertApiError(caught, 302);
      const request = singleRequest(transport);
      assertWire(request, "GET", "/books/", { cursor: ["gnr8"], genre: ["gnr8"], sort: ["gnr8"] }, {});
    },
  },
];
