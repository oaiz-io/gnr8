import {
  ApiError,
  AuthConfigurationError,
  ResponseDecodeError,
} from "./errors";
import * as models from "./models";

export interface RequestOptions {
  timeoutMs?: number;
  maxRetries?: number;
  idempotencyKey?: string;
  headers?: Record<string, string>;
  metadata?: Record<string, string>;
  signal?: AbortSignal;
  followRedirects?: boolean;
}

export interface HookContext {
  operationId: string;
  method: string;
  pathTemplate: string;
  url: string;
  headers: Record<string, string>;
  requestMetadata: Record<string, string>;
  status?: number;
  responseHeaders?: Headers;
  opaqueRedirect?: boolean;
}

export type RequestHook = (
  context: HookContext,
  init: RequestInit,
) => void | Promise<void>;
export type ResponseHook = (
  context: HookContext,
  response: Response,
) => void | Promise<void>;
export type ErrorHook = (
  context: HookContext,
  error: unknown,
) => void | Promise<void>;

export interface ClientHooks {
  request?: RequestHook[];
  response?: ResponseHook[];
  error?: ErrorHook[];
}

export interface ClientOptions {
  baseUrl: string;
  fetch?: typeof fetch;
  authMode?: "credentials" | "transport";
  apiKey?: string;
  apiKeys?: Record<string, string>;
  timeoutMs?: number;
  maxRetries?: number;
  hooks?: ClientHooks;
}

interface RuntimeRequestContext {
  operationId: string;
  pathTemplate: string;
  idempotent?: boolean;
  idempotencyKeyHeader?: string;
  successStatuses?: readonly number[];
}

interface AuthRequirement {
  schemeId: string;
  kind: "apiKey" | "bearer" | "basic";
  name?: string;
}

/** First transport-error backoff step; doubles per attempt up to MAX_RETRY_DELAY_MS. */
const BASE_RETRY_DELAY_MS = 100;
/**
 * Ceiling for the TOTAL time spent waiting between retries, including any server-supplied
 * Retry-After. A per-wait cap alone still lets maxRetries x cap accumulate, so the budget is
 * spent down across the whole retry sequence and retrying stops once it is exhausted.
 */
const MAX_RETRY_DELAY_MS = 60000;

export class Client {
  private readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;
  private readonly authMode: "credentials" | "transport";
  private readonly apiKey?: string | undefined;
  private readonly apiKeys: Record<string, string>;
  private readonly timeoutMs?: number;
  private readonly maxRetries: number;
  private readonly retryStatuses: Set<number>;
  private readonly retryUnsafeMethods: boolean;
  private readonly hooks: Required<ClientHooks>;
  private readonly bearerToken?: string;
  private readonly basicAuth?: { username: string; password: string };

  constructor(opts: ClientOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    // The global fetch must stay bound to the global object. Stored unbound and then called as
    // `this.fetchFn(...)`, its receiver becomes the Client and browsers reject it with
    // "TypeError: Illegal invocation". A caller-supplied fetch is left alone — it may be a
    // method that depends on its own receiver.
    this.fetchFn = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.authMode = opts.authMode ?? "credentials";
    this.apiKey = opts.apiKey;
    this.apiKeys = opts.apiKeys ?? {};
    this.timeoutMs = opts.timeoutMs ?? 30000;
    this.maxRetries = opts.maxRetries ?? 0;
    this.retryStatuses = new Set<number>([408, 429]);
    this.retryUnsafeMethods = false;
    this.hooks = {
      request: opts.hooks?.request ?? [],
      response: opts.hooks?.response ?? [],
      error: opts.hooks?.error ?? [],
    };
  }

  _apiKey(...names: string[]): string | undefined {
    for (const name of names) {
      const value = this.apiKeys[name];
      if (value !== undefined && value !== "") {
        return value;
      }
    }
    return this.apiKey === "" ? undefined : this.apiKey;
  }

  _selectAuthAlternative(
    operationId: string,
    alternatives: readonly (readonly AuthRequirement[])[],
  ): Set<string> {
    if (alternatives.length === 0) {
      return new Set<string>();
    }
    if (this.authMode === "transport") {
      return new Set<string>();
    }
    for (const alternative of alternatives) {
      const satisfied = alternative.every((requirement) => {
        if (requirement.kind === "apiKey") {
          return (
            this._apiKey(requirement.schemeId, requirement.name ?? "") !==
            undefined
          );
        }
        if (requirement.kind === "bearer") {
          return this.bearerToken !== undefined;
        }
        return this.basicAuth !== undefined;
      });
      if (satisfied) {
        return new Set(alternative.map((requirement) => requirement.schemeId));
      }
    }
    throw new AuthConfigurationError(
      operationId,
      alternatives.map((alternative) =>
        alternative.map((requirement) => requirement.schemeId),
      ),
    );
  }

  async _decodeJson<T>(response: Response): Promise<T> {
    const rawBody = await response.text();
    const actualContentType = response.headers.get("content-type") ?? undefined;
    const mediaType =
      actualContentType?.split(";", 1)[0]?.trim().toLowerCase() ?? "";
    const errorInit = {
      headers: response.headers,
      requestId: response.headers.get("x-request-id") ?? undefined,
      rawBody,
      expectedContentType: "application/json",
      actualContentType,
    };
    if (rawBody.length === 0) {
      throw new ResponseDecodeError("empty_body", response.status, errorInit);
    }
    if (mediaType !== "application/json" && !mediaType.endsWith("+json")) {
      throw new ResponseDecodeError(
        "unexpected_content_type",
        response.status,
        errorInit,
      );
    }
    try {
      return JSON.parse(rawBody) as T;
    } catch (cause) {
      throw new ResponseDecodeError("invalid_json", response.status, {
        ...errorInit,
        cause,
      });
    }
  }

  async _decodeText(response: Response): Promise<string> {
    const bytes = new Uint8Array(await response.arrayBuffer());
    try {
      return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
        bytes,
      );
    } catch (cause) {
      throw new ResponseDecodeError("invalid_text", response.status, {
        headers: response.headers,
        requestId: response.headers.get("x-request-id") ?? undefined,
        expectedContentType: "text/*",
        actualContentType: response.headers.get("content-type") ?? undefined,
        cause,
      });
    }
  }

  async _readErrorBody(
    response: Response,
  ): Promise<{ rawBody: string; jsonBody: unknown }> {
    const rawBody = await response.text();
    if (rawBody.length === 0) {
      return { rawBody, jsonBody: null };
    }
    const actualContentType =
      response.headers
        .get("content-type")
        ?.split(";", 1)[0]
        ?.trim()
        .toLowerCase() ?? "";
    if (
      actualContentType !== "application/json" &&
      !actualContentType.endsWith("+json")
    ) {
      return { rawBody, jsonBody: null };
    }
    try {
      return { rawBody, jsonBody: JSON.parse(rawBody) as unknown };
    } catch {
      return { rawBody, jsonBody: null };
    }
  }

  private _encodeBody(body: unknown): BodyInit | undefined {
    if (body === undefined) {
      return undefined;
    }
    if (
      body instanceof URLSearchParams ||
      body instanceof FormData ||
      body instanceof Blob ||
      body instanceof ArrayBuffer ||
      typeof body === "string"
    ) {
      return body;
    }
    if (ArrayBuffer.isView(body)) {
      return new Blob([body as unknown as BlobPart]);
    }
    return JSON.stringify(body);
  }

  _formBody(body: unknown): URLSearchParams {
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(
      body as Record<string, unknown>,
    )) {
      if (value === undefined || value === null) {
        continue;
      }
      if (Array.isArray(value)) {
        for (const item of value) {
          params.append(key, String(item));
        }
      } else {
        params.set(key, String(value));
      }
    }
    return params;
  }

  _multipartBody(body: unknown): FormData {
    const form = new FormData();
    for (const [key, value] of Object.entries(
      body as Record<string, unknown>,
    )) {
      if (value === undefined || value === null) {
        continue;
      }
      if (Array.isArray(value)) {
        for (const item of value) {
          this._appendMultipartValue(form, key, item);
        }
      } else {
        this._appendMultipartValue(form, key, value);
      }
    }
    return form;
  }

  private _appendMultipartValue(
    form: FormData,
    key: string,
    value: unknown,
  ): void {
    if (value === undefined || value === null) {
      return;
    }
    if (value instanceof Blob) {
      form.append(key, value);
    } else if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
      form.append(key, new Blob([value as BlobPart]), key);
    } else {
      form.append(key, String(value));
    }
  }

  async _request(
    method: string,
    path: string,
    headers: Record<string, string>,
    body?: unknown,
    requestContext?: RuntimeRequestContext,
    options: RequestOptions = {},
  ): Promise<Response> {
    const context = requestContext ?? { operationId: "", pathTemplate: path };
    const url = `${this.baseUrl}${path}`;
    const requestMetadata = options.metadata ?? {};
    Object.assign(headers, options.headers ?? {});
    if (context.idempotent === true && options.idempotencyKey !== undefined) {
      headers[context.idempotencyKeyHeader ?? "Idempotency-Key"] =
        options.idempotencyKey;
    }
    const maxRetries = Math.max(0, options.maxRetries ?? this.maxRetries);
    const retryAttempts =
      this.retryUnsafeMethods ||
      context.idempotent === true ||
      this._retryableMethod(method)
        ? maxRetries
        : 0;
    const timeoutMs = options.timeoutMs ?? this.timeoutMs;
    const bodyPayload = this._encodeBody(body);
    let lastError: unknown = undefined;
    let retryBudgetMs = MAX_RETRY_DELAY_MS;
    for (let attempt = 0; attempt <= retryAttempts; attempt += 1) {
      const controller =
        timeoutMs !== undefined && timeoutMs > 0
          ? new AbortController()
          : undefined;
      const timeoutId =
        controller === undefined
          ? undefined
          : setTimeout(() => controller.abort(), timeoutMs);
      const signals = [options.signal, controller?.signal].filter(
        (signal): signal is AbortSignal => signal !== undefined,
      );
      const init: RequestInit = {
        method,
        headers,
        body: bodyPayload ?? null,
        redirect: options.followRedirects === true ? "follow" : "manual",
        signal:
          signals.length > 1 ? AbortSignal.any(signals) : (signals[0] ?? null),
      };
      const hookContext: HookContext = {
        operationId: context.operationId,
        method,
        pathTemplate: context.pathTemplate,
        url,
        headers: { ...headers },
        requestMetadata,
      };
      try {
        for (const hook of this.hooks.request) {
          await hook(hookContext, init);
        }
      } catch (error) {
        if (timeoutId !== undefined) {
          clearTimeout(timeoutId);
        }
        for (const hook of this.hooks.error) {
          await hook(hookContext, error);
        }
        throw error;
      }
      let response: Response | undefined = undefined;
      try {
        response = await this.fetchFn(url, init);
        if (timeoutId !== undefined) {
          clearTimeout(timeoutId);
        }
      } catch (error) {
        if (timeoutId !== undefined) {
          clearTimeout(timeoutId);
        }
        lastError = error;
        if (attempt < retryAttempts && retryBudgetMs > 0) {
          // Back off before reconnecting: retrying a refused connection instantly just
          // multiplies load on a service that is already restarting.
          const delayMs = Math.min(
            this._backoffDelayMs(attempt),
            retryBudgetMs,
          );
          retryBudgetMs -= delayMs;
          await this._waitBeforeRetry(delayMs, options.signal, hookContext);
          continue;
        }
        for (const hook of this.hooks.error) {
          await hook(hookContext, error);
        }
        throw error;
      }
      if (response === undefined) {
        throw new Error("request failed without response");
      }
      const opaqueRedirect =
        response.type === "opaqueredirect" &&
        options.followRedirects !== true &&
        (context.successStatuses ?? []).some(
          (status) => status >= 300 && status < 400,
        );
      hookContext.status = response.status;
      hookContext.responseHeaders = response.headers;
      hookContext.opaqueRedirect = opaqueRedirect;
      try {
        for (const hook of this.hooks.response) {
          await hook(hookContext, response);
        }
      } catch (error) {
        for (const hook of this.hooks.error) {
          await hook(hookContext, error);
        }
        throw error;
      }
      if (
        this._shouldRetryStatus(response.status) &&
        attempt < retryAttempts &&
        retryBudgetMs > 0
      ) {
        // Release the connection before retrying. An unread body keeps its socket checked out
        // of the pool until GC, so a retry storm can exhaust the pool.
        await this._discardBody(response);
        const delayMs = Math.min(
          this._retryDelayMs(response, attempt),
          retryBudgetMs,
        );
        retryBudgetMs -= delayMs;
        await this._waitBeforeRetry(delayMs, options.signal, hookContext);
        continue;
      }
      if (
        (response.status < 200 || response.status >= 300) &&
        !(context.successStatuses ?? []).includes(response.status) &&
        !opaqueRedirect
      ) {
        const error = new ApiError(response.status, {
          headers: response.headers,
          requestId: response.headers.get("x-request-id") ?? undefined,
        });
        for (const hook of this.hooks.error) {
          await hook(hookContext, error);
        }
      }
      return response;
    }
    throw lastError ?? new Error("request failed without response");
  }

  private _retryableMethod(method: string): boolean {
    return (
      method === "GET" ||
      method === "HEAD" ||
      method === "OPTIONS" ||
      method === "PUT" ||
      method === "DELETE"
    );
  }

  private _shouldRetryStatus(status: number): boolean {
    return this.retryStatuses.has(status) || status >= 500;
  }

  private async _discardBody(response: Response): Promise<void> {
    try {
      await response.body?.cancel();
    } catch {
      // Already consumed or errored; the connection is being torn down either way.
    }
  }

  private _retryDelayMs(response: Response, attempt: number): number {
    const retryAfter = response.headers.get("Retry-After");
    if (retryAfter !== null) {
      const seconds = Number.parseInt(retryAfter, 10);
      if (Number.isFinite(seconds) && seconds > 0) {
        // A server may ask for an arbitrarily long wait. Honour it only up to the ceiling,
        // so a hostile or misconfigured origin cannot park the caller for hours.
        return Math.min(seconds * 1000, MAX_RETRY_DELAY_MS);
      }
    }
    return this._backoffDelayMs(attempt);
  }

  private _backoffDelayMs(attempt: number): number {
    return Math.min(BASE_RETRY_DELAY_MS * 2 ** attempt, MAX_RETRY_DELAY_MS);
  }

  private async _waitBeforeRetry(
    ms: number,
    signal: AbortSignal | undefined,
    hookContext: HookContext,
  ): Promise<void> {
    try {
      await this._sleep(ms, signal);
    } catch (error) {
      // Every other failure path notifies the error hooks; an abort landing mid-wait must not
      // be the one exception, or callers lose the event entirely.
      for (const hook of this.hooks.error) {
        await hook(hookContext, error);
      }
      throw error;
    }
  }

  private async _sleep(ms: number, signal?: AbortSignal): Promise<void> {
    if (ms <= 0) {
      return;
    }
    if (signal?.aborted === true) {
      throw signal.reason ?? new Error("request aborted");
    }
    // The wait must observe cancellation: without this an aborted or timed-out request still
    // blocks for the full retry delay before anyone notices.
    await new Promise<void>((resolve, reject) => {
      const onAbort = () => {
        clearTimeout(timer);
        reject(signal?.reason ?? new Error("request aborted"));
      };
      const timer = setTimeout(() => {
        signal?.removeEventListener("abort", onAbort);
        resolve();
      }, ms);
      signal?.addEventListener("abort", onAbort, { once: true });
    });
  }

  /**
   * List books in one genre.
   *
   * Results are ordered by title and paginated with an opaque cursor. Pass the
   * cursor from the previous page to continue; omit it to start from the beginning.
   */
  async listBooks(
    params: ListBooksParams,
    options?: RequestOptions,
  ): Promise<models.ListBooksResponse> {
    let path = `/books/`;
    const searchParams = new URLSearchParams();
    for (const [wireName, wireValue] of wireParameterPairs(
      "genre",
      params.genre,
      "form",
      true,
    )) {
      searchParams.append(wireName, wireValue);
    }
    if (params.cursor !== undefined) {
      for (const [wireName, wireValue] of wireParameterPairs(
        "cursor",
        params.cursor,
        "form",
        true,
      )) {
        searchParams.append(wireName, wireValue);
      }
    }
    if (params.sort !== undefined) {
      for (const [wireName, wireValue] of wireParameterPairs(
        "sort",
        params.sort,
        "form",
        true,
      )) {
        searchParams.append(wireName, wireValue);
      }
    }
    const qs = wireQueryString(searchParams);
    if (qs) {
      path = path + "?" + qs;
    }
    const headers: Record<string, string> = {};
    const res = await this._request(
      "GET",
      path,
      headers,
      undefined,
      {
        operationId: "listBooks",
        pathTemplate: "/books/",
        idempotent: false,
        idempotencyKeyHeader: "Idempotency-Key",
        successStatuses: [200],
      },
      options,
    );
    if (res.status < 200 || res.status >= 300) {
      const { rawBody, jsonBody } = await this._readErrorBody(res);
      let errorBody: unknown = jsonBody;
      throw new ApiError(res.status, {
        headers: res.headers,
        requestId: res.headers.get("x-request-id") ?? undefined,
        rawBody,
        jsonBody,
        body: errorBody,
      });
    }
    if (res.status === 200) {
      return await this._decodeJson<models.ListBooksResponse>(res);
    }
    throw new ApiError(res.status);
  }

  /**
   * Add a book to the catalogue.
   *
   * The book is created immediately and its generated identifier is returned.
   */
  async createBook(
    body: models.BookDto,
    options?: RequestOptions,
  ): Promise<models.CreatedMessage> {
    let path = `/books/`;
    const headers: Record<string, string> = {};
    headers["Content-Type"] = "application/json";
    const requestBody = JSON.stringify(body);
    const res = await this._request(
      "POST",
      path,
      headers,
      requestBody,
      {
        operationId: "createBook",
        pathTemplate: "/books/",
        idempotent: false,
        idempotencyKeyHeader: "Idempotency-Key",
        successStatuses: [201],
      },
      options,
    );
    if (res.status < 200 || res.status >= 300) {
      const { rawBody, jsonBody } = await this._readErrorBody(res);
      let errorBody: unknown = jsonBody;
      throw new ApiError(res.status, {
        headers: res.headers,
        requestId: res.headers.get("x-request-id") ?? undefined,
        rawBody,
        jsonBody,
        body: errorBody,
      });
    }
    if (res.status === 201) {
      return await this._decodeJson<models.CreatedMessage>(res);
    }
    throw new ApiError(res.status);
  }

  /**
   * Fetch one book by its identifier.
   *
   * Returns the book when it is in stock, and an out-of-stock notice otherwise.
   */
  async getBook(
    bookId: number,
    params?: GetBookParams,
    options?: RequestOptions,
  ): Promise<models.BookOrError> {
    let path = `/books/${wireEscape(String(bookId))}`;
    const searchParams = new URLSearchParams();
    if (params?.fmt !== undefined) {
      for (const [wireName, wireValue] of wireParameterPairs(
        "fmt",
        params.fmt,
        "form",
        true,
      )) {
        searchParams.append(wireName, wireValue);
      }
    }
    const qs = wireQueryString(searchParams);
    if (qs) {
      path = path + "?" + qs;
    }
    const headers: Record<string, string> = {};
    const res = await this._request(
      "GET",
      path,
      headers,
      undefined,
      {
        operationId: "getBook",
        pathTemplate: "/books/{bookId}",
        idempotent: false,
        idempotencyKeyHeader: "Idempotency-Key",
        successStatuses: [200],
      },
      options,
    );
    if (res.status < 200 || res.status >= 300) {
      const { rawBody, jsonBody } = await this._readErrorBody(res);
      let errorBody: unknown = jsonBody;
      throw new ApiError(res.status, {
        headers: res.headers,
        requestId: res.headers.get("x-request-id") ?? undefined,
        rawBody,
        jsonBody,
        body: errorBody,
      });
    }
    if (res.status === 200) {
      return await this._decodeJson<models.BookOrError>(res);
    }
    throw new ApiError(res.status);
  }

  /**
   * Update the stored filters for one book.
   *
   * Filters left unset in the payload keep their current values.
   */
  async updateBook(
    bookId: number,
    body: models.BookFilters,
    options?: RequestOptions,
  ): Promise<models.CreatedMessage> {
    let path = `/books/${wireEscape(String(bookId))}`;
    const headers: Record<string, string> = {};
    headers["Content-Type"] = "application/json";
    const requestBody = JSON.stringify(body);
    const res = await this._request(
      "PUT",
      path,
      headers,
      requestBody,
      {
        operationId: "updateBook",
        pathTemplate: "/books/{bookId}",
        idempotent: false,
        idempotencyKeyHeader: "Idempotency-Key",
        successStatuses: [200],
      },
      options,
    );
    if (res.status < 200 || res.status >= 300) {
      const { rawBody, jsonBody } = await this._readErrorBody(res);
      let errorBody: unknown = jsonBody;
      throw new ApiError(res.status, {
        headers: res.headers,
        requestId: res.headers.get("x-request-id") ?? undefined,
        rawBody,
        jsonBody,
        body: errorBody,
      });
    }
    if (res.status === 200) {
      return await this._decodeJson<models.CreatedMessage>(res);
    }
    throw new ApiError(res.status);
  }
}

export type ListBooksParams = {
  cursor?: string;
  genre: string;
  sort?: string;
};

export type GetBookParams = {
  fmt?: models.BookFormat;
};

function wireParameterPairs(
  name: string,
  value: unknown,
  style: string,
  explode: boolean,
): Array<[string, string]> {
  const delimiter =
    style === "spaceDelimited" ? " " : style === "pipeDelimited" ? "|" : ",";
  if (Array.isArray(value)) {
    const parts = value.map((item) => String(item));
    return explode && style === "form"
      ? parts.map((item) => [name, item])
      : [[name, parts.join(delimiter)]];
  }
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).sort(
      ([a], [b]) => a.localeCompare(b),
    );
    if (style === "deepObject") {
      return entries.map(([key, item]) => [
        name + "[" + key + "]",
        String(item),
      ]);
    }
    if (explode && style === "form") {
      return entries.map(([key, item]) => [key, String(item)]);
    }
    const parts = entries.flatMap(([key, item]) =>
      explode ? [key + "=" + String(item)] : [key, String(item)],
    );
    return [[name, parts.join(delimiter)]];
  }
  return [[name, String(value)]];
}

function wireQueryString(
  values: URLSearchParams,
  allowReserved: Set<number> = new Set<number>(),
): string {
  const restoreReserved = (value: string): string =>
    value.replace(
      /%3A|%2F|%3F|%23|%5B|%5D|%40|%21|%24|%26|%27|%28|%29|%2A|%2B|%2C|%3B|%3D/gi,
      (token) => decodeURIComponent(token),
    );
  const parts: string[] = [];
  let index = 0;
  values.forEach((value, key) => {
    const encoded = wireEscape(value);
    parts.push(
      wireEscape(key) +
        "=" +
        (allowReserved.has(index) ? restoreReserved(encoded) : encoded),
    );
    index += 1;
  });
  return parts.join("&");
}

function wireEscape(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (char) => "%" + char.charCodeAt(0).toString(16).toUpperCase(),
  );
}
